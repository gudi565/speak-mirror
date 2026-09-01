use super::lexicon::{lexicon, WordMatcher};
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use std::collections::{HashMap, HashSet};

/// 词汇精确度规则：词库 vagueToPrecise（126 组笼统词 → 精准替代）。
/// 最长优先匹配（如「很多」不会被「很多次」类更长前缀拆坏重复计）；
/// 每个词每会话最多提示一次。
pub struct PrecisionRule {
    matcher: WordMatcher,
    alternatives: HashMap<String, Vec<String>>,
    suggested: HashSet<String>,
}

impl PrecisionRule {
    pub fn from_lexicon() -> Self {
        let lex = lexicon();
        let mut alternatives = HashMap::new();
        let mut words = Vec::new();
        for (k, v) in &lex.vague_to_precise {
            alternatives.insert(k.clone(), v.clone());
            words.push(k.clone());
        }
        Self {
            matcher: WordMatcher::new(words),
            alternatives,
            suggested: HashSet::new(),
        }
    }
}

impl Default for PrecisionRule {
    fn default() -> Self {
        Self::from_lexicon()
    }
}

impl Rule for PrecisionRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        let mut events = Vec::new();
        for word in self.matcher.find_distinct(&sentence.text) {
            if self.suggested.contains(word) {
                continue;
            }
            self.suggested.insert(word.to_string());
            let alternatives = self
                .alternatives
                .get(word)
                .cloned()
                .unwrap_or_default();
            events.push(FeedbackEvent {
                kind: FeedbackKind::WordPrecision,
                sentence_id: Some(sentence.id),
                message: format!("「{word}」可以更精确"),
                payload: serde_json::json!({
                    "original": word,
                    "alternatives": alternatives,
                }),
            });
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

    #[test]
    fn longest_first_avoids_splitting_longer_keys() {
        // 词库含「想想」与「想」：句子里的「想想」只按长词计一次
        let mut rule = PrecisionRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "大家想想这个思路"), &ctx);
        let originals: Vec<&str> = events
            .iter()
            .map(|e| e.payload["original"].as_str().unwrap())
            .collect();
        assert_eq!(originals, vec!["想想"]);
        // 再出现单字「想」仍可单独提示（不同词）
        let events = rule.on_sentence(&sent(2, "我想到了一个点子"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["original"], "想");
    }

    #[test]
    fn lexicon_scale_is_loaded() {
        let rule = PrecisionRule::default();
        assert_eq!(rule.alternatives.len(), 126);
    }
}
