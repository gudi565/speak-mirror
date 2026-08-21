use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

pub const CONCLUSION_STREAK: usize = 5;
pub const OPINION_STREAK: usize = 4;

pub const CONCLUSION_MARKERS: &[&str] = &["所以", "总之", "结论是", "我的观点是", "一句话总结"];
pub const OPINION_MARKERS: &[&str] = &["我觉得", "我认为", "应该"];
pub const EXAMPLE_MARKERS: &[&str] = &["比如", "举个例子", "就像"];

fn contains_any(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| text.contains(m))
}

pub struct ConclusionMissingRule {
    streak: usize,
}

impl Default for ConclusionMissingRule {
    fn default() -> Self {
        Self { streak: 0 }
    }
}

impl Rule for ConclusionMissingRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        if contains_any(&sentence.text, CONCLUSION_MARKERS) {
            self.streak = 0;
            return vec![];
        }
        self.streak += 1;
        if self.streak == CONCLUSION_STREAK {
            self.streak = 0; // 提醒后重新计，避免连刷
            return vec![FeedbackEvent {
                kind: FeedbackKind::ConclusionMissing,
                sentence_id: Some(sentence.id),
                message: "描述已持续很久，该给结论了".into(),
                payload: serde_json::json!({ "streak": CONCLUSION_STREAK }),
            }];
        }
        vec![]
    }

    fn name(&self) -> &'static str {
        "conclusion_missing"
    }
}

pub struct ExampleMissingRule {
    opinion_streak: usize,
}

impl Default for ExampleMissingRule {
    fn default() -> Self {
        Self { opinion_streak: 0 }
    }
}

impl Rule for ExampleMissingRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        if contains_any(&sentence.text, EXAMPLE_MARKERS) {
            self.opinion_streak = 0;
            return vec![];
        }
        if contains_any(&sentence.text, OPINION_MARKERS) {
            self.opinion_streak += 1;
        } else {
            self.opinion_streak = 0;
        }
        if self.opinion_streak == OPINION_STREAK {
            self.opinion_streak = 0;
            return vec![FeedbackEvent {
                kind: FeedbackKind::ExampleMissing,
                sentence_id: Some(sentence.id),
                message: "连续输出观点了，举个例子会更有画面感".into(),
                payload: serde_json::json!({ "streak": OPINION_STREAK }),
            }];
        }
        vec![]
    }

    fn name(&self) -> &'static str {
        "example_missing"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    #[test]
    fn conclusion_nudge_after_five_descriptive_sentences() {
        let mut rule = ConclusionMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=4 {
            assert!(rule.on_sentence(&sent(i, "这里有一个很细节的描述"), &ctx).is_empty());
        }
        let events = rule.on_sentence(&sent(5, "继续补充更多的细节内容"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, FeedbackKind::ConclusionMissing);
    }

    #[test]
    fn conclusion_marker_resets_streak() {
        let mut rule = ConclusionMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=4 {
            rule.on_sentence(&sent(i, "细节描述"), &ctx);
        }
        assert!(rule.on_sentence(&sent(5, "所以这就是结论"), &ctx).is_empty());
        assert!(rule.on_sentence(&sent(6, "又开始描述"), &ctx).is_empty());
    }

    #[test]
    fn example_nudge_after_four_opinions_without_example() {
        let mut rule = ExampleMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=3 {
            assert!(rule.on_sentence(&sent(i, "我觉得这个方向没问题"), &ctx).is_empty());
        }
        let events = rule.on_sentence(&sent(4, "我认为还应该继续推进"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, FeedbackKind::ExampleMissing);
    }

    #[test]
    fn example_marker_resets_opinion_streak() {
        let mut rule = ExampleMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=3 {
            rule.on_sentence(&sent(i, "我觉得这样更好"), &ctx);
        }
        assert!(rule.on_sentence(&sent(4, "举个例子来说明一下"), &ctx).is_empty());
        assert!(rule.on_sentence(&sent(5, "我觉得还要继续"), &ctx).is_empty());
    }
}
