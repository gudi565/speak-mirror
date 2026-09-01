use super::emotion::EmotionRule;
use super::filler::{FillerWordsRule, DEFAULT_MEDIUM_THRESHOLD_PER_MIN};
use super::golden_quote::GoldenQuoteRule;
use super::hedge::HedgeRule;
use super::imagery::ImageryRule;
use super::lexicon::{lexicon, WordMatcher};
use super::precision::PrecisionRule;
use super::repetition::RepetitionRule;
use super::structure::{ConclusionMissingRule, ExampleMissingRule};
use super::time_vague::TimeVagueRule;
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// 引擎认得的全部规则名（设置开关的键，与各 Rule::name() 一致）
pub const ALL_RULES: &[&str] = &[
    "filler_words",
    "word_precision",
    "repetition",
    "conclusion_missing",
    "example_missing",
    "emotion_lexicon",
    "hedge",
    "time_vague",
    "imagery",
    "golden_quote",
];

/// 同类事件的默认冷却句数：某规则触发后，接下来 3 句内同类事件不再显示
/// （规则照常运行、统计照常累计，只是不再打扰用户）。
/// 口头禅例外：本来就按词计数不刷屏。
pub const DEFAULT_COOLDOWN_SENTENCES: u32 = 3;

/// 情感类别统计：次数 + 平均强度（1–9）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EmotionStat {
    pub count: u32,
    pub avg_intensity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub sentence_count: u64,
    pub filler_counts: Vec<(String, u32)>,
    pub filler_per_minute: f64,
    pub emotion_counts: Vec<(String, EmotionStat)>,
    /// 立场模糊词逐词计数（无论是否触发提醒都统计）
    pub hedge_counts: Vec<(String, u32)>,
    /// 立场模糊词总次数
    pub hedge_total: u32,
    pub duration_ms: u64,
    /// 总字数（非空白字符）
    pub total_chars: u64,
    /// 语速（字/分钟），时长未知时为 0
    pub speech_rate: f64,
    /// 平均句长（字/句），无句子时为 0
    pub avg_sentence_chars: f64,
    /// 金句候选句数（GoldenQuote 事件计数，无论是否被节流隐藏都累计）；
    /// 旧历史 JSON 缺省为 0
    #[serde(default)]
    pub golden_quote_count: u32,
    /// 声音层指标（M3）：引擎本身不产生，由会话线程在 stop/get_snapshot 时并入；
    /// 会话中的实时值走 voice_update 事件。None 时序列化省略（旧 JSON 反序列化时缺省）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub voice: Option<crate::voice::VoiceMetrics>,
}

/// 规则引擎构建配置（来自设置，会话开始时生效）
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// 启用的规则名集合（默认全部）
    pub enabled: HashSet<String>,
    /// 用户自定义口头禅（合并进口头禅规则，视同高频词）
    pub custom_fillers: Vec<String>,
    /// 中频口头禅提醒阈值（次/分钟）
    pub filler_medium_threshold_per_min: f64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            enabled: ALL_RULES.iter().map(|s| s.to_string()).collect(),
            custom_fillers: Vec::new(),
            filler_medium_threshold_per_min: DEFAULT_MEDIUM_THRESHOLD_PER_MIN,
        }
    }
}

impl EngineConfig {
    pub fn is_enabled(&self, name: &str) -> bool {
        self.enabled.contains(name)
    }
}

/// 一条规则 + 它的节流状态。`cooldown_sentences == 0` 表示豁免（口头禅）。
struct RuleSlot {
    rule: Box<dyn Rule>,
    cooldown_sentences: u32,
    /// 距离下次可显示还需经过的句数
    remaining: u32,
}

pub struct RuleEngine {
    slots: Vec<RuleSlot>,
    ctx: SessionContext,
    last_end_ms: u64,
    /// 口头禅统计扫描器（含用户自定义词；统计与规则显示解耦）
    filler_scan: WordMatcher,
    /// 立场模糊词统计扫描器（统计不受规则开关影响）
    hedge_scan: WordMatcher,
}

