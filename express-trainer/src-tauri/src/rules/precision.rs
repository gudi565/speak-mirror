use super::lexicon::REPLACEMENTS;
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use std::collections::HashSet;

pub struct PrecisionRule {
    suggested: HashSet<&'static str>,
}

impl Default for PrecisionRule {
    fn default() -> Self {
        Self { suggested: HashSet::new() }
    }
}

impl Rule for PrecisionRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        let mut events = Vec::new();
        for (word, alternatives) in REPLACEMENTS {
            if self.suggested.contains(word) {
                continue;
            }
            if sentence.text.contains(word) {
                self.suggested.insert(word);
                events.push(FeedbackEvent {
                    kind: FeedbackKind::WordPrecision,
                    sentence_id: Some(sentence.id),
                    message: format!("「{}」可以更精确", word),
                    payload: serde_json::json!({
                        "original": word,
                        "alternatives": alternatives,
                    }),
                });
            }
        }
        events
    }

    fn name(&self) -> &'static str {
        "word_precision"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: 1000 }
    }

    #[test]
    fn suggests_alternatives_for_vague_words() {
        let mut rule = PrecisionRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "我想做一个很好的东西"), &ctx);
        let originals: Vec<&str> = events
            .iter()
            .map(|e| e.payload["original"].as_str().unwrap())
            .collect();
        assert!(originals.contains(&"想"));
        assert!(originals.contains(&"很好"));
        assert!(originals.contains(&"东西"));
        let first = events.iter().find(|e| e.payload["original"] == "想").unwrap();
        assert_eq!(first.payload["alternatives"][0], "渴望");
    }

    #[test]
    fn suggests_each_word_at_most_once_per_session() {
        let mut rule = PrecisionRule::default();
        let ctx = SessionContext::default();
        assert_eq!(rule.on_sentence(&sent(1, "我想要这个"), &ctx).len(), 1);
        assert!(rule.on_sentence(&sent(2, "我还想要那个"), &ctx).is_empty());
    }
}
