use super::lexicon::{lexicon, WordMatcher};
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use std::collections::{HashMap, HashSet};

/// 比喻 / 画面感标记词（产品方案 §5.3）
pub const IMAGERY_MARKERS: &[&str] = &["就像", "好比", "相当于", "仿佛", "如同"];

/// 画面感规则：
/// 1) 句内出现比喻标记（就像/好比/相当于/仿佛/如同）→ 正向提示一次（每会话一次）；
/// 2) 词库 imageryPairs 的抽象词（很累/很忙…）出现 → 给出具象化方向，
///    每个抽象词每会话提示一次（同类提示不重复刷屏）。
pub struct ImageryRule {
    marker_matcher: WordMatcher,
    abstract_matcher: WordMatcher,
    suggestions: HashMap<String, Vec<String>>,
    marker_hinted: bool,
    abstract_hinted: HashSet<String>,
}

impl ImageryRule {
    pub fn from_lexicon() -> Self {
        let lex = lexicon();
        let mut suggestions = HashMap::new();
        let mut words = Vec::new();
        for (k, v) in &lex.imagery_pairs {
            suggestions.insert(k.clone(), v.clone());
            words.push(k.clone());
        }
        Self {
            marker_matcher: WordMatcher::new(IMAGERY_MARKERS.to_vec()),
            abstract_matcher: WordMatcher::new(words),
            suggestions,
            marker_hinted: false,
            abstract_hinted: HashSet::new(),
        }
    }
}

impl Default for ImageryRule {
    fn default() -> Self {
        Self::from_lexicon()
    }
}

impl Rule for ImageryRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        let mut events = Vec::new();
        if !self.marker_hinted {
            if let Some(marker) = self.marker_matcher.find_distinct(&sentence.text).first() {
                self.marker_hinted = true;
                events.push(FeedbackEvent {
                    kind: FeedbackKind::Imagery,
                    sentence_id: Some(sentence.id),
                    message: format!("用了「{marker}」打比方，画面感好，继续保持"),
                    payload: serde_json::json!({ "marker": marker, "kind": "marker" }),
                });
            }
        }
        for word in self.abstract_matcher.find_distinct(&sentence.text) {
            if self.abstract_hinted.contains(word) {
                continue;
            }
            self.abstract_hinted.insert(word.to_string());
            let suggestions = self.suggestions.get(word).cloned().unwrap_or_default();
            let sample = suggestions.first().cloned().unwrap_or_default();
            events.push(FeedbackEvent {
                kind: FeedbackKind::Imagery,
                sentence_id: Some(sentence.id),
                message: format!("「{word}」偏抽象，可以说得更有画面感，比如「{sample}」"),
                payload: serde_json::json!({
                    "kind": "abstract",
                    "abstract": word,
                    "suggestions": suggestions,
                }),
            });
        }
        events
    }

    fn name(&self) -> &'static str {
        "imagery"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    #[test]
    fn marker_hint_fires_once_per_session() {
        let mut rule = ImageryRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "写周报就像给老板讲脱口秀"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["kind"], "marker");
        assert_eq!(events[0].payload["marker"], "就像");
        // 第二次出现比喻标记不再提示
        assert!(rule.on_sentence(&sent(2, "这就好比每天都有一场开放麦"), &ctx).is_empty());
    }

    #[test]
    fn abstract_word_gets_concrete_suggestion() {
        let mut rule = ImageryRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "这一周我很忙"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["kind"], "abstract");
        assert_eq!(events[0].payload["abstract"], "很忙");
        let suggestions = events[0].payload["suggestions"].as_array().unwrap();
        assert!(suggestions.len() >= 2);
        assert!(!events[0].payload["suggestions"][0].as_str().unwrap().is_empty());
    }

    #[test]
    fn each_abstract_word_hinted_once_per_session() {
        let mut rule = ImageryRule::default();
        let ctx = SessionContext::default();
        assert_eq!(rule.on_sentence(&sent(1, "这周连轴转，我已经很累了"), &ctx).len(), 1);
        // 「很累」第二次不提示；「很忙」是新词仍提示
        assert_eq!(rule.on_sentence(&sent(2, "还是很累但也很忙"), &ctx).len(), 1);
    }

    #[test]
    fn concrete_sentence_no_event() {
        let mut rule = ImageryRule::default();
        let ctx = SessionContext::default();
        assert!(rule.on_sentence(&sent(1, "会议纪要已同步到共享文档"), &ctx).is_empty());
    }
}