impl RuleEngine {
    pub fn new() -> Self {
        Self::from_config(EngineConfig::default())
    }

    /// 按设置构建规则集：只装启用的规则；口头禅豁免节流，其余默认 3 句冷却。
    pub fn from_config(config: EngineConfig) -> Self {
        let mut slots: Vec<RuleSlot> = Vec::new();
        let push = |rule: Box<dyn Rule>, cooldown: u32, slots: &mut Vec<RuleSlot>| {
            slots.push(RuleSlot { rule, cooldown_sentences: cooldown, remaining: 0 });
        };
        if config.is_enabled("filler_words") {
            push(
                Box::new(FillerWordsRule::with_lexicon(
                    &config.custom_fillers,
                    config.filler_medium_threshold_per_min,
                )),
                0, // 口头禅例外：按词计数不刷屏，不做句级冷却
                &mut slots,
            );
        }
        if config.is_enabled("word_precision") {
            push(Box::new(PrecisionRule::default()), DEFAULT_COOLDOWN_SENTENCES, &mut slots);
        }
        if config.is_enabled("repetition") {
            push(Box::new(RepetitionRule::default()), DEFAULT_COOLDOWN_SENTENCES, &mut slots);
        }
        if config.is_enabled("conclusion_missing") {
            push(Box::new(ConclusionMissingRule::default()), DEFAULT_COOLDOWN_SENTENCES, &mut slots);
        }
        if config.is_enabled("example_missing") {
            push(Box::new(ExampleMissingRule::default()), DEFAULT_COOLDOWN_SENTENCES, &mut slots);
        }
        if config.is_enabled("emotion_lexicon") {
            push(Box::new(EmotionRule::default()), DEFAULT_COOLDOWN_SENTENCES, &mut slots);
        }
        if config.is_enabled("hedge") {
            push(Box::new(HedgeRule::default()), DEFAULT_COOLDOWN_SENTENCES, &mut slots);
        }
        if config.is_enabled("time_vague") {
            push(Box::new(TimeVagueRule::default()), DEFAULT_COOLDOWN_SENTENCES, &mut slots);
        }
        if config.is_enabled("imagery") {
            push(Box::new(ImageryRule::default()), DEFAULT_COOLDOWN_SENTENCES, &mut slots);
        }
        if config.is_enabled("golden_quote") {
            push(Box::new(GoldenQuoteRule::new()), DEFAULT_COOLDOWN_SENTENCES, &mut slots);
        }

        // 统计扫描器始终构建（统计与规则开关解耦）
        let lex = lexicon();
        let mut filler_words: Vec<String> = lex
            .fillers
            .high
            .iter()
            .chain(lex.fillers.medium.iter())
            .cloned()
            .collect();
        for w in &config.custom_fillers {
            if !w.is_empty() && !filler_words.contains(w) {
                filler_words.push(w.clone());
            }
        }
        Self {
            slots,
            ctx: SessionContext::default(),
            last_end_ms: 0,
            filler_scan: WordMatcher::new(filler_words),
            hedge_scan: WordMatcher::new(lex.hedges.clone()),
        }
    }

    pub fn start(&mut self, now_ms: u64) {
        self.ctx.started_at_ms = now_ms;
    }

