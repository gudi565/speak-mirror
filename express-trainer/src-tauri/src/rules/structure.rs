use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

pub const CONCLUSION_STREAK: usize = 5;
pub const OPINION_STREAK: usize = 4;

/// 结论词表（中英并列，英文标记一律小写——匹配时文本小写化后重查，
/// 容忍句首大写；子串容差意味着 "so" 命中 "sorted" 只会漏报一次提醒，
/// 符合宁可漏报不刷屏的取向）
pub const CONCLUSION_MARKERS: &[&str] = &[
    "所以", "总之", "结论是", "我的观点是", "一句话总结",
    "so", "therefore", "in conclusion", "the point is",
];
/// 观点词表（ExampleMissingRule 的连击计数用，中英并列，英文小写）
pub const OPINION_MARKERS: &[&str] = &[
    "我觉得", "我认为", "应该",
    "i think", "i believe", "i feel like", "in my opinion", "we should",
];
/// 举例词表（中英并列，英文小写）
pub const EXAMPLE_MARKERS: &[&str] = &[
    "比如", "举个例子", "就像",
    "for example", "for instance", "such as",
];

fn contains_any(text: &str, markers: &[&str]) -> bool {
    if markers.iter().any(|m| text.contains(m)) {
        return true;
    }
    // 英文标记容忍句首大写（For instance / I think…）：小写化后重查。
    // 中文标记小写化无变化，早退分支已覆盖正常命中。
    let lower = text.to_lowercase();
    markers.iter().any(|m| lower.contains(m))
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

    // --- 英文句：语言无关启发式对英文同样成立（结论/举例词表中英并列） ------

    #[test]
    fn english_conclusion_marker_resets_streak() {
        let mut rule = ConclusionMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=4 {
            assert!(rule.on_sentence(&sent(i, "We worked on the search ranking this quarter"), &ctx).is_empty());
        }
        // "in conclusion" 重置连击 → 不提醒
        assert!(rule.on_sentence(&sent(5, "In conclusion, the ranking improved"), &ctx).is_empty());
        assert!(rule.on_sentence(&sent(6, "The dashboard also changed a lot"), &ctx).is_empty());
    }

    #[test]
    fn english_description_streak_nudges_conclusion() {
        let mut rule = ConclusionMissingRule::default();
        let ctx = SessionContext::default();
        let mut fired = false;
        for i in 1..=5 {
            let events = rule.on_sentence(&sent(i, "The dashboard shows usage trends across regions"), &ctx);
            if !events.is_empty() {
                fired = true;
                assert_eq!(events[0].kind, FeedbackKind::ConclusionMissing);
            }
        }
        assert!(fired, "连续 5 句英文描述应提醒给结论");
    }

    #[test]
    fn english_opinions_without_example_nudge() {
        let mut rule = ExampleMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=3 {
            assert!(rule.on_sentence(&sent(i, "I think we should rewrite the module"), &ctx).is_empty());
        }
        let events = rule.on_sentence(&sent(4, "I believe the rewrite pays off quickly"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, FeedbackKind::ExampleMissing);
    }

    #[test]
    fn english_example_marker_resets_opinion_streak() {
        let mut rule = ExampleMissingRule::default();
        let ctx = SessionContext::default();
        for i in 1..=3 {
            rule.on_sentence(&sent(i, "I think this approach is better"), &ctx);
        }
        // "for instance" 重置 → 不提醒
        assert!(rule.on_sentence(&sent(4, "For instance, the retry path shrinks"), &ctx).is_empty());
        assert!(rule.on_sentence(&sent(5, "I think the win is clear"), &ctx).is_empty());
    }
}
