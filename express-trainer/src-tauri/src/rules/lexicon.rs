//! 词库 v2（docs/lexicon/lexicon-v2.json 的编译期内嵌副本，docs 只读）。
//!
//! 结构体对齐 docs/lexicon/README.md 的 serde 建议：启动时解析一次入缓存，
//! 解析失败 panic 并带清晰错误信息（词库是产品地基，损坏必须尽早暴露）。
//!
//! 生效词库 = 内置词库 + 用户词库（appdata/user-lexicon.json，见 growth.rs）：
//! 用户条目优先、同词覆盖。`apply_user_lexicon` 更新全局生效副本，
//! 各规则经 `lexicon()` 取到的始终是合并后的词库。
//!
//! 注意：`timeVague` / `hedgeToDirectMap` 里各有一个 `description` 元数据键，
//! 规则侧遍历实体条目时用 `time_vague_entries()` / `hedge_to_direct_entries()` 跳过它。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

pub const LEXICON_JSON: &str = include_str!("../../lexicon/lexicon-v2.json");

/// 元数据键：出现在 `timeVague` / `hedgeToDirectMap` 顶层的说明文字，不是词条
pub const META_DESCRIPTION_KEY: &str = "description";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LexiconV2 {
    #[serde(rename = "_meta")]
    pub meta: serde_json::Value,
    pub fillers: Fillers,
    pub hedges: Vec<String>,
    #[serde(rename = "vagueToPrecise")]
    pub vague_to_precise: HashMap<String, Vec<String>>,
    #[serde(rename = "emotionWords")]
    pub emotion_words: HashMap<String, EmotionGroup>,
    #[serde(rename = "timeVague")]
    pub time_vague: HashMap<String, String>,
    #[serde(rename = "imageryPairs")]
    pub imagery_pairs: HashMap<String, Vec<String>>,
    #[serde(rename = "intensityScale")]
    pub intensity_scale: serde_json::Value,
    #[serde(rename = "hedgeToDirectMap")]
    pub hedge_to_direct_map: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fillers {
    pub high: Vec<String>,
    pub medium: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmotionGroup {
    pub description: String,
    pub words: HashMap<String, u8>,
}

impl LexiconV2 {
    /// timeVague 的实体条目（跳过 description 元数据键）
    pub fn time_vague_entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.time_vague
            .iter()
            .filter(|(k, _)| k.as_str() != META_DESCRIPTION_KEY)
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// hedgeToDirectMap 的实体条目（跳过 description 元数据键，报告层用）
    pub fn hedge_to_direct_entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.hedge_to_direct_map
            .iter()
            .filter(|(k, _)| k.as_str() != META_DESCRIPTION_KEY)
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// 情绪词条扁平化：(词, 类别, 强度 1–9)。跨类无重复词（词库约定）。
    pub fn emotion_entries(&self) -> Vec<(&str, &str, u8)> {
        let mut v: Vec<(&str, &str, u8)> = self
            .emotion_words
            .iter()
            .flat_map(|(cat, group)| {
                group
                    .words
                    .iter()
                    .map(move |(w, i)| (w.as_str(), cat.as_str(), *i))
            })
            .collect();
        v.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()));
        v
    }
}

/// 解析并全局缓存内置词库。损坏时 panic（带错误详情），因为全部实时规则都依赖它。
pub fn builtin_lexicon() -> &'static LexiconV2 {
    static LEXICON: OnceLock<LexiconV2> = OnceLock::new();
    LEXICON.get_or_init(|| {
        serde_json::from_str(LEXICON_JSON).unwrap_or_else(|e| {
            panic!(
                "词库 src-tauri/lexicon/lexicon-v2.json 解析失败：{e}。\
                 该文件是编译期内嵌的产品地基，请校验 JSON 后重新构建。"
            )
        })
    })
}

