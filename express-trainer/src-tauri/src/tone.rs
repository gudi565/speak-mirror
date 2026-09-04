//! 普通话声调偏差检查（v0：纯 DSP + 词典启发式，不训练模型）。
//!
//! 产品定位：声韵调级评分在开源世界无成品、商业云只做 B2B——这是差异化空白。
//! v0 只做「会话结束后的声调偏差提示」：对录音逐句做基音（F0）轨迹分析，
//! 按能量包络粗切音节，每音节轮廓归为五类声调形状（高平/升/降升/降/短轻），
//! 与内嵌词典的期望声调比对，高置信的偏差才标记——接总结页逐句回放，
//! 形成「看提示 → 点播放对照」的闭环。
//!
//! 置信门控策略（宁缺毋滥，误报是声调提示的致命伤）：
//! 1. 音节数门控：能量峰估计的音节数与句内汉字数误差 >30% → 整句跳过；
//! 2. 逐音节门控：浊音帧不足 4 帧（<40ms）、形状落在判定边界附近（置信 <0.6）不判；
//! 3. 整句门控：可分析音节占比 <60% → 全句放弃；
//! 4. 轻声（5）与「短轻」形状只参与形状集合，绝不作为偏差依据；
//! 5. 多音字任一读音匹配即通过（「行」的升/降都不算错）；
//! 6. 每句最多标 3 个（防刷屏，与规则引擎冷却同一纪律）。
//!
//! 拼音声调数据：内嵌 `lexicon/pinyin-tones.tsv`（mozillazg/pinyin-data，MIT；
//! 常用 6000 字按 jieba 词频聚合排序筛出，多音字全列、主读音在前）。
//! 纯函数 + 单测为主；线程接入见 lib.rs 的 spawn_tone_analysis。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// 词典（内嵌 TSV：字<TAB>pin1|pin2…，声调用数字 1-5，轻声 5）
// ---------------------------------------------------------------------------

pub const TONE_TSV: &str = include_str!("../lexicon/pinyin-tones.tsv");

/// 汉字 → 声调集合（文件顺序，首个 1–4 声视为主读音）。启动后首次访问解析入缓存。
pub fn tone_table() -> &'static HashMap<char, Vec<u8>> {
    static TABLE: OnceLock<HashMap<char, Vec<u8>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = HashMap::new();
        for line in TONE_TSV.lines() {
            let Some((ch_field, reads_field)) = line.split_once('\t') else { continue };
            let Some(ch) = ch_field.chars().next() else { continue };
            let mut tones: Vec<u8> = Vec::new();
            for read in reads_field.split('|') {
                let Some(digit) = read.as_bytes().last() else { continue };
                let tone = digit - b'0';
                if (1..=5).contains(&tone) && !tones.contains(&tone) {
                    tones.push(tone);
                }
            }
            if !tones.is_empty() {
                map.insert(ch, tones);
            }
        }
        map
    })
}

/// 是否基本区汉字（与词表覆盖范围一致：U+4E00–U+9FFF）
pub fn is_han(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

/// 声调数字 → 中文（报告与前端文案同口径）
pub fn tone_number_cn(tone: u8) -> &'static str {
    match tone {
        1 => "一",
        2 => "二",
        3 => "三",
        4 => "四",
        _ => "轻",
    }
}

/// 检测形状 → 中文听感描述
pub fn shape_label_cn(shape: u8) -> &'static str {
    match shape {
        1 => "高平",
        2 => "升调",
        3 => "降升",
        4 => "降调",
        _ => "短轻",
    }
}

// ---------------------------------------------------------------------------
// F0 估计（自相关法：帧 25ms / 步长 10ms，搜索 60–400Hz，能量阈值跳过清音帧）
// ---------------------------------------------------------------------------

pub const FRAME_MS: usize = 25;
pub const HOP_MS: usize = 10;
pub const F0_MIN_HZ: f64 = 60.0;
pub const F0_MAX_HZ: f64 = 400.0;
/// 归一化自相关峰值（清晰度）低于该值视为非周期（清音/噪声）帧
pub const VOICED_CLARITY_MIN: f64 = 0.45;
/// 帧能量低于句内最大帧能量该比例的帧跳过（静音/弱尾音）
pub const ENERGY_GATE_RATIO: f64 = 0.08;

fn frame_len(sample_rate: u32) -> usize {
    (sample_rate as usize * FRAME_MS).div_ceil(1000)
}

fn hop(sample_rate: u32) -> usize {
    (sample_rate as usize * HOP_MS).max(1) / 1000
}

fn rms(frame: &[f32]) -> f64 {
    (frame.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / frame.len() as f64).sqrt()
}

