use super::lexicon::{lexicon, WordMatcher};
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

/// 单句触发「立场模糊」提醒所需的最少犹豫弱化词出现次数。
/// 出现 1 个不触发（「应该」「感觉」等在正常句法中高频，防刷屏），
/// 计数照常进会话统计（由引擎负责累计）。
pub const HEDGE_TRIGGER_COUNT: usize = 2;

/// 立场模糊规则：词库 hedges（54 个犹豫弱化词），最长优先匹配；
/// 单句堆叠 ≥2 个才触发一次提醒。
pub struct HedgeRule {
    matcher: WordMatcher,
}

impl HedgeRule {
    pub fn from_lexicon() -> Self {
        Self {
            matcher: WordMatcher::new(lexicon().hedges.clone()),
        }
    }
}

impl Default for HedgeRule {
    fn default() -> Self {
        Self::from_lexicon()
    }
}

impl Rule for HedgeRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        let matched = self.matcher.find_all(&sentence.text);
        if matched.len() < HEDGE_TRIGGER_COUNT {
            return Vec::new();
        }
        let distinct = self.matcher.find_distinct(&sentence.text);
        let quoted: Vec<String> = distinct.iter().map(|w| format!("「{w}」")).collect();
        vec![FeedbackEvent {
            kind: FeedbackKind::Hedge,
            sentence_id: Some(sentence.id),
            message: format!("立场模糊：一句话里堆了 {} 个弱化词（{}），可以直接给判断", matched.len(), quoted.join("")),
            payload: serde_json::json!({
                "words": distinct,
                "count": matched.len(),
            }),
        }]
    }

    fn name(&self) -> &'static str {
        "hedge"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    #[test]
    fn single_hedge_does_not_trigger() {
        let mut rule = HedgeRule::default();
        let ctx = SessionContext::default();
        // 只有「我觉得」一个弱化词（「挺好」不是词条）→ 不触发
        assert!(rule.on_sentence(&sent(1, "我觉得这个方案挺好"), &ctx).is_empty());
    }

    #[test]
    fn two_distinct_hedges_trigger_once() {
        let mut rule = HedgeRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "可能大概是这个意思吧"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, FeedbackKind::Hedge);
        assert_eq!(events[0].payload["count"], 2);
        let words = events[0].payload["words"].as_array().unwrap();
        assert!(words.contains(&serde_json::json!("可能")));
        assert!(words.contains(&serde_json::json!("大概")));
    }

    #[test]
    fn same_hedge_twice_also_triggers() {
        let mut rule = HedgeRule::default();
        let ctx = SessionContext::default();
        // 「可能…可能吧」= 2 次出现（最长优先：可能吧 不被拆）
        let events = rule.on_sentence(&sent(1, "可能吧也可能是我记错了"), &ctx);
        assert_eq!(events.len(), 1);
        assert!(events[0].payload["count"].as_u64().unwrap() >= 2);
    }

    #[test]
    fn longest_first_no_double_count_for_compound_hedge() {
        let mut rule = HedgeRule::default();
        let ctx = SessionContext::default();
        // 「我觉得吧」是完整词条：不重复计「我觉得」
        let events = rule.on_sentence(&sent(1, "我觉得吧这事还行"), &ctx);
        // 我觉得吧(1) + 还行(1) = 2 → 触发，但 count 不应是 3
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["count"], 2);
    }

    #[test]
    fn clean_sentence_has_no_event() {
        let mut rule = HedgeRule::default();
        let ctx = SessionContext::default();
        assert!(rule.on_sentence(&sent(1, "这个结论有三个数据支撑"), &ctx).is_empty());
    }
}
