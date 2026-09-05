use super::lang::lexicon_routes;
use super::lexicon::{builtin_lexicon_en, lexicon, WordMatcher};
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use std::collections::{HashMap, HashSet};

/// 单语言的笼统词表（匹配器 + 替代词）；中英两表文字不相交
struct PrecLexicon {
    matcher: WordMatcher,
    alternatives: HashMap<String, Vec<String>>,
}

fn build_prec_lexicon(entries: &HashMap<String, Vec<String>>) -> PrecLexicon {
    let mut alternatives = HashMap::new();
    let mut words = Vec::new();
    for (k, v) in entries {
        alternatives.insert(k.clone(), v.clone());
        words.push(k.clone());
    }
    PrecLexicon { matcher: WordMatcher::new(words), alternatives }
}

/// 词汇精确度规则：中文词库 vagueToPrecise（126 组）+ 英文词库 vagueToPrecise
/// （76 组），按句语言路由（混合句两套都跑）。最长优先匹配（如「很多」不会被
/// 「很多次」类更长前缀拆坏重复计；英文 like 不命中 likely）；每个词每会话
/// 最多提示一次。
pub struct PrecisionRule {
    zh: PrecLexicon,
    en: PrecLexicon,
    suggested: HashSet<String>,
}

impl PrecisionRule {
    pub fn from_lexicon() -> Self {
        let zh_lex = lexicon();
        let en_lex = builtin_lexicon_en();
        Self {
            zh: build_prec_lexicon(&zh_lex.vague_to_precise),
            en: build_prec_lexicon(&en_lex.vague_to_precise),
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
        let (run_zh, run_en) = lexicon_routes(&sentence.text);
        // (词表, 是否英文文案)：run 标志由语言路由决定，文案语言随词表
        for (lex, english_copy) in [(&self.zh, false), (&self.en, true)] {
            if !(if english_copy { run_en } else { run_zh }) {
                continue;
            }
            for word in lex.matcher.find_distinct(&sentence.text) {
                if self.suggested.contains(word) {
                    continue;
                }
                self.suggested.insert(word.to_string());
                let alternatives = lex
                    .alternatives
                    .get(word)
                    .cloned()
                    .unwrap_or_default();
                let message = if english_copy {
                    // 英文文案：'very' → remarkably, exceptionally
                    if alternatives.is_empty() {
                        format!("'{word}' could be more precise")
                    } else {
                        format!("'{word}' → {}", alternatives.join(", "))
                    }
                } else {
                    format!("「{word}」可以更精确")
                };
                events.push(FeedbackEvent {
                    kind: FeedbackKind::WordPrecision,
                    sentence_id: Some(sentence.id),
                    message,
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
        assert_eq!(rule.zh.alternatives.len(), 126);
        assert_eq!(rule.en.alternatives.len(), 76);
    }

    // --- 英文路由 ----------------------------------------------------------

    #[test]
    fn english_vague_words_get_english_copy() {
        let mut rule = PrecisionRule::default();
        let ctx = SessionContext::default();
        let events = rule.on_sentence(&sent(1, "We did a lot of work on this thing"), &ctx);
        let originals: Vec<&str> = events
            .iter()
            .map(|e| e.payload["original"].as_str().unwrap())
            .collect();
        assert!(originals.contains(&"a lot of"));
        assert!(originals.contains(&"thing"));
        // 英文文案格式：'very' → remarkably, exceptionally（payload 结构不变）
        let events = rule.on_sentence(&sent(2, "It was very good"), &ctx);
        let very_event = events.iter().find(|e| e.payload["original"] == "very").unwrap();
        assert!(very_event.message.starts_with("'very' → "));
        let alts = very_event.payload["alternatives"].as_array().unwrap();
        assert!(alts.len() >= 2);
    }

    #[test]
    fn english_boundary_prevents_subword_hits() {
        let mut rule = PrecisionRule::default();
        let ctx = SessionContext::default();
        // stuff 不命中 stuffing；good 不命中 goodness
        let events = rule.on_sentence(&sent(1, "The goodness of this stuffing surprised us"), &ctx);
        let originals: Vec<&str> = events
            .iter()
            .map(|e| e.payload["original"].as_str().unwrap())
            .collect();
        assert!(!originals.contains(&"good"));
        assert!(!originals.contains(&"stuff"));
        // 句首大写 Good 照常命中
        let events = rule.on_sentence(&sent(2, "Good design matters"), &ctx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["original"], "good");
    }

    #[test]
    fn suggested_set_shared_across_languages() {
        // 同一个英文词跨句只提示一次（suggested 集合语言间共用）
        let mut rule = PrecisionRule::default();
        let ctx = SessionContext::default();
        assert!(!rule.on_sentence(&sent(1, "good enough"), &ctx).is_empty());
        assert!(rule.on_sentence(&sent(2, "still good"), &ctx).is_empty());
    }
}
