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
pub const LEXICON_EN_JSON: &str = include_str!("../../lexicon/lexicon-en.json");

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

/// 英文词库（lexicon-en.json）：只含 fillers / hedges / vagueToPrecise 三类
/// 可移植数据；情绪词/时间模糊/画面感等中文特有规则英文句子直接跳过。
/// 全部自建（MIT），无用户合并层（英文侧用户自增长留后续）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LexiconEn {
    #[serde(rename = "_meta")]
    pub meta: serde_json::Value,
    pub fillers: Fillers,
    pub hedges: Vec<String>,
    #[serde(rename = "vagueToPrecise")]
    pub vague_to_precise: HashMap<String, Vec<String>>,
}

/// 解析并全局缓存英文内置词库（损坏即 panic，口径同中文词库）。
pub fn builtin_lexicon_en() -> &'static LexiconEn {
    static LEXICON: OnceLock<LexiconEn> = OnceLock::new();
    LEXICON.get_or_init(|| {
        serde_json::from_str(LEXICON_EN_JSON).unwrap_or_else(|e| {
            panic!(
                "词库 src-tauri/lexicon/lexicon-en.json 解析失败：{e}。\
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

/// 判断一个字符是否算「词内字符」（等效正则 \w 的 ASCII 部分）：
/// 英文字母 / 数字 / 下划线。CJK 字符不算——中文本无词边界，且这保证
/// 混合句「然后 like 这个」里的 like 两侧（CJK）不构成边界阻挡。
fn is_ascii_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// 词条是否走「英文词边界」匹配：纯 ASCII 且至少含一个字母数字
/// （如 don't、you know、right?）；含 CJK 的词保持子串匹配的现行为。
fn is_boundary_entry(chars: &[char]) -> bool {
    chars.iter().all(|c| c.is_ascii()) && chars.iter().any(|c| c.is_ascii_alphanumeric())
}

/// 最长优先（longest-first）词表匹配器：从左到右扫描文本，每个位置优先命中
/// 该位置起始的最长词条，命中后跳过整个词长。避免「然后就是」被拆成
/// 「然后」+「就是」、「大吃一惊」里再计一次「吃惊」这类重复计数。
///
/// 英文词条（纯 ASCII，含撇号如 don't、含空格短语如 you know）按词边界匹配
/// （等效 \b 语义）：匹配起点前一字符与终点后一字符都不得是英文字母/数字/
/// 下划线——「like」不再命中「likely」、「so」不再命中「sorted」；多词短语
/// 整体匹配即可（短语两侧边界即逐词边界）。英文词条同时大小写不敏感
/// （ASR 输出大小写不可控，句首 "Like" 也该数）。含 CJK 的词条保持子串匹配
/// 与大小写敏感（中文无此问题）。
pub struct WordMatcher {
    /// 词原文（供回传借用引用）
    words: Vec<String>,
    /// 与 words 平行的字符数组（避免逐次分配）
    char_words: Vec<Vec<char>>,
    /// 与 words 平行：是否英文边界词条
    boundary: Vec<bool>,
    /// 首字符（ASCII 词取小写）→ 词下标（同首字按词长降序）
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
        let mut boundary: Vec<bool> = Vec::with_capacity(words.len());
        let mut by_first: HashMap<char, Vec<usize>> = HashMap::new();
        for (i, w) in words.iter().enumerate() {
            let cw: Vec<char> = w.chars().collect();
            by_first.entry(cw[0].to_ascii_lowercase()).or_default().push(i);
            boundary.push(is_boundary_entry(&cw));
            char_words.push(cw);
        }
        // 同首字：长词在前（最长优先）
        for ids in by_first.values_mut() {
            ids.sort_by(|&a, &b| char_words[b].len().cmp(&char_words[a].len()));
        }
        Self { words, char_words, boundary, by_first }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// 词条 wi 在 chars[pos..] 处是否命中；命中返回词长（找不到返回 None）。
    /// 英文边界词条：两侧不得是 ASCII 词内字符，且大小写不敏感比较。
    fn match_at(&self, wi: usize, chars: &[char], pos: usize) -> Option<usize> {
        let w = &self.char_words[wi];
        if pos + w.len() > chars.len() {
            return None;
        }
        if self.boundary[wi] {
            // \b 语义：等效正则 \w（ASCII 部分）——CJK 字符不阻挡边界
            if pos > 0 && is_ascii_word_char(chars[pos - 1]) {
                return None;
            }
            let end = pos + w.len();
            if end < chars.len() && is_ascii_word_char(chars[end]) {
                return None;
            }
            for (i, wc) in w.iter().enumerate() {
                if wc.to_ascii_lowercase() != chars[pos + i].to_ascii_lowercase() {
                    return None;
                }
            }
        } else if chars[pos..pos + w.len()] != w[..] {
            return None;
        }
        Some(w.len())
    }

    /// 返回按出现顺序的全部命中（含同词多次出现）。
    pub fn find_all(&self, text: &str) -> Vec<&str> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < chars.len() {
            let mut matched = None;
            if let Some(ids) = self.by_first.get(&chars[pos].to_ascii_lowercase()) {
                for &wi in ids {
                    // 同首字已按长度降序，首个命中即最长（边界不满足会继续试短词）
                    if let Some(len) = self.match_at(wi, &chars, pos) {
                        matched = Some((wi, len));
                        break;
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

    // --- 英文词边界匹配（等效 \b 语义；含 CJK 的词条保持子串行为） ---------

    #[test]
    fn matcher_english_word_boundary() {
        let m = WordMatcher::new(["like"]);
        // like 不再命中 likely / unlike（子词误报）
        assert!(m.find_all("this is likely fine").is_empty());
        assert!(m.find_all("unlikely candidate").is_empty());
        // 独立成词才命中：两侧是标点/空格/边界
        assert_eq!(m.find_all("I like it"), vec!["like"]);
        assert_eq!(m.find_all("like, um, like"), vec!["like", "like"]);
    }

    #[test]
    fn matcher_english_boundary_case_insensitive() {
        let m = WordMatcher::new(["like", "you know"]);
        // 句首大写也能命中（ASR 输出大小写不可控）
        assert_eq!(m.find_all("Like, You know what I mean"), vec!["like", "you know"]);
    }

    #[test]
    fn matcher_english_boundary_with_cjk_neighbors() {
        // CJK 字符不构成边界阻挡（\w 只算 ASCII）：混合句里的英文口头禅照常命中
        let m = WordMatcher::new(["um"]);
        assert_eq!(m.find_all("然后 um 我们继续"), vec!["um"]);
    }

    #[test]
    fn matcher_phrase_with_apostrophe_and_spaces() {
        let m = WordMatcher::new(["don't", "you know", "kind of"]);
        assert_eq!(m.find_all("I don't know, you know?"), vec!["don't", "you know"]);
        assert_eq!(m.find_all("kind of works"), vec!["kind of"]);
        // 短语不被子词误报拆坏
        assert!(m.find_all("kindness of the team").is_empty());
    }

    #[test]
    fn matcher_longest_first_still_wins_for_english() {
        // sort of 先于 so（最长优先不变）；so 也不命中 sorted（边界）
        let m = WordMatcher::new(["so", "sort of"]);
        assert_eq!(m.find_all("it's sort of fine"), vec!["sort of"]);
        assert_eq!(m.find_all("sorted list, so we proceed"), vec!["so"]);
    }

    #[test]
    fn matcher_cjk_words_keep_substring_behavior() {
        // 含 CJK 的词条保持现行为（子串匹配、大小写敏感不受影响）
        let m = WordMatcher::new(["然后", "就是"]);
        assert_eq!(m.find_all("然后就是没问题"), vec!["然后", "就是"]);
    }

    // --- 英文词库加载与规模校验 ----------------------------------------------

    #[test]
    fn english_lexicon_parses_and_counts_match_meta() {
        let lex = builtin_lexicon_en();
        assert_eq!(lex.fillers.high.len() + lex.fillers.medium.len(), 36);
        assert_eq!(lex.hedges.len(), 27);
        assert_eq!(lex.vague_to_precise.len(), 76);
        // 与 _meta.counts 程序校验一致（README 维护纪律）
        assert_eq!(lex.meta["counts"]["fillers_high"], 17);
        assert_eq!(lex.meta["counts"]["fillers_medium"], 19);
        assert_eq!(lex.meta["counts"]["hedges"], 27);
        assert_eq!(lex.meta["counts"]["vagueToPrecise"], 76);
        // 自建声明与 MIT 授权在位
        assert_eq!(lex.meta["license"], "MIT");
    }

    #[test]
    fn english_lexicon_entries_are_pure_ascii_with_alternatives() {
        let lex = builtin_lexicon_en();
        // 全部词条应为纯 ASCII（边界匹配的前提；混入 CJK 属词库错误）
        let all: Vec<&str> = lex
            .fillers
            .high
            .iter()
            .chain(lex.fillers.medium.iter())
            .chain(lex.hedges.iter())
            .chain(lex.vague_to_precise.keys())
            .map(|s| s.as_str())
            .collect();
        assert!(all.iter().all(|w| w.is_ascii() && w.chars().any(|c| c.is_ascii_alphanumeric())));
        // 每组 vagueToPrecise 至少 2 个非空替代（质量底线，无同义反复凑数）
        for (k, v) in &lex.vague_to_precise {
            assert!(v.len() >= 2, "「{k}」替代词不足 2 个");
            assert!(v.iter().all(|a| !a.trim().is_empty() && a != k));
        }
        // high/medium 与 hedges 内部各自无重复词条
        let mut sorted_high = lex.fillers.high.clone();
        sorted_high.sort();
        sorted_high.dedup();
        assert_eq!(sorted_high.len(), lex.fillers.high.len());
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
