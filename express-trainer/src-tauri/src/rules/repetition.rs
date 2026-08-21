use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

pub const WINDOW: usize = 20;
pub const THRESHOLD: f64 = 0.7;

use std::collections::HashSet;

pub struct RepetitionRule;

impl Default for RepetitionRule {
    fn default() -> Self {
        Self
    }
}

/// 字符 bigram 的 Jaccard 相似度；任一字符串不足 2 字时按是否完全相等处理
pub fn char_bigram_jaccard(a: &str, b: &str) -> f64 {
    let bigrams = |s: &str| -> HashSet<String> {
        let chars: Vec<char> = s.chars().collect();
        chars
            .windows(2)
            .map(|w| w.iter().collect::<String>())
            .collect()
    };
    let (sa, sb) = (bigrams(a), bigrams(b));
    if sa.is_empty() || sb.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    inter / union
}

impl Rule for RepetitionRule {
    fn on_sentence(&mut self, sentence: &Sentence, ctx: &SessionContext) -> Vec<FeedbackEvent> {
        let recent = ctx
            .sentences
            .iter()
            .rev()
            .take(WINDOW)
            .filter(|s| s.id != sentence.id);
        let mut events = Vec::new();
        for prev in recent {
            let sim = char_bigram_jaccard(&sentence.text, &prev.text);
            if sim > THRESHOLD {
                events.push(FeedbackEvent {
                    kind: FeedbackKind::Repetition,
                    sentence_id: Some(sentence.id),
                    message: "这句话已经说过一遍了".into(),
                    payload: serde_json::json!({
                        "similarTo": prev.id,
                        "similarity": (sim * 100.0).round() / 100.0,
                        "text": prev.text,
                    }),
                });
                break; // 一句只提醒一次
            }
        }
        events
    }

    fn name(&self) -> &'static str {
        "repetition"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    #[test]
    fn bigram_jaccard_identical_is_one() {
        assert_eq!(char_bigram_jaccard("今天天气不错", "今天天气不错"), 1.0);
    }

    #[test]
    fn bigram_jaccard_disjoint_is_zero() {
        assert_eq!(char_bigram_jaccard("abcde", "xyz"), 0.0);
    }

    #[test]
    fn flags_near_duplicate_sentence() {
        let mut rule = RepetitionRule::default();
        let mut ctx = SessionContext::default();
        ctx.sentences.push(sent(1, "这个系统可以实时分析你的表达问题"));
        let events = rule.on_sentence(&sent(2, "这个系统可以实时分析你的表达问题啊"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["similarTo"], 1);
    }

    #[test]
    fn ignores_different_sentence() {
        let mut rule = RepetitionRule::default();
        let mut ctx = SessionContext::default();
        ctx.sentences.push(sent(1, "这个系统可以实时分析你的表达问题"));
        let events = rule.on_sentence(&sent(2, "晚饭吃什么比较好呢"), &ctx);
        assert!(events.is_empty());
    }
}