    pub fn ingest(&mut self, sentence: Sentence) -> Vec<FeedbackEvent> {
        let mut display = Vec::new();
        let mut produced_all: Vec<FeedbackEvent> = Vec::new();
        for slot in &mut self.slots {
            // 规则总是运行（内部状态推进，不受节流影响）
            let produced = slot.rule.on_sentence(&sentence, &self.ctx);
            let show = if slot.cooldown_sentences == 0 {
                true // 口头禅豁免：按词计数不刷屏
            } else if slot.remaining > 0 {
                slot.remaining -= 1; // 冷却中：事件丢弃（宁漏报不刷屏）
                false
            } else if !produced.is_empty() {
                slot.remaining = slot.cooldown_sentences;
                true
            } else {
                false
            };
            if show {
                display.extend(produced.iter().cloned());
            }
            produced_all.extend(produced);
        }
        // 落地统计：情感词与金句从规则事件累计；口头禅与立场模糊词由引擎扫描累计
        for e in &produced_all {
            if e.kind == FeedbackKind::Emotion {
                let cat = e.payload["category"].as_str().unwrap_or_default().to_string();
                let intensity = e.payload["intensity"].as_u64().unwrap_or(0) as u32;
                let entry = self.ctx.emotion_counts.entry(cat).or_default();
                entry.count += 1;
                entry.intensity_sum += intensity;
            }
            if e.kind == FeedbackKind::GoldenQuote {
                self.ctx.golden_quote_count += 1;
            }
        }
        for w in self.filler_scan.find_all(&sentence.text) {
            *self.ctx.filler_counts.entry(w.to_string()).or_insert(0) += 1;
        }
        for w in self.hedge_scan.find_all(&sentence.text) {
            *self.ctx.hedge_counts.entry(w.to_string()).or_insert(0) += 1;
        }
        self.last_end_ms = sentence.end_ms;
        self.ctx.sentences.push(sentence);
        display
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let total_fillers: u32 = self.ctx.filler_counts.values().sum();
        let duration_ms = self.last_end_ms.saturating_sub(self.ctx.started_at_ms);
        let minutes = duration_ms as f64 / 60_000.0;
        let filler_per_minute = if minutes > 0.0 {
            (total_fillers as f64 / minutes * 10.0).round() / 10.0
        } else {
            0.0
        };
        let round1 = |x: f64| (x * 10.0).round() / 10.0;
        let total_chars: u64 = self
            .ctx
            .sentences
            .iter()
            .flat_map(|s| s.text.chars())
            .filter(|c| !c.is_whitespace())
            .count() as u64;
        let sentence_count = self.ctx.sentences.len() as u64;
        let speech_rate = if minutes > 0.0 { round1(total_chars as f64 / minutes) } else { 0.0 };
        let avg_sentence_chars = if sentence_count > 0 {
            round1(total_chars as f64 / sentence_count as f64)
        } else {
            0.0
        };
        let sort_desc =
            |m: &std::collections::HashMap<String, u32>| -> Vec<(String, u32)> {
                let mut v: Vec<(String, u32)> = m.clone().into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1));
                v
            };
        let mut emotion_counts: Vec<(String, EmotionStat)> = self
            .ctx
            .emotion_counts
            .iter()
            .map(|(cat, acc)| {
                (
                    cat.clone(),
                    EmotionStat {
                        count: acc.count,
                        avg_intensity: if acc.count > 0 {
                            round1(acc.intensity_sum as f64 / acc.count as f64)
                        } else {
                            0.0
                        },
                    },
                )
            })
            .collect();
        emotion_counts.sort_by(|a, b| b.1.count.cmp(&a.1.count));
        SessionSnapshot {
            sentence_count,
            filler_counts: sort_desc(&self.ctx.filler_counts),
            filler_per_minute,
            emotion_counts,
            hedge_counts: sort_desc(&self.ctx.hedge_counts),
            hedge_total: self.ctx.hedge_counts.values().sum(),
            duration_ms,
            total_chars,
            speech_rate,
            avg_sentence_chars,
            golden_quote_count: self.ctx.golden_quote_count,
            voice: None,
        }
    }

    /// 全部终稿句（报告层取逐字稿用）
    pub fn sentences(&self) -> &[Sentence] {
        &self.ctx.sentences
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str, end_ms: u64) -> Sentence {
        Sentence { id, text: text.into(), start_ms: end_ms - 1000, end_ms }
    }

    fn hedge_events(events: &[FeedbackEvent]) -> usize {
        events
            .iter()
            .filter(|e| e.kind == FeedbackKind::Hedge)
            .count()
    }

    #[test]
    fn ingest_runs_all_rules_and_accumulates() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        let events = engine.ingest(sent(1, "然后我真的很开心", 60_000));
        let kinds: Vec<FeedbackKind> = events.iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&FeedbackKind::FillerWord));
        assert!(kinds.contains(&FeedbackKind::Emotion));
        let snap = engine.snapshot();
        assert_eq!(snap.sentence_count, 1);
        assert_eq!(snap.filler_counts, vec![("然后".to_string(), 1)]);
        assert_eq!(
            snap.emotion_counts,
            vec![(
                "乐".to_string(),
                EmotionStat { count: 1, avg_intensity: 5.0 }
            )]
        );
    }

    #[test]
    fn repetition_rule_sees_previous_sentences_via_ctx() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "这个系统可以实时分析你的表达问题", 60_000));
        let events = engine.ingest(sent(2, "这个系统可以实时分析你的表达问题啊", 120_000));
        assert!(events.iter().any(|e| e.kind == FeedbackKind::Repetition));
    }

    #[test]
    fn snapshot_serializes_camel_case() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "然后", 60_000));
        let v = serde_json::to_value(engine.snapshot()).unwrap();
        assert_eq!(v["sentenceCount"], 1);
        assert!(v.get("fillerPerMinute").is_some());
        assert!(v.get("totalChars").is_some());
        assert!(v.get("speechRate").is_some());
        assert!(v.get("avgSentenceChars").is_some());
        assert!(v.get("hedgeCounts").is_some());
        assert!(v.get("hedgeTotal").is_some());
        assert!(v.get("emotionCounts").is_some());
        assert!(v.get("goldenQuoteCount").is_some());
        // 引擎快照不带声音层：voice 由会话线程并入，None 时序列化省略
        assert!(v.get("voice").is_none());
    }

    #[test]
    fn snapshot_serializes_voice_when_merged() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        let mut snap = engine.snapshot();
        snap.voice = Some(crate::voice::VoiceMetrics {
            baseline_calibrated: true,
            volume_dynamic_range_db: Some(8.4),
            energy_stability: None,
            runaway_pause_count: 2,
            longest_pause_ms: 3_100,
        });
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(v["voice"]["runawayPauseCount"], 2);
        assert_eq!(v["voice"]["longestPauseMs"], 3_100);
        assert_eq!(v["voice"]["volumeDynamicRangeDb"], 8.4);
        assert!(v["voice"]["energyStability"].is_null());
    }

    #[test]
    fn snapshot_computes_chars_rate_and_avg_length() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(Sentence { id: 1, text: "今天天气好".into(), start_ms: 0, end_ms: 60_000 });
        engine.ingest(Sentence { id: 2, text: "明天天气预报说会下雨".into(), start_ms: 60_000, end_ms: 180_000 });
        let snap = engine.snapshot();
        // 时长 = 180s = 3 分钟，15 字 → 5 字/分钟；两句 → 平均 7.5 字
        assert_eq!(snap.total_chars, 15);
        assert_eq!(snap.speech_rate, 5.0);
        assert_eq!(snap.avg_sentence_chars, 7.5);
    }

    #[test]
    fn snapshot_zero_rates_before_any_sentence() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        let snap = engine.snapshot();
        assert_eq!(snap.total_chars, 0);
        assert_eq!(snap.speech_rate, 0.0);
        assert_eq!(snap.avg_sentence_chars, 0.0);
        assert!(engine.sentences().is_empty());
    }

    #[test]
    fn sentences_getter_returns_final_sentences() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "第一句", 30_000));
        engine.ingest(sent(2, "第二句", 60_000));
        let texts: Vec<&str> = engine.sentences().iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["第一句", "第二句"]);
    }

    // --- 节流（规则 UX：宁可漏报，不可刷屏） --------------------------------

    #[test]
    fn same_kind_events_cooldown_three_sentences() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        // 第 1 句触发立场模糊 → 显示
        let e1 = engine.ingest(sent(1, "可能大概是这样", 10_000));
        assert_eq!(hedge_events(&e1), 1);
        // 第 2–4 句：立场模糊规则仍会产出事件，但处于冷却被丢弃
        let e2 = engine.ingest(sent(2, "也许或许有变化", 20_000));
        assert_eq!(hedge_events(&e2), 0);
        let e3 = engine.ingest(sent(3, "好像似乎差不多", 30_000));
        assert_eq!(hedge_events(&e3), 0);
        let e4 = engine.ingest(sent(4, "估计八成是这样", 40_000));
        assert_eq!(hedge_events(&e4), 0);
        // 第 5 句：冷却结束，重新可显示
        let e5 = engine.ingest(sent(5, "应该差不多吧", 50_000));
        assert_eq!(hedge_events(&e5), 1);
    }

    #[test]
    fn filler_words_exempt_from_cooldown() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        // 连续两句都有高频口头禅 → 逐次照常显示（本来就按词计数不刷屏）
        let e1 = engine.ingest(sent(1, "然后他先讲背景", 10_000));
        let e2 = engine.ingest(sent(2, "然后他又补充了细节", 20_000));
        let count = |es: &[FeedbackEvent]| {
            es.iter()
                .filter(|e| e.kind == FeedbackKind::FillerWord)
                .count()
        };
        assert_eq!(count(&e1), 1);
        assert_eq!(count(&e2), 1);
    }

    #[test]
    fn cooldown_does_not_block_other_rules() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        // 立场模糊触发冷却后，同句/后续句的其他规则不受影响
        let e = engine.ingest(sent(1, "可能大概是这样", 10_000));
        assert!(e.iter().any(|e2| e2.kind == FeedbackKind::Hedge));
        let e = engine.ingest(sent(2, "最近我在想时间管理的问题", 20_000));
        assert!(e.iter().any(|e2| e2.kind == FeedbackKind::TimeVague));
    }

    // --- 统计与规则显示解耦 ---------------------------------------------------

    #[test]
    fn hedge_stats_count_even_single_occurrence_and_no_event() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        let events = engine.ingest(sent(1, "可能是我记错了", 10_000));
        assert!(events.iter().filter(|e| e.kind == FeedbackKind::Hedge).count() == 0);
        let snap = engine.snapshot();
        assert_eq!(snap.hedge_counts, vec![("可能".to_string(), 1)]);
        assert_eq!(snap.hedge_total, 1);
    }

    #[test]
    fn hedge_stats_survive_rule_disabled() {
        let mut cfg = EngineConfig::default();
        cfg.enabled.remove("hedge");
        let mut engine = RuleEngine::from_config(cfg);
        engine.start(0);
        let events = engine.ingest(sent(1, "可能大概是这样", 10_000));
        assert!(events.iter().all(|e| e.kind != FeedbackKind::Hedge));
        assert_eq!(engine.snapshot().hedge_total, 2);
    }

    #[test]
    fn filler_stats_count_medium_tier_even_when_event_gated() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        // 中频词「其实」5 分钟里只出现 1 次：不显示事件，但统计照常计数
        let events = engine.ingest(sent(1, "其实我今天想聊聊计划", 300_000));
        assert!(events.iter().all(|e| e.kind != FeedbackKind::FillerWord));
        let snap = engine.snapshot();
        assert!(snap.filler_counts.contains(&("其实".to_string(), 1)));
    }

    #[test]
    fn custom_fillers_counted_in_stats_and_events() {
        let mut cfg = EngineConfig::default();
        cfg.custom_fillers = vec!["老铁".to_string()];
        let mut engine = RuleEngine::from_config(cfg);
        engine.start(0);
        let events = engine.ingest(sent(1, "老铁们看过来", 10_000));
        assert!(events
            .iter()
            .any(|e| e.kind == FeedbackKind::FillerWord && e.payload["word"] == "老铁"));
        assert!(engine
            .snapshot()
            .filler_counts
            .contains(&("老铁".to_string(), 1)));
    }

    #[test]
    fn emotion_stats_aggregate_count_and_avg_intensity() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "我特别开心", 10_000)); // 开心 · 乐 · 5
        engine.ingest(sent(2, "简直是狂喜", 20_000)); // 狂喜 · 乐 · 9
        let snap = engine.snapshot();
        assert_eq!(
            snap.emotion_counts,
            vec![(
                "乐".to_string(),
                EmotionStat { count: 2, avg_intensity: 7.0 }
            )]
        );
    }

    #[test]
    fn disabled_rule_not_installed() {
        let mut cfg = EngineConfig::default();
        cfg.enabled.remove("time_vague");
        let mut engine = RuleEngine::from_config(cfg);
        engine.start(0);
        let events = engine.ingest(sent(1, "最近我在想很多", 10_000));
        assert!(events.iter().all(|e| e.kind != FeedbackKind::TimeVague));
    }

    // --- 金句捕捉：句间冷却 + 会话上限，统计不受显示节流影响 ------------------

    #[test]
    fn golden_quote_cooldown_and_cap_with_stats() {
        let golden = "这就像把 3 个月的工作压缩到 3 周";
        let mut engine = RuleEngine::new();
        engine.start(0);
        let count = |es: &[FeedbackEvent]| {
            es.iter().filter(|e| e.kind == FeedbackKind::GoldenQuote).count()
        };
        // 第 1 句金句 → 显示
        assert_eq!(count(&engine.ingest(sent(1, golden, 10_000))), 1);
        // 第 2–4 句（即使命中）：同类 3 句冷却，不显示（规则仍产出并计数）
        assert_eq!(count(&engine.ingest(sent(2, golden, 20_000))), 0);
        assert_eq!(count(&engine.ingest(sent(3, golden, 30_000))), 0);
        assert_eq!(count(&engine.ingest(sent(4, golden, 40_000))), 0);
        // 第 5 句冷却已过，但规则内部上限（3 句）在第 3 次产出后生效 → 不再显示
        assert_eq!(count(&engine.ingest(sent(5, golden, 50_000))), 0);
        // 统计计产出数（1+1+1 = 3），与显示与否无关
        assert_eq!(engine.snapshot().golden_quote_count, 3);
    }

    #[test]
    fn golden_quote_survives_cooldown_when_spaced_out() {
        let golden = "写周报就像照镜子，一目了然、毫不留情";
        let plain = "然后我们进入下一项议题的讨论";
        let mut engine = RuleEngine::new();
        engine.start(0);
        let count = |es: &[FeedbackEvent]| {
            es.iter().filter(|e| e.kind == FeedbackKind::GoldenQuote).count()
        };
        // 金句 → 普通 3 句（冷却消耗）→ 金句再次显示
        assert_eq!(count(&engine.ingest(sent(1, golden, 10_000))), 1);
        for i in 2..=4 {
            engine.ingest(sent(i, plain, i as u64 * 10_000));
        }
        assert_eq!(count(&engine.ingest(sent(5, golden, 50_000))), 1);
        assert_eq!(engine.snapshot().golden_quote_count, 2);
    }

    #[test]
    fn golden_quote_rule_can_be_disabled() {
        let mut cfg = EngineConfig::default();
        cfg.enabled.remove("golden_quote");
        let mut engine = RuleEngine::from_config(cfg);
        engine.start(0);
        let events = engine.ingest(sent(1, "这就像把 3 个月的工作压缩到 3 周", 10_000));
        assert!(events.iter().all(|e| e.kind != FeedbackKind::GoldenQuote));
        assert_eq!(engine.snapshot().golden_quote_count, 0);
    }

    #[test]
    fn snapshot_golden_quote_defaults_zero_for_legacy_json() {
        // 旧历史记录的 snapshot 没有 goldenQuoteCount 字段：serde default 补 0
        let legacy = r#"{
            "sentenceCount": 1, "fillerCounts": [], "fillerPerMinute": 0.0,
            "emotionCounts": [], "hedgeCounts": [], "hedgeTotal": 0,
            "durationMs": 60000, "totalChars": 10, "speechRate": 10.0,
            "avgSentenceChars": 10.0
        }"#;
        let snap: SessionSnapshot = serde_json::from_str(legacy).unwrap();
        assert_eq!(snap.golden_quote_count, 0);
    }
}
