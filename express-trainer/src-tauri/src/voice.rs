//! 声音层（M3 · 产品方案 §5.4，独有能力 ①）。
//!
//! `VoiceAnalyzer` 随音频流增量计算"怎么说的"：
//! - 句子级 RMS 能量（只在 VAD 判定为语音的采样上累计）
//! - 会话内音量基线：前 3 秒有效语音校准（麦克风差异红线：**永不输出跨设备绝对音量**，
//!   对外只给相对基线的 dB 值与会话内相对统计）
//! - 音量动态范围：各句相对基线 dB 的 p90 − p10（衡量"有没有起伏"）
//! - 能量稳定性：相邻句 dB 差的方差（越小越稳）
//! - 失控停顿：VAD 判静音且持续 >2s 的次数与最长一次；只统计"会话进行中"
//!   （已出现过语音之后）的静音段，开口前的沉默不算
//!
//! 纯状态机：所有时间量由采样数推出（`push_chunk` 不接收时钟），
//! 对同一串输入完全确定，便于合成音频单测。

use crate::rules::Sentence;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 基线校准使用的有效语音时长（秒）
pub const CALIBRATION_SECONDS: f64 = 3.0;
/// 失控停顿阈值：静音超过该毫秒数计一次
pub const RUNAWAY_PAUSE_MS: f64 = 2_000.0;
/// 实时指标推送间隔（session.rs 搭便车循环用）
pub const VOICE_UPDATE_INTERVAL_MS: u64 = 2_000;

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// 对外声音指标（会话内相对值；camelCase 供前端/事件直接使用）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceMetrics {
    /// 前 3 秒有效语音是否已完成基线校准
    pub baseline_calibrated: bool,
    /// 音量动态范围（dB，相对基线 p90−p10）；句能量不足 2 句时为 None
    pub volume_dynamic_range_db: Option<f64>,
    /// 能量稳定性（相邻句 dB 差的方差，越小越稳）；不足 2 句时为 None
    pub energy_stability: Option<f64>,
    /// 失控停顿（静音 >2s）次数
    pub runaway_pause_count: u32,
    /// 最长一次失控停顿（毫秒）
    pub longest_pause_ms: u64,
}

/// 报告层用的完整声音数据：聚合指标 + 各句相对基线 dB 序列
#[derive(Debug, Clone, Default)]
pub struct VoiceReport {
    pub metrics: VoiceMetrics,
    pub sentence_db: Vec<f64>,
}

/// 已排序切片上的分位数（线性插值），q ∈ [0, 100]
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = (q / 100.0) * (sorted.len() - 1) as f64;
    let idx = pos.floor() as usize;
    let frac = pos - idx as f64;
    let a = sorted[idx];
    let b = sorted[(idx + 1).min(sorted.len() - 1)];
    a + (b - a) * frac
}

/// 音量动态范围：相对基线 dB 的 p90 − p10；少于 2 句无意义 → None
pub fn dynamic_range_db(sentence_db: &[f64]) -> Option<f64> {
    if sentence_db.len() < 2 {
        return None;
    }
    let mut sorted = sentence_db.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(round1(percentile(&sorted, 90.0) - percentile(&sorted, 10.0)))
}

/// 能量稳定性：相邻句 dB 差的总体方差；少于 2 句 → None
pub fn energy_stability(sentence_db: &[f64]) -> Option<f64> {
    if sentence_db.len() < 2 {
        return None;
    }
    let diffs: Vec<f64> = sentence_db.windows(2).map(|w| w[1] - w[0]).collect();
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    let var = diffs.iter().map(|d| (d - mean) * (d - mean)).sum::<f64>() / diffs.len() as f64;
    Some(round1(var))
}

pub struct VoiceAnalyzer {
    sample_rate: usize,
    // 基线校准（前 3 秒有效语音）
    calib_sq_sum: f64,
    calib_samples: usize,
    calib_target_samples: usize,
    baseline: Option<f64>,
    // 当前句能量桶（只累计语音采样）
    sent_sq_sum: f64,
    sent_samples: usize,
    sentence_db: Vec<f64>,
    // 失控停顿（仅"会话进行中"：已出现过语音）
    speech_seen: bool,
    silence_ms: f64,
    pause_counted: bool,
    runaway_pause_count: u32,
    longest_pause_ms: f64,
}