/// 单帧 F0：归一化自相关在 [60,400]Hz 对应 lag 范围内找峰，抛物线插值精化。
/// 轻微偏向短 lag（×(1−0.1·归一位置)），抑制倍周期（半频）误判。
/// 返回 (F0, 清晰度)；清晰度 < VOICED_CLARITY_MIN 或能量过低返回 None。
pub fn estimate_f0_frame(frame: &[f32], sample_rate: u32) -> Option<(f64, f64)> {
    let n = frame.len();
    if n < 64 {
        return None;
    }
    let mean = frame.iter().sum::<f32>() / n as f32;
    let x: Vec<f64> = frame.iter().map(|&s| (s - mean) as f64).collect();
    let lag_min = ((sample_rate as f64 / F0_MAX_HZ).floor() as usize).max(2);
    let lag_max =
        ((sample_rate as f64 / F0_MIN_HZ).ceil() as usize).min(n - 8);
    if lag_max <= lag_min + 2 {
        return None;
    }
    let energy: f64 = x.iter().map(|v| v * v).sum();
    if energy <= f64::EPSILON {
        return None;
    }
    // 逐 lag 的归一化互相关（除以两侧局部能量，抑制窗口截断偏差）
    let span = (lag_max - lag_min) as f64;
    let mut ncf = vec![0.0f64; lag_max - lag_min + 1];
    let mut best = (0usize, f64::MIN); // (lag, score)
    for lag in lag_min..=lag_max {
        let m = n - lag;
        let (mut r, mut e1, mut e2) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..m {
            r += x[i] * x[i + lag];
            e1 += x[i] * x[i];
            e2 += x[i + lag] * x[i + lag];
        }
        let denom = (e1 * e2).sqrt();
        let value = if denom > f64::EPSILON { r / denom } else { 0.0 };
        ncf[lag - lag_min] = value;
        let score = value * (1.0 - 0.10 * (lag - lag_min) as f64 / span);
        if score > best.1 {
            best = (lag, score);
        }
    }
    // 抛物线插值：峰两侧各取一点，亚样本精度（220Hz 在 16k 下 lag≈72.7）
    let lag = best.0;
    let idx = lag - lag_min;
    let clarity = ncf[idx];
    if clarity < VOICED_CLARITY_MIN {
        return None;
    }
    // 抛物线插值：峰两侧各取一点，亚样本精度（220Hz 在 16k 下 lag≈72.7）
    let left = ncf[idx.saturating_sub(1)];
    let right = ncf[(idx + 1).min(ncf.len() - 1)];
    let denom = left - 2.0 * clarity + right;
    let delta = if denom.abs() > 1e-9 {
        (0.5 * (left - right) / denom).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let lag_refined = lag as f64 + delta;
    let f0 = sample_rate as f64 / lag_refined;
    if !(F0_MIN_HZ * 0.95..=F0_MAX_HZ * 1.05).contains(&f0) {
        return None;
    }
    Some((f0, clarity))
}

/// 逐帧分析结果（10ms 步长对齐）：能量包络 + F0（None = 静音/清音帧）
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSeries {
    pub energies: Vec<f64>,
    pub f0s: Vec<Option<f64>>,
}

/// 整段逐帧分析：先算能量包络定门限（相对最大帧能量），高于门限的帧再估 F0
pub fn analyze_frames(samples: &[f32], sample_rate: u32) -> FrameSeries {
    let flen = frame_len(sample_rate);
    let step = hop(sample_rate);
    let mut energies = Vec::new();
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + flen <= samples.len() {
        energies.push(rms(&samples[i..i + flen]));
        starts.push(i);
        i += step;
    }
    let max_e = energies.iter().copied().fold(0.0f64, f64::max);
    let gate = (max_e * ENERGY_GATE_RATIO).max(1e-4);
    let f0s = starts
        .iter()
        .zip(&energies)
        .map(|(&st, &e)| {
            if e < gate {
                None
            } else {
                estimate_f0_frame(&samples[st..st + flen], sample_rate).map(|(f0, _)| f0)
            }
        })
        .collect();
    FrameSeries { energies, f0s }
}

// ---------------------------------------------------------------------------
// 形状分类（半滑窗三点轮廓 + 斜率特征，单位半音）
// ---------------------------------------------------------------------------

/// 声调形状（数值与声调 1–5 对齐）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneShape {
    /// 1 高平
    Flat = 1,
    /// 2 升
    Rising = 2,
    /// 3 降升（低）
    Dipping = 3,
    /// 4 降
    Falling = 4,
    /// 5 短轻
    Neutral = 5,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeDecision {
    pub shape: ToneShape,
    /// 置信度 0–1：距判定边界的裕度越大概率越高；<0.6 不作为偏差依据
    pub confidence: f64,
}

/// 单音节浊音帧数下限（<40ms 的音节形状不可判）
pub const MIN_VOICED_FRAMES: usize = 4;
/// 「短轻」判定的浊音帧上限（≤60ms 视为短）
pub const NEUTRAL_MAX_FRAMES: usize = 6;
/// 高平：首尾差绝对值上限（半音）
pub const ST_LEVEL_MAX: f64 = 1.2;
/// 升/降：首尾差的最小幅度（半音）
pub const ST_MOVE_MIN: f64 = 1.5;
/// 降升：前半下降幅（半音，负值）与后半回升幅（半音）阈值
pub const ST_DIP_DROP: f64 = -1.0;
pub const ST_DIP_RISE: f64 = 0.8;
/// 偏差标记的置信度下限
pub const FLAG_CONFIDENCE_MIN: f64 = 0.6;

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// 三点滑动平均（半滑窗：削弱逐帧抖动，保留整体走向）
pub fn smooth3(v: &[f64]) -> Vec<f64> {
    if v.len() < 3 {
        return v.to_vec();
    }
    (0..v.len())
        .map(|i| {
            let a = v[i.saturating_sub(1)];
            let b = v[i];
            let c = v[(i + 1).min(v.len() - 1)];
            (a + b + c) / 3.0
        })
        .collect()
}

/// 归一化 F0 轮廓 → (中点半音差, 尾点半音差)：以首点为 0，
/// 取 0% / 50% / 85% 位置（避开起收尾的过渡帧）。帧数不足返回 None。
pub fn three_point_profile(voiced_f0: &[f64]) -> Option<(f64, f64)> {
    if voiced_f0.len() < MIN_VOICED_FRAMES {
        return None;
    }
    let sm = smooth3(voiced_f0);
    let n = sm.len();
    let at = |p: f64| sm[((p * (n - 1) as f64).round() as usize).min(n - 1)];
    let start = at(0.0);
    if start <= 0.0 {
        return None;
    }
    let st = |hz: f64| 12.0 * (hz / start).log2();
    Some((st(at(0.5)), st(at(0.85))))
}

/// 声调形状分类（纯函数）。d_mid/d_end 为中点/尾点相对首点的半音差；
/// voiced_frames 为该音节的浊音帧数（短音节 → 短轻）。
/// 判定顺序：短轻 → 降升 → 升/降 → 高平；都不沾边返回 None（不判）。
pub fn classify_shape(d_mid: f64, d_end: f64, voiced_frames: usize) -> Option<ShapeDecision> {
    // 短轻：音节过短，形状不可判（置信低，仅用于占满五类枚举，不作偏差依据）
    if voiced_frames <= NEUTRAL_MAX_FRAMES {
        return Some(ShapeDecision { shape: ToneShape::Neutral, confidence: 0.5 });
    }
    // 降升（3）：先明显下降、再回升
    if d_mid <= ST_DIP_DROP && (d_end - d_mid) >= ST_DIP_RISE {
        let strength = (-d_mid) + (d_end - d_mid);
        return Some(ShapeDecision {
            shape: ToneShape::Dipping,
            confidence: clamp01(0.5 + (strength + ST_DIP_DROP - ST_DIP_RISE) / 3.0),
        });
    }
    // 升（2）：整体上行且前半没有明显下探
    if d_end >= ST_MOVE_MIN && d_mid > ST_DIP_DROP {
        return Some(ShapeDecision {
            shape: ToneShape::Rising,
            confidence: clamp01(0.5 + (d_end - ST_MOVE_MIN) / 3.0),
        });
    }
    // 降（4）：整体下行且前半没有明显上冲
    if d_end <= -ST_MOVE_MIN && d_mid < -ST_DIP_DROP {
        return Some(ShapeDecision {
            shape: ToneShape::Falling,
            confidence: clamp01(0.5 + (-d_end - ST_MOVE_MIN) / 3.0),
        });
    }
    // 高平（1）：首尾与中段都基本持平（中段明显偏移的「峰形/谷形」不判）
    if d_end.abs() <= ST_LEVEL_MAX && d_mid.abs() <= ST_LEVEL_MAX {
        return Some(ShapeDecision {
            shape: ToneShape::Flat,
            confidence: clamp01(0.5 + (ST_LEVEL_MAX - d_end.abs()) / 2.4),
        });
    }
    None
}

/// 词典声调 vs 检测形状的偏差判定（纯函数）：置信不足/短轻/轻声读音不标；
/// 多音字任一 1–4 声读音匹配形状即通过；不匹配时返回主读音（供展示）。
pub fn tone_mismatch(tones: &[u8], shape: u8, confidence: f64) -> Option<u8> {
    if confidence < FLAG_CONFIDENCE_MIN || shape == 5 {
        return None;
    }
    let full: Vec<u8> = tones.iter().copied().filter(|t| (1..=4).contains(t)).collect();
    if full.is_empty() {
        return None; // 只有轻声读音（语气词等）：不标
    }
    if full.contains(&shape) {
        return None; // 任一读音匹配 → 通过
    }
    Some(full[0])
}

// ---------------------------------------------------------------------------
// 音节切分 v0（能量包络峰 + 谷点分段，粗粒度置信门控）
// ---------------------------------------------------------------------------

/// 能量峰进入阈值（相对平滑后最大帧能量的比例）
pub const PEAK_RATIO: f64 = 0.25;
/// 音节「起伏」闭合阈值：能量回落到当前峰值的该比例以下才结束一个音节。
/// 用滞回而非单纯局部极大：纯正弦的帧 RMS 有 ±3% 的相位纹波，
/// 局部极大会数出数倍假峰；滞回只认「起—落」完整的能量起伏
pub const PEAK_EXIT_RATIO: f64 = 0.6;

/// 音节数门控（纯函数）：能量峰估计的音节数与汉字数误差 >30% → 整句跳过
pub fn syllable_gate(estimated: usize, text_count: usize) -> bool {
    if text_count == 0 || estimated == 0 {
        return false;
    }
    let err = (estimated as f64 - text_count as f64).abs() / text_count as f64;
    err <= 0.30
}

/// 平滑能量包络上的音节起伏位置（施密特触发计数）：能量越过 0.25·max
/// 开峰、回落到 0.6·峰值以下闭峰；闭峰后必须先落回开峰阈值以下才能再开
/// （缓慢衰减的尾巴不会自成一峰），峰内反弹只更新峰位不另开峰。
/// 返回每个起伏的峰值帧下标。
pub fn energy_peak_positions(energies: &[f64]) -> Vec<usize> {
    if energies.len() < 3 {
        return Vec::new();
    }
    let s = smooth3(energies);
    let max = s.iter().copied().fold(0.0f64, f64::max);
    if max <= 0.0 {
        return Vec::new();
    }
    let enter = max * PEAK_RATIO;
    // 施密特触发三态：0 静默等待开峰；1 峰内（跟踪最高帧）；
    // 2 已闭峰、等待回落到 enter 以下（期间反弹视作同一峰的延续）
    let mut state = 0u8;
    let mut peak_i = 0usize;
    let mut peak_v = 0.0f64;
    let mut peaks: Vec<usize> = Vec::new();
    for (i, &e) in s.iter().enumerate() {
        match state {
            0 => {
                if e >= enter {
                    state = 1;
                    peak_i = i;
                    peak_v = e;
                }
            }
            1 => {
                if e > peak_v {
                    peak_i = i;
                    peak_v = e;
                } else if e < peak_v * PEAK_EXIT_RATIO {
                    peaks.push(peak_i);
                    state = 2;
                }
            }
            _ => {
                if e > peak_v {
                    state = 1; // 未回落到底的反弹：延续同一峰
                    peak_i = i;
                    peak_v = e;
                } else if e < enter {
                    state = 0;
                }
            }
        }
    }
    if state == 1 {
        peaks.push(peak_i); // 尾部未回落的最后一个音节
    }
    peaks
}

/// 相邻切分谷的最小间隔（帧）：短于该间隔的两个谷不可能分属两个音节
pub const VALLEY_MIN_SEPARATION_FRAMES: usize = 8;

/// 按汉字数切音节边界（纯函数）：取能量谷（局部极小且显著低于峰值）中
/// 最深的 segments−1 个作边界，不足时用均匀切分补齐。返回升序内部边界帧下标。
pub fn segment_boundaries(energies: &[f64], segments: usize) -> Vec<usize> {
    let n = energies.len();
    if segments < 2 || n < 4 {
        return Vec::new();
    }
    let s = smooth3(energies);
    let max = s.iter().copied().fold(0.0f64, f64::max);
    // 候选谷：局部极小且低于峰值 70%（谷要有意义）
    let mut candidates: Vec<(usize, f64)> = Vec::new();
    for i in 1..n - 1 {
        if s[i] <= s[i - 1] && s[i] <= s[i + 1] && s[i] < max * 0.7 {
            candidates.push((i, s[i]));
        }
    }
    // 最深的谷优先（值最小者排前），间隔与两端约束
    candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let mut chosen: Vec<usize> = Vec::new();
    for (idx, _) in candidates {
        if chosen.len() >= segments - 1 {
            break;
        }
        if idx < 3 || idx > n - 4 {
            continue;
        }
        if chosen.iter().any(|&c| (c as i64 - idx as i64).abs() < VALLEY_MIN_SEPARATION_FRAMES as i64) {
            continue;
        }
        chosen.push(idx);
    }
    // 谷不足（连读无明显谷）→ 均匀切分补齐
    for k in 1..segments {
        if chosen.len() >= segments - 1 {
            break;
        }
        let pos = n * k / segments;
        if pos >= 3 && pos <= n - 4 && !chosen.iter().any(|&c| (c as i64 - pos as i64).abs() < 4) {
            chosen.push(pos);
        }
    }
    chosen.sort_unstable();
    chosen
}

// ---------------------------------------------------------------------------
// 句级分析入口
// ---------------------------------------------------------------------------

/// 一条声调偏差标记（快照 toneFlags / tone_update 事件 / 前端声调提示面板共用）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToneFlag {
    /// 所属句子 id（与 transcript 的 Sentence.id 对齐，前端据此定位回放）
    pub sentence_id: u64,
    /// 音节在句内汉字序列中的下标（0 起）
    pub char_index: u32,
    /// 疑似读错的字
    pub char: String,
    /// 词典期望声调（主读音；1–4）
    pub expected_tone: u8,
    /// 检测到的形状（1 高平 / 2 升 / 3 降升 / 4 降 / 5 短轻）
    pub detected_shape: u8,
}

