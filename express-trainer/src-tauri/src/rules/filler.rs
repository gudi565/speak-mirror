use super::lexicon::{lexicon, WordMatcher};
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use std::collections::HashMap;

/// 口头禅频级：high = 最常爆发、始终计数与提醒；medium = 次级，
/// 词频达到阈值（次/分钟）后才逐次提醒（宁可漏报）；custom = 用户自定义，视同 high。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillerTier {
    High,
    Medium,
    Custom,
}

impl FillerTier {
    pub fn as_str(self) -> &'static str {
        match self {
            FillerTier::High => "high",
            FillerTier::Medium => "medium",
            FillerTier::Custom => "custom",
        }
    }
}

/// 中频词提醒阈值的默认值（次/分钟），可在设置中调整
pub const DEFAULT_MEDIUM_THRESHOLD_PER_MIN: f64 = 3.0;

pub struct FillerWordsRule {
    matcher: WordMatcher,
    tiers: HashMap<String, FillerTier>,
    /// medium 词的提醒阈值（次/分钟）
    medium_threshold_per_min: f64,
}

impl FillerWordsRule {
    /// 词库 high + medium 分级词表 + 用户自定义词（自定义视同 high）
    pub fn with_lexicon(custom: &[String], medium_threshold_per_min: f64) -> Self {
        let lex = lexicon();
        let mut words: Vec<String> = Vec::new();
        let mut tiers = HashMap::new();
        let mut push = |word: String, tier: FillerTier, out: &mut Vec<String>| {
            if !word.is_empty() && !tiers.contains_key(&word) {
                tiers.insert(word.clone(), tier);
                out.push(word);
            }
        };
        for w in &lex.fillers.high {
            push(w.clone(), FillerTier::High, &mut words);
        }
        for w in &lex.fillers.medium {
            push(w.clone(), FillerTier::Medium, &mut words);
        }
        for w in custom {
            push(w.clone(), FillerTier::Custom, &mut words);
        }
        Self {
            matcher: WordMatcher::new(words),
            tiers,
            medium_threshold_per_min,
        }
    }
}

impl Default for FillerWordsRule {
    fn default() -> Self {
        Self::with_lexicon(&[], DEFAULT_MEDIUM_THRESHOLD_PER_MIN)
    }
}

impl Rule for FillerWordsRule {
    fn on_sentence(&mut self, sentence: &Sentence, ctx: &SessionContext) -> Vec<FeedbackEvent> {
        let matched = self.matcher.find_all(&sentence.text);
        if matched.is_empty() {
            return Vec::new();
        }
        let elapsed_min =
            sentence.end_ms.saturating_sub(ctx.started_at_ms) as f64 / 60_000.0;
        // 词级累计（含本句出现次数），供阈值判断与 totalCount
        let mut in_sentence: HashMap<&str, u32> = HashMap::new();
        for w in &matched {
            *in_sentence.entry(w).or_insert(0) += 1;
        }
        let mut word_totals: HashMap<&str, u32> = HashMap::new();
        for (w, n) in &in_sentence {
            word_totals.insert(w, ctx.filler_counts.get(*w).copied().unwrap_or(0) + n);
        }
        let round1 = |x: f64| (x * 10.0).round() / 10.0;

        let mut events = Vec::new();
        let mut cumulative_total: u32 = ctx.filler_counts.values().sum();
        // 按文本出现顺序逐次发事件，保持与 M1 相同的逐次计数语义
        for w in matched {
            cumulative_total += 1;
            let tier = self.tiers.get(w).copied().unwrap_or(FillerTier::Medium);
            let total = word_totals[w];
            let per_minute = if elapsed_min > 0.0 {
                round1(total as f64 / elapsed_min)
            } else {
                0.0
            };
            // medium 词：词频未达阈值不打扰（计数照常，由引擎统计负责）
            if tier == FillerTier::Medium && per_minute < self.medium_threshold_per_min {
                continue;
            }
            events.push(FeedbackEvent {
                kind: FeedbackKind::FillerWord,
                sentence_id: Some(sentence.id),
                message: format!("口头禅「{w}」"),
                payload: serde_json::json!({
                    "word": w,
                    "tier": tier.as_str(),
                    "countInSentence": in_sentence[w],
                    "totalCount": cumulative_total,
                    "perMinute": per_minute,
                }),
            });
        }
        events
    }

    fn name(&self) -> &'static str {
        "filler_words"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str, end_ms: u64) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms }
    }

    #[test]
    fn detects_each_filler_occurrence_with_longest_first() {
        let mut rule = FillerWordsRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "然后那个然后就是", 60_000), &ctx);
        // 「然后就是」整体命中一次，不拆成 然后 + 就是
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].payload["word"], "然后");
        assert_eq!(events[0].payload["tier"], "high");
        assert_eq!(events[0].payload["countInSentence"], 1);
        assert_eq!(events[1].payload["word"], "那个");
        assert_eq!(events[1].payload["totalCount"], 2);
        assert_eq!(events[2].payload["word"], "然后就是");
        assert_eq!(events[2].payload["totalCount"], 3);
        assert!(!events.iter().any(|e| e.payload["word"] == "就是"));
    }

    #[test]
    fn no_event_when_clean() {
        let mut rule = FillerWordsRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "今天介绍产品设计的三个要点", 60_000), &ctx);
        assert!(events.is_empty());
    }

    #[test]
    fn computes_per_minute_from_session_elapsed() {
        let mut rule = FillerWordsRule::default();
        let mut ctx = SessionContext::default();
        ctx.filler_counts.insert("然后".into(), 4); // 之前已累计 4 次
        let events = rule.on_sentence(&sent(2, "然后继续", 120_000), &ctx);
        assert_eq!(events[0].payload["totalCount"], 5);
        assert_eq!(events[0].payload["perMinute"], 2.5);
    }

    #[test]
    fn medium_tier_events_gated_by_threshold() {
        // medium 词「其实」：早期频率低不打扰
        let mut rule = FillerWordsRule::with_lexicon(&[], 3.0);
        let ctx = SessionContext::default();
        // 2 分钟内才出现 1 次 → 0.5 次/分钟 < 3，静默
        let events = rule.on_sentence(&sent(1, "其实我今天想聊聊", 120_000), &ctx);
        assert!(events.is_empty());
        // 高频词「然后」不受阈值限制
        let events = rule.on_sentence(&sent(2, "然后呢", 120_000), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["tier"], "high");
    }

    #[test]
    fn medium_tier_events_fire_once_frequency_reaches_threshold() {
        let mut rule = FillerWordsRule::with_lexicon(&[], 3.0);
        let mut ctx = SessionContext::default();
        ctx.filler_counts.insert("其实".into(), 5); // 之前已 5 次
        // 第 6 次出现在 2 分钟处 → 3.0 次/分钟 ≥ 3，提醒
        let events = rule.on_sentence(&sent(2, "其实我觉得可以", 120_000), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["word"], "其实");
        assert_eq!(events[0].payload["tier"], "medium");
    }

    #[test]
    fn custom_fillers_are_merged_and_treated_as_high() {
        let mut rule = FillerWordsRule::with_lexicon(&["绝绝子".to_string()], 999.0);
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "整场发布会绝绝子", 60_000), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["word"], "绝绝子");
        assert_eq!(events[0].payload["tier"], "custom");
        // 与词库重合的自定义词不重复计数
        let mut dedup = FillerWordsRule::with_lexicon(&["然后".to_string()], 3.0);
        let events = dedup.on_sentence(&sent(1, "然后", 60_000), &SessionContext::default());
        assert_eq!(events.len(), 1);
    }
}
