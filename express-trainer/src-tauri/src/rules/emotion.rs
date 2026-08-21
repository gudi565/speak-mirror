use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

/// MVP 内置小型情感词表（词, 类别）。DUTIR 词库作为可选下载项，后续接入。
pub static EMOTION_WORDS: &[(&str, &str)] = &[
    ("开心", "喜"), ("高兴", "喜"), ("喜悦", "喜"), ("兴奋", "喜"),
    ("愤怒", "怒"), ("生气", "怒"), ("恼火", "怒"), ("气死", "怒"),
    ("难过", "哀"), ("伤心", "哀"), ("失落", "哀"), ("遗憾", "哀"),
    ("害怕", "惧"), ("担心", "惧"), ("紧张", "惧"), ("焦虑", "惧"),
    ("讨厌", "恶"), ("恶心", "恶"), ("烦", "恶"), ("厌恶", "恶"),
    ("惊讶", "惊"), ("震惊", "惊"), ("没想到", "惊"), ("意外", "惊"),
    ("喜欢", "好"), ("佩服", "好"), ("信任", "好"), ("感谢", "好"),
];

pub struct EmotionRule;

impl Default for EmotionRule {
    fn default() -> Self {
        Self
    }
}

impl Rule for EmotionRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        EMOTION_WORDS
            .iter()
            .filter(|(word, _)| sentence.text.contains(word))
            .map(|(word, category)| FeedbackEvent {
                kind: FeedbackKind::Emotion,
                sentence_id: Some(sentence.id),
                message: format!("情感词「{}」（{}）", word, category),
                payload: serde_json::json!({ "word": word, "category": category }),
            })
            .collect()
    }

    fn name(&self) -> &'static str {
        "emotion_lexicon"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    #[test]
    fn detects_emotion_words_with_category() {
        let mut rule = EmotionRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "我真的很开心但也有点紧张"), &ctx);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].payload["category"], "喜");
        assert_eq!(events[1].payload["category"], "惧");
    }

    #[test]
    fn no_event_for_neutral_sentence() {
        let mut rule = EmotionRule::default();
        let ctx = SessionContext::default();
        assert!(rule.on_sentence(&sent(1, "这个按钮在页面右上角"), &ctx).is_empty());
    }
}
