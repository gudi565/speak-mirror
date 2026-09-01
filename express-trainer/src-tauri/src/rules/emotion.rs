use super::lexicon::{lexicon, WordMatcher};
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use std::collections::HashMap;

/// 情感词规则：词库 emotionWords（七大类 439 词，强度 1–9）。
/// 最长优先匹配（「大吃一惊」不再重复计内部的「吃惊」）；
/// 每个词每句计一次，事件 payload 带类别与强度，供快照聚合平均强度。
pub struct EmotionRule {
    matcher: WordMatcher,
    /// 词 → (类别, 强度)。类别存 owned String：词库经 Arc 生效（可含用户合并副本）
    info: HashMap<String, (String, u8)>,
}

fn emotion_info() -> &'static HashMap<String, (String, u8)> {
    static INFO: std::sync::OnceLock<HashMap<String, (String, u8)>> =
        std::sync::OnceLock::new();
    INFO.get_or_init(|| {
        let lex = lexicon();
        lex.emotion_entries()
            .into_iter()
            .map(|(w, c, i)| (w.to_string(), (c.to_string(), i)))
            .collect()
    })
}

impl EmotionRule {
    pub fn from_lexicon() -> Self {
        let info = emotion_info();
        let words: Vec<String> = info.keys().cloned().collect();
        Self {
            matcher: WordMatcher::new(words),
            info: info.clone(),
        }
    }
}

impl Default for EmotionRule {
    fn default() -> Self {
        Self::from_lexicon()
    }
}

impl Rule for EmotionRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        self.matcher
            .find_distinct(&sentence.text)
            .into_iter()
            .filter_map(|word| {
                let (category, intensity) = self.info.get(word)?.clone();
                Some(FeedbackEvent {
                    kind: FeedbackKind::Emotion,
                    sentence_id: Some(sentence.id),
                    message: format!("情感词「{word}」（{category}，强度 {intensity}）"),
                    payload: serde_json::json!({
                        "word": word,
                        "category": category,
                        "intensity": intensity,
                    }),
                })
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
    fn detects_emotion_words_with_category_and_intensity() {
        let mut rule = EmotionRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "我真的很开心但也有点紧张"), &ctx);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].payload["category"], "乐");
        assert_eq!(events[0].payload["intensity"], 5);
        assert_eq!(events[1].payload["category"], "惧");
        assert_eq!(events[1].payload["intensity"], 6);
    }

    #[test]
    fn no_event_for_neutral_sentence() {
        let mut rule = EmotionRule::default();
        let ctx = SessionContext::default();
        assert!(rule.on_sentence(&sent(1, "这个按钮在页面右上角"), &ctx).is_empty());
    }

    #[test]
    fn longest_first_no_double_count_inside_compound() {
        let mut rule = EmotionRule::default();
        let ctx = SessionContext::default();
        // 「大吃一惊」是词条（惊·7），内部的「吃惊」不再单独计
        let events = rule.on_sentence(&sent(1, "这个结果让我大吃一惊"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["word"], "大吃一惊");
        assert_eq!(events[0].payload["category"], "惊");
    }

    #[test]
    fn loads_full_lexicon_scale() {
        let rule = EmotionRule::default();
        assert_eq!(rule.info.len(), 439);
    }
}