/// 用户词库（appdata/user-lexicon.json）：vagueToPrecise 条目 + 可选自定义 filler 词。
/// 由「设置 → 词库候选」写入（growth.rs 命令层负责 IO）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserLexicon {
    /// 笼统词 → 精准替代（覆盖内置同词条目）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub vague_to_precise: HashMap<String, Vec<String>>,
    /// 自定义口头禅（并入 high 档，视同高频词）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fillers: Vec<String>,
}

impl UserLexicon {
    pub fn is_empty(&self) -> bool {
        self.vague_to_precise.is_empty() && self.fillers.is_empty()
    }
}

/// 合并内置词库与用户词库（纯函数，可单测）：
/// - vagueToPrecise：用户条目优先，同词覆盖内置；
/// - fillers：用户词并入 high 档（去重、跳过空词）。
pub fn merge_user_lexicon(base: &LexiconV2, user: &UserLexicon) -> LexiconV2 {
    let mut merged = base.clone();
    for (word, alternatives) in &user.vague_to_precise {
        let cleaned: Vec<String> = alternatives
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !word.trim().is_empty() && !cleaned.is_empty() {
            merged.vague_to_precise.insert(word.trim().to_string(), cleaned);
        }
    }
    for w in &user.fillers {
        let w = w.trim();
        if !w.is_empty() && !merged.fillers.high.contains(&w.to_string()) {
            merged.fillers.high.push(w.to_string());
        }
    }
    merged
}

static SHARED_BUILTIN: OnceLock<Arc<LexiconV2>> = OnceLock::new();
static EFFECTIVE: RwLock<Option<Arc<LexiconV2>>> = RwLock::new(None);

/// 应用用户词库：合并进此后 `lexicon()` 的返回值。
/// None / 空用户词库 = 恢复纯内置词库。重复调用以最后一次为准。
pub fn apply_user_lexicon(user: Option<&UserLexicon>) {
    let next = user
        .filter(|u| !u.is_empty())
        .map(|u| Arc::new(merge_user_lexicon(builtin_lexicon(), u)));
    *EFFECTIVE.write().unwrap() = next;
}

/// 当前生效词库（内置 + 已应用的用户词库；未应用过 = 纯内置）。
/// 返回 Arc 克隆：规则/统计只在小界面构建，非热路径。
pub fn lexicon() -> Arc<LexiconV2> {
    if let Some(arc) = EFFECTIVE.read().unwrap().clone() {
        return arc;
    }
    SHARED_BUILTIN
        .get_or_init(|| Arc::new(builtin_lexicon().clone()))
        .clone()
}

// ---------------------------------------------------------------------------
// 最长优先匹配器
// ---------------------------------------------------------------------------

/// 最长优先（longest-first）词表匹配器：从左到右扫描文本，每个位置优先命中
/// 该位置起始的最长词条，命中后跳过整个词长。避免「然后就是」被拆成
/// 「然后」+「就是」、「大吃一惊」里再计一次「吃惊」这类重复计数。
pub struct WordMatcher {
    /// 词原文（供回传借用引用）
    words: Vec<String>,
    /// 与 words 平行的字符数组（避免逐次分配）
    char_words: Vec<Vec<char>>,
    /// 首字 → 词下标（同首字按词长降序）
    by_first: HashMap<char, Vec<usize>>,
}

impl WordMatcher {
    pub fn new<I, S>(words: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let words: Vec<String> = words
            .into_iter()
            .map(Into::into)
            .filter(|w| !w.is_empty()) // 空词条无意义，先剔除保证索引对齐
            .collect();
        let mut char_words: Vec<Vec<char>> = Vec::with_capacity(words.len());
        let mut by_first: HashMap<char, Vec<usize>> = HashMap::new();
        for (i, w) in words.iter().enumerate() {
            let cw: Vec<char> = w.chars().collect();
            by_first.entry(cw[0]).or_default().push(i);
            char_words.push(cw);
        }
        // 同首字：长词在前（最长优先）
        for ids in by_first.values_mut() {
            ids.sort_by(|&a, &b| char_words[b].len().cmp(&char_words[a].len()));
        }
        Self { words, char_words, by_first }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// 返回按出现顺序的全部命中（含同词多次出现）。
    pub fn find_all(&self, text: &str) -> Vec<&str> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < chars.len() {
            let mut matched = None;
            if let Some(ids) = self.by_first.get(&chars[pos]) {
                for &wi in ids {
                    let w = &self.char_words[wi];
                    if pos + w.len() <= chars.len() && chars[pos..pos + w.len()] == w[..] {
                        matched = Some((wi, w.len()));
                        break; // 同首字已按长度降序，首个命中即最长
                    }
                }
            }
            match matched {
                Some((wi, len)) => {
                    out.push(self.words[wi].as_str());
                    pos += len;
                }
                None => pos += 1,
            }
        }
        out
    }