impl VoiceAnalyzer {
    pub fn new(sample_rate: usize) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            calib_sq_sum: 0.0,
            calib_samples: 0,
            calib_target_samples: (sample_rate as f64 * CALIBRATION_SECONDS) as usize,
            baseline: None,
            sent_sq_sum: 0.0,
            sent_samples: 0,
            sentence_db: Vec::new(),
            speech_seen: false,
            silence_ms: 0.0,
            pause_counted: false,
            runaway_pause_count: 0,
            longest_pause_ms: 0.0,
        }
    }

    /// 喂入一段 16k 采样与该段的 VAD 语音判定（只读采样，不反向影响任何行为）。
    /// 时长由采样数推出：chunk_ms = len / sample_rate。
    pub fn push_chunk(&mut self, samples: &[f32], speech: bool) {
        if samples.is_empty() {
            return;
        }
        let chunk_ms = samples.len() as f64 / self.sample_rate as f64 * 1000.0;
        if speech {
            self.speech_seen = true;
            self.silence_ms = 0.0;
            self.pause_counted = false;
            let sq: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
            // 基线校准：前 3 秒有效语音（跨过 3s 边界的整块计入）
            if self.baseline.is_none() && self.calib_samples < self.calib_target_samples {
                self.calib_sq_sum += sq;
                self.calib_samples += samples.len();
                if self.calib_samples >= self.calib_target_samples {
                    self.baseline =
                        Some((self.calib_sq_sum / self.calib_samples as f64).sqrt());
                }
            }
            self.sent_sq_sum += sq;
            self.sent_samples += samples.len();
        } else if self.speech_seen {
            // 会话进行中的静音段才计停顿；开口前的沉默不算
            self.silence_ms += chunk_ms;
            if self.silence_ms > RUNAWAY_PAUSE_MS {
                if !self.pause_counted {
                    self.pause_counted = true;
                    self.runaway_pause_count += 1;
                }
                self.longest_pause_ms = self.longest_pause_ms.max(self.silence_ms);
            }
        }
    }

    /// 在 VAD 断句边界结算当前句能量（空桶或未完成基线校准时为 no-op）
    pub fn close_sentence(&mut self) {
        if let (Some(baseline), false) = (self.baseline, self.sent_samples == 0) {
            let rms = (self.sent_sq_sum / self.sent_samples as f64).sqrt();
            if rms > 0.0 && baseline > 0.0 {
                self.sentence_db.push(round2(20.0 * (rms / baseline).log10()));
            }
        }
        self.sent_sq_sum = 0.0;
        self.sent_samples = 0;
    }

    pub fn metrics(&self) -> VoiceMetrics {
        VoiceMetrics {
            baseline_calibrated: self.baseline.is_some(),
            volume_dynamic_range_db: dynamic_range_db(&self.sentence_db),
            energy_stability: energy_stability(&self.sentence_db),
            runaway_pause_count: self.runaway_pause_count,
            longest_pause_ms: self.longest_pause_ms.round() as u64,
        }
    }

    /// 报告层用的完整数据（聚合 + 各句 dB 序列）
    pub fn report(&self) -> VoiceReport {
        VoiceReport { metrics: self.metrics(), sentence_db: self.sentence_db.clone() }
    }
}

// ---------------------------------------------------------------------------
// 报告层 JSON（snake_case 键，嵌入 user 消息的 stats.voice）
// ---------------------------------------------------------------------------

fn non_whitespace_chars(text: &str) -> u64 {
    text.chars().filter(|c| !c.is_whitespace()).count() as u64
}

