use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

pub const DEFAULT_FILLERS: &[&str] = &["然后", "就是", "那个", "呃", "嗯", "其实", "比如说"];

pub struct FillerWordsRule {
    words: Vec<String>,
}

impl Default for FillerWordsRule {
    fn default() -> Self {
        Self::new(DEFAULT_FILLERS.iter().map(|s| s.to_string()).collect())
    }
}

impl FillerWordsRule {
    pub fn new(words: Vec<String>) -> Self {
        Self { words }
    }
}

impl Rule for FillerWordsRule {
    fn on_sentence(&mut self, sentence: &Sentence, ctx: &SessionContext) -> Vec<FeedbackEvent> {
        let prev_total: u32 = ctx.filler_counts.values().sum();
        let mut total = prev_total;
        let mut events = Vec::new();
        let mut in_sentence: std::collections::HashMap<&str, u32> = Default::default();
        for word in &self.words {
            let occurrences = sentence.text.matches(word.as_str()).count() as u32;
            for _ in 0..occurrences {
                *in_sentence.entry(word).or_insert(0) += 1;
                total += 1;
                let elapsed_min =
                    sentence.end_ms.saturating_sub(ctx.started_at_ms) as f64 / 60_000.0;
                let per_minute = if elapsed_min > 0.0 {
                    (total as f64 / elapsed_min * 10.0).round() / 10.0
                } else {
                    0.0
                };
                events.push(FeedbackEvent {
                    kind: FeedbackKind::FillerWord,
                    sentence_id: Some(sentence.id),
                    message: format!("口头禅「{}」", word),
                    payload: serde_json::json!({
                        "word": word,
                        "countInSentence": in_sentence[word.as_str()],
                        "totalCount": total,
                        "perMinute": per_minute,
                    }),
                });
            }
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
   fn detects_each_filler_occurrence() {
       let mut rule = FillerWordsRule::default();
       let ctx = SessionContext::default();
       let events = rule.on_sentence(&sent(1, "然后我想说然后就是", 60_000), &ctx);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].payload["word"], "然后");
        assert_eq!(events[1].payload["word"], "然后");
        assert_eq!(events[1].payload["countInSentence"], 2);
        assert_eq!(events[2].payload["word"], "就是");
   }

    #[test]
    fn no_event_when_clean() {
        let mut rule = FillerWordsRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "今天介绍这个系统的三个功能", 60_000), &ctx);
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
}