/// 每句最多标记数（防刷屏；与规则引擎「宁可漏报」同一纪律）
pub const MAX_FLAGS_PER_SENTENCE: usize = 3;
/// 整句置信下限：可分析音节占比低于该比例 → 全句放弃
pub const MIN_ANALYZED_RATIO: f64 = 0.6;
/// 参与分析的最短句音频时长（秒）
pub const MIN_SENTENCE_SEC: f64 = 0.25;

/// 句级声调检查（纯函数）：文本 → 词典声调序列，音频 → 逐音节形状，
/// 高置信偏差才标记。sentence_id 置 0，由 analyze_session/调用方盖印。
///
/// 跳过条件（返回空）：无汉字 / 缺字（词表未收录）/ 音频过短 / 无能量峰 /
/// 音节数门控不过 / 可分析音节占比 <60%。最多返回 3 条标记。
pub fn check_sentence(text: &str, samples: &[f32], sample_rate: u32) -> Vec<ToneFlag> {
    let chars: Vec<char> = text.chars().filter(|c| is_han(*c)).collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let table = tone_table();
    if chars.iter().any(|c| !table.contains_key(c)) {
        return Vec::new(); // 缺字（超出常用表）：整句跳过，宁缺毋滥
    }
    if (samples.len() as f64) < MIN_SENTENCE_SEC * sample_rate as f64 {
        return Vec::new();
    }
    let series = analyze_frames(samples, sample_rate);
    if series.energies.len() < 3 {
        return Vec::new();
    }
    let peaks = energy_peak_positions(&series.energies);
    if peaks.is_empty() || !syllable_gate(peaks.len(), chars.len()) {
        return Vec::new();
    }
    // 边界：0 = boundaries... = 帧数，切成 chars.len() 段
    let mut edges: Vec<usize> = vec![0];
    edges.extend(segment_boundaries(&series.energies, chars.len()));
    edges.push(series.f0s.len());
    let mut flags = Vec::new();
    let mut analyzed = 0usize;
    for (i, ch) in chars.iter().enumerate() {
        let Some(seg_end) = edges.get(i + 1) else { break };
        let voiced: Vec<f64> = series.f0s[edges[i]..*seg_end].iter().filter_map(|f| *f).collect();
        let Some((d_mid, d_end)) = three_point_profile(&voiced) else {
            continue;
        };
        let Some(decision) = classify_shape(d_mid, d_end, voiced.len()) else {
            continue;
        };
        if decision.shape == ToneShape::Neutral {
            continue; // 短轻不算「可分析」，也不作偏差依据
        }
        analyzed += 1;
        if let Some(expected) =
            tone_mismatch(&table[ch], decision.shape as u8, decision.confidence)
        {
            flags.push(ToneFlag {
                sentence_id: 0,
                char_index: i as u32,
                char: ch.to_string(),
                expected_tone: expected,
                detected_shape: decision.shape as u8,
            });
        }
    }
    // 整句置信度：可分析音节占比不足 → 全句放弃
    if (analyzed as f64) < MIN_ANALYZED_RATIO * chars.len() as f64 {
        return Vec::new();
    }
    flags.truncate(MAX_FLAGS_PER_SENTENCE);
    flags
}

