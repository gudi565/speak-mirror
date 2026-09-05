use super::lang::{detect_lang, SentenceLang};
use super::lexicon::{lexicon, WordMatcher};
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use std::collections::{HashMap, HashSet};

/// 时间模糊规则：词库 timeVague（25 条模糊时间词 → 具体化建议）。
/// 命中即温和提示，payload 带词库里的建议替代；每词每会话最多提示一次。
/// （顶层 JSON 的 description 是元数据键，构造时已跳过。）
/// 英文句直接跳过（时间模糊词表是中文特有数据，英文侧不强做）。
pub struct TimeVagueRule {
    matcher: WordMatcher,
    suggestions: HashMap<String, String>,
    reminded: HashSet<String>,
}

impl TimeVagueRule {
    pub fn from_lexicon() -> Self {
        let lex = lexicon();
        let mut suggestions = HashMap::new();
        let mut words = Vec::new();
        for (k, v) in lex.time_vague_entries() {
            suggestions.insert(k.to_string(), v.to_string());
            words.push(k.to_string());
        }
        Self {
            matcher: WordMatcher::new(words),
            suggestions,
            reminded: HashSet::new(),
        }
    }
}

impl Default for TimeVagueRule {
    fn default() -> Self {
        Self::from_lexicon()
    }
}

impl Rule for TimeVagueRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        if detect_lang(&sentence.text) != SentenceLang::Chinese {
            return Vec::new(); // 英文/无字母句跳过：时间模糊词表是中文特有数据
        }
        let mut events = Vec::new();
        for word in self.matcher.find_distinct(&sentence.text) {
            if self.reminded.contains(word) {
                continue;
            }
            self.reminded.insert(word.to_string());
            let suggestion = self.suggestions.get(word).cloned().unwrap_or_default();
            events.push(FeedbackEvent {
                kind: FeedbackKind::TimeVague,
                sentence_id: Some(sentence.id),
                message: format!("时间有点模糊：「{word}」可以更具体"),
                payload: serde_json::json!({
                    "word": word,
                    "suggestion": suggestion,
                }),
            });
        }
        events
    }

    fn name(&self) -> &'static str {
        "time_vague"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    #[test]
    fn fires_with_suggestion_from_lexicon() {
        let mut rule = TimeVagueRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "最近我在坚持早起"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, FeedbackKind::TimeVague);
        assert_eq!(events[0].payload["word"], "最近");
        assert!(events[0].payload["suggestion"]
            .as_str()
            .unwrap()
            .contains("时间范围"));
    }

    #[test]
    fn each_word_reminded_once_per_session() {
        let mut rule = TimeVagueRule::default();
        let ctx = SessionContext::default();
        assert_eq!(rule.on_sentence(&sent(1, "回头我再把文档发你"), &ctx).len(), 1);
        assert!(rule.on_sentence(&sent(2, "回头再说吧"), &ctx).is_empty());
        // 不同词仍可提示
        assert_eq!(rule.on_sentence(&sent(3, "过几天我们碰一下"), &ctx).len(), 1);
    }

    #[test]
    fn skips_description_meta_key() {
        // description 不是词条，永远不应命中
        let mut rule = TimeVagueRule::default();
        let ctx = SessionContext::default();
        assert!(rule.on_sentence(&sent(1, "description"), &ctx).is_empty());
        assert_eq!(rule.suggestions.len(), 25);
    }

    #[test]
    fn no_event_for_specific_time_sentence() {
        let mut rule = TimeVagueRule::default();
        let ctx = SessionContext::default();
        assert!(rule.on_sentence(&sent(1, "上周三我们开了复盘会"), &ctx).is_empty());
    }

    #[test]
    fn english_sentence_is_skipped() {
        // 英文句不做时间模糊匹配（中文特有规则；英文「soon / later」不强做）
        let mut rule = TimeVagueRule::default();
        let ctx = SessionContext::default();
        assert!(rule.on_sentence(&sent(1, "We will finish it soon, maybe later"), &ctx).is_empty());
    }
}