    /// 命中的去重词表（保持首次出现顺序）
    pub fn find_distinct(&self, text: &str) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for w in self.find_all(text) {
            if !seen.contains(&w) {
                seen.push(w);
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexicon_parses_and_meta_counts_match_entries() {
        let lex = lexicon();
        let emotion_total: usize = lex.emotion_words.values().map(|g| g.words.len()).sum();
        assert_eq!(emotion_total, 439);
        assert_eq!(lex.vague_to_precise.len(), 126);
        assert_eq!(lex.hedges.len(), 54);
        assert_eq!(lex.fillers.high.len() + lex.fillers.medium.len(), 45);
        assert_eq!(lex.time_vague_entries().count(), 25);
        assert_eq!(lex.imagery_pairs.len(), 23);
        // 与 _meta.counts 程序校验一致（README 维护纪律）
        assert_eq!(lex.meta["counts"]["emotionWords"], 439);
        assert_eq!(lex.meta["counts"]["vagueToPrecise"], 126);
        assert_eq!(lex.meta["counts"]["hedges"], 54);
        assert_eq!(lex.meta["counts"]["timeVague"], 25);
    }

    #[test]
    fn meta_description_keys_are_skipped_in_entries() {
        let lex = lexicon();
        // hedgeToDirectMap 顶层存在 description 元数据键，实体条目迭代跳过它
        assert!(lex.hedge_to_direct_map.contains_key(META_DESCRIPTION_KEY));
        assert!(!lex.hedge_to_direct_entries().any(|(k, _)| k == META_DESCRIPTION_KEY));
        // timeVague 的实体条目同样不受影响（当前文件无 description 键，25 条全为词条）
        assert!(!lex.time_vague_entries().any(|(k, _)| k == META_DESCRIPTION_KEY));
    }

    #[test]
    fn emotion_entries_cover_seven_categories_with_intensity() {
        let lex = lexicon();
        let entries = lex.emotion_entries();
        assert_eq!(entries.len(), 439);
        let cats: std::collections::HashSet<&str> =
            entries.iter().map(|(_, c, _)| *c).collect();
        assert_eq!(
            cats,
            ["乐", "好", "怒", "哀", "惧", "恶", "惊"]
                .iter()
                .copied()
                .collect()
        );
        assert!(entries.iter().all(|(_, _, i)| (1..=9).contains(i)));
    }

    #[test]
    fn matcher_prefers_longest_word() {
        let m = WordMatcher::new(["然后", "就是", "然后就是", "就是就是"]);
        // 「然后就是」整体命中一次，不拆成 然后 + 就是
        assert_eq!(m.find_all("他说然后就是没问题"), vec!["然后就是"]);
        assert_eq!(m.find_all("然后我就是"), vec!["然后", "就是"]);
    }

    #[test]
    fn matcher_skips_overlapped_shorter_word() {
        // 「大吃一惊」是词条时，内部的「吃惊」不再计
        let m = WordMatcher::new(["吃惊", "大吃一惊"]);
        assert_eq!(m.find_all("我大吃一惊"), vec!["大吃一惊"]);
        assert_eq!(m.find_all("我很吃惊"), vec!["吃惊"]);
    }

    #[test]
    fn matcher_counts_repeats_and_finds_distinct() {
        let m = WordMatcher::new(["可能", "可能吧"]);
        assert_eq!(m.find_all("可能可能吧"), vec!["可能", "可能吧"]);
        assert_eq!(m.find_distinct("可能可能"), vec!["可能"]);
        assert!(m.find_all("毫无命中").is_empty());
    }

    #[test]
    fn matcher_handles_empty_word_list() {
        let m = WordMatcher::new(Vec::<String>::new());
        assert!(m.is_empty());
        assert!(m.find_all("任意文本").is_empty());
    }

    // --- 用户词库合并（词库自生长：用户条目优先、同词覆盖） -------------------

    #[test]
    fn user_lexicon_overrides_and_extends_builtin() {
        let base = builtin_lexicon();
        let mut user = UserLexicon::default();
        // 覆盖内置同词条目（「很多」内置已有 6 个替代）
        user.vague_to_precise
            .insert("很多".into(), vec!["一大把".into(), "成片".into()]);
        // 新增词条
        user.vague_to_precise
            .insert("内卷".into(), vec!["过度竞争".into(), "非理性内耗".into()]);
        // filler：内置词不重复、新词并入 high 档
        user.fillers = vec!["然后".into(), "绝绝子".into()];

        let merged = merge_user_lexicon(base, &user);
        assert_eq!(
            merged.vague_to_precise["很多"],
            vec!["一大把".to_string(), "成片".to_string()]
        );
        assert_eq!(
            merged.vague_to_precise["内卷"],
            vec!["过度竞争".to_string(), "非理性内耗".to_string()]
        );
        assert_eq!(merged.vague_to_precise.len(), base.vague_to_precise.len() + 1);
        assert!(merged.fillers.high.contains(&"绝绝子".to_string()));
        assert_eq!(merged.fillers.high.iter().filter(|w| *w == "然后").count(), 1);
        assert_eq!(merged.fillers.high.len(), base.fillers.high.len() + 1);
        // 纯函数纪律：内置词库不被修改
        assert_eq!(base.vague_to_precise.len(), 126);
        assert!(!base.vague_to_precise.contains_key("内卷"));
        assert!(!base.fillers.high.contains(&"绝绝子".to_string()));
    }

    #[test]
    fn user_lexicon_skips_invalid_entries() {
        let base = builtin_lexicon();
        let mut user = UserLexicon::default();
        user.vague_to_precise.insert("  ".into(), vec!["替代".into()]); // 空词
        user.vague_to_precise.insert("空替代".into(), vec!["  ".into(), "".into()]); // 替代全空
        user.fillers = vec!["  ".into(), "".into()]; // 空 filler
        let merged = merge_user_lexicon(base, &user);
        assert_eq!(merged.vague_to_precise.len(), base.vague_to_precise.len());
        assert_eq!(merged.fillers.high.len(), base.fillers.high.len());
        // 空用户词库判空（apply 时据此回落内置）
        assert!(UserLexicon::default().is_empty());
        assert!(!user.is_empty());
    }

    #[test]
    fn user_lexicon_serializes_camel_case_and_roundtrips() {
        let mut user = UserLexicon::default();
        user.vague_to_precise.insert("内卷".into(), vec!["过度竞争".into()]);
        user.fillers = vec!["绝绝子".into()];
        let v = serde_json::to_value(&user).unwrap();
        assert_eq!(v["vagueToPrecise"]["内卷"][0], "过度竞争");
        assert_eq!(v["fillers"][0], "绝绝子");
        let back: UserLexicon = serde_json::from_value(v).unwrap();
        assert_eq!(back, user);
        // 空词库序列化为 {}（空段省略），反序列化回空
        let empty = serde_json::to_value(UserLexicon::default()).unwrap();
        assert!(empty.get("vagueToPrecise").is_none());
        assert_eq!(serde_json::from_value::<UserLexicon>(json_object()).unwrap(), UserLexicon::default());
    }

    fn json_object() -> serde_json::Value {
        serde_json::json!({})
    }
}