/// 会话级分析（纯函数）：整段 16k 单声道录音 + 终稿句列表 → 带 sentence_id
/// 的标记。逐句按 start_ms/end_ms 切片，切片越界自动夹到音频末尾。
pub fn analyze_session(
    sentences: &[crate::rules::Sentence],
    samples: &[f32],
    sample_rate: u32,
) -> Vec<ToneFlag> {
    let sr = sample_rate as usize;
    let mut out = Vec::new();
    for s in sentences {
        let start = ((s.start_ms as usize) * sr / 1000).min(samples.len());
        let end = (((s.end_ms as usize) * sr / 1000).max(start + 1)).min(samples.len());
        for mut flag in check_sentence(&s.text, &samples[start..end], sample_rate) {
            flag.sentence_id = s.id;
            out.push(flag);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 单测：合成正弦（固定 = 平、升频、降频、降升）验证 F0 与形状分类；
// 门控边界、查表（多音字/缺字）、标记上限、快照兼容。
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 16_000;

    /// 线性变頻正弦（起/收音包络，避免边界能量突变造成伪峰）
    fn synth_chirp(f0_start: f64, f0_end: f64, dur_sec: f64) -> Vec<f32> {
        synth_chirp_amp(0.6, f0_start, f0_end, dur_sec)
    }

    /// 降升（先降后升）：相位连续的单音节（中点不衰减——能量起伏不能断，
    /// 否则会被当成两个音节）
    fn synth_dipping(f0_high: f64, f0_low: f64, dur_sec: f64) -> Vec<f32> {
        let n = (dur_sec * SR as f64) as usize;
        let mut out = Vec::with_capacity(n);
        let mut phase = 0.0f64;
        for i in 0..n {
            let t = i as f64 / n as f64;
            let f = if t < 0.5 {
                f0_high + (f0_low - f0_high) * (t / 0.5)
            } else {
                f0_low + (f0_high * 1.15 - f0_low) * ((t - 0.5) / 0.5)
            };
            phase += 2.0 * std::f64::consts::PI * f / SR as f64;
            let attack = (i as f64 / (0.02 * SR as f64)).min(1.0);
            let decay = ((n - i) as f64 / (0.03 * SR as f64)).min(1.0);
            let env = attack.min(decay);
            out.push((0.6 * env * phase.sin()) as f32);
        }
        out
    }

    /// 音节序列以 60ms 静音间隔拼接
    fn join_syllables(parts: &[Vec<f32>]) -> Vec<f32> {
        let gap = vec![0.0f32; SR as usize * 60 / 1000];
        let mut out = Vec::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                out.extend_from_slice(&gap);
            }
            out.extend_from_slice(p);
        }
        out
    }

    /// 确定性白噪声脉冲（同包络）：帧能量包络是一个规整起伏（可过音节峰
    /// 门控），但自相关无周期峰（清晰度 < 0.45 → 无 F0）→ 不可分析音节
    fn noise_burst(dur_sec: f64) -> Vec<f32> {
        let n = (dur_sec * SR as f64) as usize;
        let mut state = 12345u64;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let noise = ((state >> 33) as f64 / (u32::MAX as f64) - 0.5) * 2.0;
            let attack = (i as f64 / (0.02 * SR as f64)).min(1.0);
            let decay = ((n - i) as f64 / (0.03 * SR as f64)).min(1.0);
            let env = attack.min(decay);
            out.push((0.6 * env * noise) as f32);
        }
        out
    }

    /// 指定振幅的变頻正弦（短音节需要更高振幅才能过能量峰阈值）
    fn synth_chirp_amp(amp: f64, f0_start: f64, f0_end: f64, dur_sec: f64) -> Vec<f32> {
        let n = (dur_sec * SR as f64) as usize;
        let mut out = Vec::with_capacity(n);
        let mut phase = 0.0f64;
        for i in 0..n {
            let t = i as f64 / n as f64;
            let f = f0_start + (f0_end - f0_start) * t;
            phase += 2.0 * std::f64::consts::PI * f / SR as f64;
            let attack = (i as f64 / (0.02 * SR as f64)).min(1.0);
            let decay = ((n - i) as f64 / (0.03 * SR as f64)).min(1.0);
            let env = attack.min(decay);
            out.push((amp * env * phase.sin()) as f32);
        }
        out
    }

    // --- F0 估计 -----------------------------------------------------------

    #[test]
    fn f0_frame_tracks_pure_tones() {
        for (f0, lo, hi) in [(220.0, 210.0, 230.0), (330.0, 318.0, 342.0), (100.0, 95.0, 105.0)] {
            let s = synth_chirp(f0, f0, 0.4);
            let frame = &s[SR as usize / 4..SR as usize / 4 + frame_len(SR)];
            let (est, clarity) = estimate_f0_frame(frame, SR).unwrap_or_else(|| {
                panic!("{f0}Hz 纯音应能估出 F0")
            });
            assert!((lo..=hi).contains(&est), "{f0}Hz 估出 {est:.1}Hz");
            assert!(clarity > 0.8, "{f0}Hz 清晰度 {clarity:.2} 应高于 0.8");
        }
    }

    #[test]
    fn f0_frame_rejects_silence() {
        let silence = vec![0.0f32; frame_len(SR)];
        assert!(estimate_f0_frame(&silence, SR).is_none());
    }

    #[test]
    fn f0_track_skips_silent_regions_and_follows_chirp() {
        let mut audio = vec![0.0f32; SR as usize / 5]; // 0.2s 静音
        audio.extend(synth_chirp(130.0, 260.0, 0.4)); // 升频
        let series = analyze_frames(&audio, SR);
        // 静音区帧（能量门限以下）无 F0
        assert!(series.f0s[5].is_none(), "静音帧不应有 F0");
        // 中段有 F0 且跟随升频：前 1/3 与后 1/3 的中位 F0 相差 >4 半音
        let voiced: Vec<f64> = series.f0s.iter().filter_map(|f| *f).collect();
        assert!(voiced.len() > 15, "浊音帧应足够多：{}", voiced.len());
        let n = voiced.len();
        let median = |v: &[f64]| {
            let mut s = v.to_vec();
            s.sort_by(|a, b| a.partial_cmp(b).unwrap());
            s[s.len() / 2]
        };
        let first = median(&voiced[..n / 3]);
        let last = median(&voiced[2 * n / 3..]);
        let st = 12.0 * (last / first).log2();
        assert!(st > 4.0, "升频前后应差 >4 半音，实测 {st:.1}");
    }

    // --- 形状分类（纯函数真值表） ------------------------------------------

    #[test]
    fn classify_shape_truth_table() {
        use ToneShape::*;
        // 高平：首尾持平
        let d = classify_shape(0.0, 0.0, 30).unwrap();
        assert_eq!(d.shape, Flat);
        assert!((d.confidence - 1.0).abs() < 1e-9);
        // 升：幅度到阈值时置信 0.5（不作为偏差依据），大幅升为 1.0
        let edge = classify_shape(0.0, ST_MOVE_MIN, 30).unwrap();
        assert_eq!(edge.shape, Rising);
        assert!((edge.confidence - 0.5).abs() < 1e-9);
        let clear = classify_shape(0.5, 3.0, 30).unwrap();
        assert_eq!(clear.shape, Rising);
        assert!((clear.confidence - 1.0).abs() < 1e-9);
        // 降
        let d = classify_shape(-1.0, -3.0, 30).unwrap();
        assert_eq!(d.shape, Falling);
        assert!(d.confidence > 0.5);
        // 降升：先降后升
        let d = classify_shape(-2.0, 1.0, 30).unwrap();
        assert_eq!(d.shape, Dipping);
        // 恰好压线的降升（-1.0 下探 + 0.8 回升）置信 0.5：不作为偏差依据
        let edge = classify_shape(ST_DIP_DROP, ST_DIP_DROP + ST_DIP_RISE, 30).unwrap();
        assert_eq!(edge.shape, Dipping);
        assert!((edge.confidence - 0.5).abs() < 1e-9);
        // 边界之间（1.2–1.5 半音的平/升过渡区）：不判
        assert!(classify_shape(0.0, 1.35, 30).is_none());
        // 先扬后抑（峰形）：不判
        assert!(classify_shape(2.0, 1.0, 30).is_none());
        // 短音节 → 短轻
        let d = classify_shape(0.0, 0.0, NEUTRAL_MAX_FRAMES).unwrap();
        assert_eq!(d.shape, Neutral);
    }

    // --- 查表 ----------------------------------------------------------------

    #[test]
    fn tone_table_loads_common_chars_with_multi_readings() {
        let t = tone_table();
        assert!(t.len() >= 5000, "常用表应有约 6000 字，实测 {}", t.len());
        // 单读音
        assert_eq!(t[&'妈'], vec![1]);
        // 多音字：主读音在前、去重
        assert!(t[&'行'].contains(&2) && t[&'行'].contains(&4), "行 {:?}", t[&'行']);
        // 轻声读音（的 → de5|di2|di4）
        assert!(t[&'的'].contains(&5) && t[&'的'].contains(&2) && t[&'的'].contains(&4));
        // 声调值全部落在 1–5
        for tones in t.values() {
            assert!(tones.iter().all(|x| (1..=5).contains(x)));
        }
    }

    #[test]
    fn is_han_covers_basic_block_only() {
        assert!(is_han('一'));
        assert!(is_han('\u{9FFF}'));
        assert!(!is_han('\u{3007}')); // 〇（兼容区）
        assert!(!is_han('\u{20BB7}')); // 扩展 B 区（表外）
        assert!(!is_han('a'));
    }

    // --- 句级端到端（合成音节） ----------------------------------------------

    #[test]
    fn check_sentence_matching_tones_produce_no_flag() {
        // 妈 mā1 = 高平；麻 má2 = 升；马 mǎ3 = 降升；骂 mà4 = 降
        for (text, synth) in [
            ("妈", synth_chirp(220.0, 220.0, 0.45)),
            ("麻", synth_chirp(130.0, 260.0, 0.45)),
            ("马", synth_dipping(220.0, 150.0, 0.5)),
            ("骂", synth_chirp(290.0, 160.0, 0.45)),
        ] {
            assert!(
                check_sentence(text, &synth, SR).is_empty(),
                "「{text}」形状与声调相符，不应标记"
            );
        }
    }

    #[test]
    fn check_sentence_flags_wrong_shape_with_expected_tone() {
        // 「妈」（应为一声高平）读成降调 → 标记：expected 1 / detected 4
        let flags = check_sentence("妈", &synth_chirp(290.0, 160.0, 0.45), SR);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].char, "妈");
        assert_eq!(flags[0].expected_tone, 1);
        assert_eq!(flags[0].detected_shape, 4);
        assert_eq!(flags[0].char_index, 0);
        // 「骂」（应为四声降）读成高平 → expected 4 / detected 1
        let flags = check_sentence("骂", &synth_chirp(220.0, 220.0, 0.45), SR);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].expected_tone, 4);
        assert_eq!(flags[0].detected_shape, 1);
    }

    #[test]
    fn check_sentence_polyphone_passes_on_any_matching_reading() {
        // 行：xing2/hang2/hang4/heng2——升与降都与某一读音相符，不标；
        // 只有降升（无三声读音）才标，期望展示主读音 2
        assert!(check_sentence("行", &synth_chirp(130.0, 260.0, 0.45), SR).is_empty());
        assert!(check_sentence("行", &synth_chirp(290.0, 160.0, 0.45), SR).is_empty());
        let flags = check_sentence("行", &synth_dipping(220.0, 150.0, 0.5), SR);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].expected_tone, 2);
        assert_eq!(flags[0].detected_shape, 3);
    }

    #[test]
    fn check_sentence_skips_empty_or_missing_chars() {
        // 无汉字 / 只有标点英文 → 空
        assert!(check_sentence("", &synth_chirp(220.0, 220.0, 0.4), SR).is_empty());
        assert!(check_sentence("OK, fine.", &synth_chirp(220.0, 220.0, 0.4), SR).is_empty());
        // 缺字（扩展 B 区汉字不在表内）→ 整句跳过
        assert!(
            check_sentence("你𠮷好", &synth_chirp(220.0, 220.0, 0.4), SR).is_empty(),
            "缺字必须整句跳过（宁缺毋滥）"
        );
        // 音频过短 → 空
        assert!(check_sentence("妈", &synth_chirp(220.0, 220.0, 0.15), SR).is_empty());
    }

    #[test]
    fn check_sentence_caps_flags_at_three() {
        // 5 个音节全部读成降调（「妈」×5 应为一声）→ 只标 3 个
        let audio = join_syllables(&[
            synth_chirp(290.0, 160.0, 0.35),
            synth_chirp(300.0, 165.0, 0.35),
            synth_chirp(280.0, 155.0, 0.35),
            synth_chirp(310.0, 170.0, 0.35),
            synth_chirp(295.0, 160.0, 0.35),
        ]);
        let flags = check_sentence("妈妈妈妈妈", &audio, SR);
        assert_eq!(flags.len(), MAX_FLAGS_PER_SENTENCE, "每句最多标 3 个");
        assert!(flags.iter().all(|f| f.char == "妈" && f.expected_tone == 1));
        // charIndex 按句内汉字下标推进
        assert_eq!(flags[0].char_index, 0);
        assert_eq!(flags[1].char_index, 1);
    }

    #[test]
    fn check_sentence_drops_whole_sentence_when_mostly_unanalyzable() {
        // 4 个能量起伏对应 4 字（音节数门控通过），其中 2 个音节是白噪声
        // （有能量峰但无周期性 → 无 F0 → 不可分析）：可分析占比 50% < 60% →
        // 全句放弃——即使那 2 个可分析音节（读成降调）本会产生标记
        let audio = join_syllables(&[
            synth_chirp(290.0, 160.0, 0.45),
            noise_burst(0.35),
            synth_chirp(290.0, 160.0, 0.45),
            noise_burst(0.35),
        ]);
        let series = analyze_frames(&audio, SR);
        assert_eq!(energy_peak_positions(&series.energies).len(), 4, "前置条件：4 峰过门控");
        let flags = check_sentence("妈妈妈妈", &audio, SR);
        assert!(flags.is_empty(), "可分析音节不足 60% 应整句放弃，实测 {flags:?}");
    }

    // --- 音节数门控边界 -------------------------------------------------------

    #[test]
    fn syllable_gate_boundary_is_thirty_percent() {
        // 误差恰 30%：通过
        assert!(syllable_gate(7, 10));
        assert!(syllable_gate(13, 10));
        // 误差 40%：跳过
        assert!(!syllable_gate(6, 10));
        assert!(!syllable_gate(14, 10));
        // 保守端：单字句只接受恰好 1 个峰
        assert!(syllable_gate(1, 1));
        assert!(!syllable_gate(2, 1));
        // 退化输入
        assert!(!syllable_gate(0, 5));
        assert!(!syllable_gate(5, 0));
    }

    #[test]
    fn syllable_gate_skips_sentence_when_audio_peaks_mismatch_chars() {
        // 文本 4 字、音频只有 1 个音节峰：误差 75% → 整句跳过
        let audio = synth_chirp(290.0, 160.0, 0.6);
        assert!(check_sentence("妈妈妈妈", &audio, SR).is_empty());
    }

    // --- 能量峰 / 切分 --------------------------------------------------------

    #[test]
    fn energy_peaks_count_separated_syllables() {
        let audio = join_syllables(&[
            synth_chirp(220.0, 220.0, 0.3),
            synth_chirp(220.0, 220.0, 0.3),
            synth_chirp(220.0, 220.0, 0.3),
        ]);
        let series = analyze_frames(&audio, SR);
        let peaks = energy_peak_positions(&series.energies);
        assert_eq!(peaks.len(), 3, "3 个隔开音节应数出 3 个能量起伏");
        // 起伏峰位单调推进，且相邻峰相隔 ≥100ms（帧距 10）
        for w in peaks.windows(2) {
            assert!(w[1] > w[0]);
            assert!(w[1] - w[0] >= 10);
        }
        // 平台纹波不再产生假峰：单音节长平台只有 1 个起伏
        let single = synth_chirp(220.0, 220.0, 0.6);
        let s2 = analyze_frames(&single, SR);
        assert_eq!(energy_peak_positions(&s2.energies).len(), 1);
    }

    #[test]
    fn segment_boundaries_pick_valleys_and_fall_back_to_even_split() {
        // 三音节：2 条内部边界，均落在谷附近（间隔 60ms 静音 → 谷位置可预期）
        let audio = join_syllables(&[
            synth_chirp(220.0, 220.0, 0.3),
            synth_chirp(220.0, 220.0, 0.3),
            synth_chirp(220.0, 220.0, 0.3),
        ]);
        let series = analyze_frames(&audio, SR);
        let bounds = segment_boundaries(&series.energies, 3);
        assert_eq!(bounds.len(), 2);
        assert!(bounds[0] < bounds[1]);
        // 无谷（连续平音）→ 均匀切分兜底：段数仍正确
        let flat = vec![0.5f64; 100];
        assert_eq!(segment_boundaries(&flat, 4).len(), 3);
        assert_eq!(segment_boundaries(&flat, 1).len(), 0);
    }

    // --- 会话级：切片、盖印、越界 --------------------------------------------

    #[test]
    fn analyze_session_slices_by_sentence_and_stamps_ids() {
        // 两句：第 1 句（id 7）「妈」读成降调；第 2 句（id 8）「妈」高平正常
        let gap = vec![0.0f32; SR as usize / 5];
        let mut audio = synth_chirp(290.0, 160.0, 0.45); // 0–0.45s
        audio.extend_from_slice(&gap);
        let second_start_ms = (audio.len() as f64 / SR as f64 * 1000.0) as u64;
        audio.extend(synth_chirp(220.0, 220.0, 0.45));
        let sentences = vec![
            crate::rules::Sentence {
                id: 7,
                text: "妈".into(),
                start_ms: 0,
                end_ms: 450,
            },
            crate::rules::Sentence {
                id: 8,
                text: "妈".into(),
                start_ms: second_start_ms,
                end_ms: second_start_ms + 450,
            },
        ];
        let flags = analyze_session(&sentences, &audio, SR);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].sentence_id, 7, "标记必须盖印所属句子 id");
    }

    #[test]
    fn analyze_session_tolerates_out_of_range_timestamps() {
        // 句子时间戳远超音频长度：夹到末尾后过短 → 跳过，不 panic
        let audio = synth_chirp(290.0, 160.0, 0.45);
        let sentences = vec![crate::rules::Sentence {
            id: 1,
            text: "妈".into(),
            start_ms: 60_000,
            end_ms: 120_000,
        }];
        assert!(analyze_session(&sentences, &audio, SR).is_empty());
    }

    // --- 序列化兼容 ------------------------------------------------------------

    #[test]
    fn tone_flag_serializes_camel_case() {
        let flag = ToneFlag {
            sentence_id: 3,
            char_index: 2,
            char: "妈".into(),
            expected_tone: 1,
            detected_shape: 4,
        };
        let v = serde_json::to_value(&flag).unwrap();
        assert_eq!(v["sentenceId"], 3);
        assert_eq!(v["charIndex"], 2);
        assert_eq!(v["char"], "妈");
        assert_eq!(v["expectedTone"], 1);
        assert_eq!(v["detectedShape"], 4);
        let back: ToneFlag = serde_json::from_value(v).unwrap();
        assert_eq!(back, flag);
    }

    #[test]
    fn snapshot_without_tone_flags_defaults_empty_for_legacy_json() {
        // 本功能之前的快照 JSON 没有 toneFlags 字段：serde default 补空
        let legacy = r#"{
            "sentenceCount": 1, "fillerCounts": [], "fillerPerMinute": 0.0,
            "emotionCounts": [], "hedgeCounts": [], "hedgeTotal": 0,
            "durationMs": 60000, "totalChars": 10, "speechRate": 10.0,
            "avgSentenceChars": 10.0
        }"#;
        let snap: crate::rules::engine::SessionSnapshot = serde_json::from_str(legacy).unwrap();
        assert!(snap.tone_flags.is_empty());
    }

    // --- 常量锁定（跟手性/防误报关键参数，防无意漂移） -------------------------

    #[test]
    fn tone_constants_are_locked() {
        assert_eq!((FRAME_MS, HOP_MS), (25, 10));
        assert_eq!((F0_MIN_HZ as u32, F0_MAX_HZ as u32), (60, 400));
        assert_eq!(MAX_FLAGS_PER_SENTENCE, 3);
        assert_eq!(MIN_VOICED_FRAMES, 4);
        assert!((FLAG_CONFIDENCE_MIN - 0.6).abs() < 1e-9);
        assert!((MIN_ANALYZED_RATIO - 0.6).abs() < 1e-9);
    }
}