/// 组报告用的 voice 对象（字段缺失 = 未知）：
/// - pause_count / longest_pause_sec：失控停顿
/// - volume_dynamic_range_db / energy_stability：会话内相对值
/// - speech_rate_first_half / second_half：按句中点对半分的语速（时间戳缺失时省略）
/// - volume_db_first_half / second_half：按句序对半分的相对音量均值
/// 无任何声音信号（纯打字生成报告等）→ None（整体省略）
pub fn build_voice_json(v: &VoiceReport, sentences: &[Sentence], duration_ms: u64) -> Option<Value> {
    if !v.metrics.baseline_calibrated
        && v.metrics.runaway_pause_count == 0
        && v.sentence_db.is_empty()
    {
        return None;
    }
    let mut voice = json!({
        "pause_count": v.metrics.runaway_pause_count,
        "longest_pause_sec": round1(v.metrics.longest_pause_ms as f64 / 1000.0),
    });
    if let Some(dr) = v.metrics.volume_dynamic_range_db {
        voice["volume_dynamic_range_db"] = json!(dr);
    }
    if let Some(st) = v.metrics.energy_stability {
        voice["energy_stability"] = json!(st);
    }
    // 音量对半（按句序）
    if v.sentence_db.len() >= 2 {
        let half = v.sentence_db.len() / 2;
        let avg = |xs: &[f64]| round1(xs.iter().sum::<f64>() / xs.len() as f64);
        voice["volume_db_first_half"] = json!(avg(&v.sentence_db[..half]));
        voice["volume_db_second_half"] = json!(avg(&v.sentence_db[half..]));
    }
    // 语速对半（按句中点归属；修正稿时间戳为 0 时省略）
    let has_ts = sentences.iter().any(|s| s.end_ms > 0);
    if duration_ms >= 2_000 && sentences.len() >= 2 && has_ts {
        let midpoint = duration_ms as f64 / 2.0;
        let mut chars: [u64; 2] = [0, 0];
        for s in sentences {
            let mid = (s.start_ms + s.end_ms) as f64 / 2.0;
            let idx = if mid <= midpoint { 0 } else { 1 };
            chars[idx] += non_whitespace_chars(&s.text);
        }
        let half_min = duration_ms as f64 / 2.0 / 60_000.0;
        voice["speech_rate_first_half"] = json!(round1(chars[0] as f64 / half_min));
        voice["speech_rate_second_half"] = json!(round1(chars[1] as f64 / half_min));
    }
    Some(voice)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成 seconds 秒、幅度 amp 的恒定电平"语音"块（100ms 一块，模拟真实喂法）
    fn tone(amp: f32, seconds: f64) -> Vec<Vec<f32>> {
        let total = (16_000.0 * seconds) as usize;
        (0..total / 1_600)
            .map(|_| vec![amp; 1_600])
            .collect()
    }

    fn silence(seconds: f64) -> Vec<Vec<f32>> {
        tone(0.0, seconds)
    }

    fn feed(an: &mut VoiceAnalyzer, chunks: &[Vec<f32>], speech: bool) {
        for c in chunks {
            an.push_chunk(c, speech);
        }
    }

    #[test]
    fn baseline_calibrates_from_first_three_seconds_of_speech() {
        let mut an = VoiceAnalyzer::new(16_000);
        // 不足 3 秒不校准
        feed(&mut an, &tone(0.5, 2.9), true);
        assert!(!an.metrics().baseline_calibrated);
        // 补到 3 秒 → 校准完成；恒定幅度 0.5 → RMS = 0.5
        feed(&mut an, &tone(0.5, 0.2), true);
        assert!(an.metrics().baseline_calibrated);
        // 之后更响的语音不改变基线：校准那句 0 dB，1.0 幅度的一句 → +6.02 dB
        an.close_sentence();
        feed(&mut an, &tone(1.0, 1.0), true);
        an.close_sentence();
        feed(&mut an, &tone(0.25, 1.0), true); // 半幅 → −6.02 dB
        an.close_sentence();
        assert_eq!(an.report().sentence_db, vec![0.0, 6.02, -6.02]);
    }

    #[test]
    fn calibration_chunk_straddling_boundary_is_included() {
        let mut an = VoiceAnalyzer::new(16_000);
        // 2.5 秒 @0.1 + 0.9 秒 @0.9：校准在第 3.0 秒完成，跨界的 0.9 幅度块计入基线
        // 基线 RMS = sqrt((40000×0.01 + 8000×0.81)/48000) = sqrt(0.143333) ≈ 0.37859
        feed(&mut an, &tone(0.1, 2.5), true);
        feed(&mut an, &tone(0.9, 0.9), true);
        an.close_sentence(); // 结掉校准句，后续句能量独立结算
        assert!(an.metrics().baseline_calibrated);
        // 后续同幅 0.1 语音的一句 → 20·log10(0.1/0.37859) ≈ −11.56 dB
        feed(&mut an, &tone(0.1, 1.0), true);
        an.close_sentence();
        let series = an.report().sentence_db;
        assert_eq!(series.len(), 2);
        assert_eq!(series[1], -11.56);
    }

    #[test]
    fn silence_chunks_do_not_advance_calibration_or_sentence_energy() {
        let mut an = VoiceAnalyzer::new(16_000);
        feed(&mut an, &tone(0.5, 2.0), true);
        feed(&mut an, &silence(2.0), false); // 未满 3 秒就沉默
        feed(&mut an, &tone(0.5, 1.0), true);
        // 3 秒有效语音凑齐才校准
        assert!(an.metrics().baseline_calibrated);
    }

    #[test]
    fn percentile_interpolates_on_sorted_slice() {
        assert_eq!(percentile(&[0.0, 10.0], 10.0), 1.0);
        assert_eq!(percentile(&[0.0, 10.0], 90.0), 9.0);
        assert_eq!(percentile(&[-10.0, -10.0, 0.0, 0.0], 10.0), -10.0);
        assert_eq!(percentile(&[-10.0, -10.0, 0.0, 0.0], 90.0), 0.0);
        assert_eq!(percentile(&[5.0], 50.0), 5.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
    }

    #[test]
    fn dynamic_range_and_stability_gates() {
        assert_eq!(dynamic_range_db(&[]), None);
        assert_eq!(dynamic_range_db(&[3.0]), None);
        assert_eq!(energy_stability(&[3.0]), None);
        assert_eq!(dynamic_range_db(&[-10.0, -10.0, 0.0, 0.0]), Some(10.0));
        assert_eq!(dynamic_range_db(&[0.0, 10.0]), Some(8.0));
    }

    #[test]
    fn energy_stability_zero_for_flat_and_high_for_jumpy() {
        assert_eq!(energy_stability(&[0.0, 0.0, 0.0, 0.0]), Some(0.0));
        // 差分序列 [10,-10,10,-10]：均值 0，方差 100
        assert_eq!(energy_stability(&[0.0, 10.0, 0.0, 10.0, 0.0]), Some(100.0));
    }

    #[test]
    fn runaway_pauses_counted_only_after_speech_and_strictly_over_2s() {
        let mut an = VoiceAnalyzer::new(16_000);
        // 开口前的 5 秒沉默不算
        feed(&mut an, &silence(5.0), false);
        assert_eq!(an.metrics().runaway_pause_count, 0);
        // 说 0.5 秒，然后静音恰好 2.0 秒 → 不计（需严格大于）
        feed(&mut an, &tone(0.5, 0.5), true);
        feed(&mut an, &silence(2.0), false);
        let m = an.metrics();
        assert_eq!(m.runaway_pause_count, 0);
        assert_eq!(m.longest_pause_ms, 0);
        // 再多 100ms 静音 → 计 1 次，最长 2100ms
        feed(&mut an, &silence(0.1), false);
        let m = an.metrics();
        assert_eq!(m.runaway_pause_count, 1);
        assert_eq!(m.longest_pause_ms, 2_100);
        // 恢复说话后再静音 1.5 秒 → 不新增
        feed(&mut an, &tone(0.5, 0.1), true);
        feed(&mut an, &silence(1.5), false);
        assert_eq!(an.metrics().runaway_pause_count, 1);
        // 再来 3 秒静音 → 第 2 次，最长 3000ms
        feed(&mut an, &tone(0.5, 0.1), true);
        feed(&mut an, &silence(3.0), false);
        let m = an.metrics();
        assert_eq!(m.runaway_pause_count, 2);
        assert_eq!(m.longest_pause_ms, 3_000);
    }

    #[test]
    fn silence_when_speech_never_started_is_ignored() {
        let mut an = VoiceAnalyzer::new(16_000);
        feed(&mut an, &silence(10.0), false);
        let m = an.metrics();
        assert_eq!(m.runaway_pause_count, 0);
        assert_eq!(m.longest_pause_ms, 0);
        assert!(!m.baseline_calibrated);
    }

    #[test]
    fn close_sentence_on_empty_bucket_or_without_baseline_is_noop() {
        let mut an = VoiceAnalyzer::new(16_000);
        an.close_sentence(); // 未校准 + 空桶
        feed(&mut an, &silence(1.0), false);
        an.close_sentence(); // 静音块不进桶
        assert!(an.report().sentence_db.is_empty());
    }

    #[test]
    fn metrics_default_to_empty_for_untouched_analyzer() {
        let an = VoiceAnalyzer::new(16_000);
        assert_eq!(
            an.metrics(),
            VoiceMetrics {
                baseline_calibrated: false,
                volume_dynamic_range_db: None,
                energy_stability: None,
                runaway_pause_count: 0,
                longest_pause_ms: 0,
            }
        );
    }

    fn sent(id: u64, text: &str, start_ms: u64, end_ms: u64) -> Sentence {
        Sentence { id, text: text.into(), start_ms, end_ms }
    }

    #[test]
    fn voice_json_omitted_without_any_voice_signal() {
        assert!(build_voice_json(&VoiceReport::default(), &[], 0).is_none());
    }

    #[test]
    fn voice_json_includes_halves_and_metrics() {
        let mut an = VoiceAnalyzer::new(16_000);
        feed(&mut an, &tone(0.5, 2.9), true); // 校准句（凑满 3 秒有效语音）
        feed(&mut an, &tone(0.5, 0.1), true);
        an.close_sentence(); // → 0 dB
        feed(&mut an, &tone(0.5, 1.0), true);
        an.close_sentence(); // → 0 dB
        feed(&mut an, &tone(0.25, 1.0), true);
        an.close_sentence(); // → −6.02
        feed(&mut an, &tone(0.25, 1.0), true);
        an.close_sentence(); // → −6.02
        let report = an.report();
        // 序列 [0, 0, -6.02, -6.02]：音量对半 → 前半 0 / 后半 −6.0
        let sentences = vec![
            sent(1, "前半句十个字整", 0, 60_000),
            sent(2, "前半又十个字整", 60_000, 120_000),
            sent(3, "后半句十个字整十", 120_000, 180_000),
            sent(4, "后半又十个字整十", 180_000, 240_000),
        ];
        let v = build_voice_json(&report, &sentences, 240_000).unwrap();
        assert_eq!(v["pause_count"], 0);
        assert_eq!(v["longest_pause_sec"], 0.0);
        assert_eq!(v["volume_db_first_half"], 0.0);
        assert_eq!(v["volume_db_second_half"], -6.0);
        // 前半 2 句各 7 字 → 14 字/2 分钟 = 7；后半各 8 字 → 16/2 = 8
        assert_eq!(v["speech_rate_first_half"], 7.0);
        assert_eq!(v["speech_rate_second_half"], 8.0);
        assert_eq!(v["volume_dynamic_range_db"], 6.0);
    }

    #[test]
    fn voice_json_skips_speech_rate_halves_without_timestamps() {
        let mut an = VoiceAnalyzer::new(16_000);
        feed(&mut an, &tone(0.5, 3.0), true);
        an.close_sentence();
        feed(&mut an, &tone(0.5, 1.0), true);
        an.close_sentence();
        feed(&mut an, &tone(0.25, 1.0), true);
        an.close_sentence();
        let report = an.report();
        // 修正稿：时间戳全 0
        let sentences = vec![sent(1, "一句话", 0, 0), sent(2, "两句话", 0, 0)];
        let v = build_voice_json(&report, &sentences, 0).unwrap();
        assert!(v.get("speech_rate_first_half").is_none());
        assert!(v.get("speech_rate_second_half").is_none());
        // 2 句仍可给音量对半
        assert!(v.get("volume_db_first_half").is_some());
    }

    #[test]
    fn voice_metrics_serialize_camel_case() {
        let m = VoiceMetrics {
            baseline_calibrated: true,
            volume_dynamic_range_db: Some(8.4),
            energy_stability: Some(1.2),
            runaway_pause_count: 2,
            longest_pause_ms: 3_100,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["baselineCalibrated"], true);
        assert_eq!(v["volumeDynamicRangeDb"], 8.4);
        assert_eq!(v["energyStability"], 1.2);
        assert_eq!(v["runawayPauseCount"], 2);
        assert_eq!(v["longestPauseMs"], 3_100);
    }

    #[test]
    fn pauses_without_calibration_still_reported() {
        // 只说话 1 秒（未满基线）+ 3 秒静音：voice 对象仍应给出停顿
        let mut an = VoiceAnalyzer::new(16_000);
        feed(&mut an, &tone(0.5, 1.0), true);
        feed(&mut an, &silence(3.0), false);
        let report = an.report();
        let v = build_voice_json(&report, &[], 4_000).unwrap();
        assert_eq!(v["pause_count"], 1);
        assert_eq!(v["longest_pause_sec"], 3.0);
        assert!(v.get("volume_dynamic_range_db").is_none());
    }
}
