//! 词库自生长（产品方案 §5.2 维护流程的落地）。
//!
//! 会话落历史时统计逐字稿词频，提取"候选词"（中文词长 ≥2、跨会话累计 ≥3 次、
//! 且不在内置词库任何表与用户词库中），累计计数存 `appdata/lexicon-candidates.json`
//! （同词累加、按频次保留前 200 个）。用户在「设置 → 词库候选」审核：
//! - 加入词库：写入 `appdata/user-lexicon.json`（vagueToPrecise 条目；无替代词时
//!   视作自定义口头禅），并即时合并进全局生效词库（rules::lexicon）；
//! - 忽略：从候选文件移除。
//!
//! 纯函数（提取 / 聚合 / 已知词收集）与 IO 分离：IO 以目录为参数（可单测），
//! 命令层薄封装。任何失败静默降级，不影响练习主流程。

use crate::rules::lexicon::{self, LexiconV2, UserLexicon};
use crate::rules::Sentence;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub const CANDIDATES_FILE: &str = "lexicon-candidates.json";
pub const USER_LEXICON_FILE: &str = "user-lexicon.json";
/// 候选词进入列表展示的最低累计次数（跨会话累计）
pub const CANDIDATE_MIN_COUNT: u32 = 3;
/// 候选文件保留的最大词条数（按累计次数降序截断）
pub const CANDIDATES_CAP: usize = 200;
/// 本地降级报告结尾提示的候选词个数上限
pub const REPORT_HINT_WORDS: usize = 5;
/// 加入词库时单个词条允许的最多替代词数（与内置词库约定一致）
pub const MAX_ALTERNATIVES: usize = 6;
/// 加入词库时词条的最大长度（字符数）
pub const MAX_WORD_CHARS: usize = 12;

/// 候选词降噪内置停用词表：高频语法/功能词的相邻二字组（bigram）在
/// 词频统计里必然高频，但既非口头禅也非可替换的模糊词——候选提取与
/// 聚合时一律过滤（约 98 个；与内置口头禅词表互补，重叠无害）。
pub const CANDIDATE_STOPWORDS: &[&str] = &[
    // 代词 / 指示
    "我们", "你们", "他们", "自己", "别人", "大家",
    "这个", "那个", "这些", "那些", "这样", "那样",
    "什么", "怎么", "如何", "为什么",
    // 连词 / 副词
    "然后", "但是", "所以", "因为", "如果", "虽然", "而且", "以及",
    "或者", "还是", "就是", "也是", "都是", "还有", "甚至", "不过",
    "其实", "同时", "当然", "的话",
    "直接", "简单", "重要", "主要", "基本", "所有", "全部",
    "最后", "首先", "其次", "再次", "后来",
    // 时间
    "现在", "目前", "已经", "之后", "之前", "以前", "以后",
    // 心理 / 情态动词
    "觉得", "感觉", "认为", "知道", "希望", "喜欢", "愿意",
    "可以", "应该", "可能", "必须", "需要", "能够",
    "出来", "起来",
    // 介词 / 系动词化用法
    "进行", "通过", "对于", "关于", "根据", "随着", "作为", "成为", "变得", "显得",
    // 泛指名词（无替换价值）
    "东西", "事情", "问题", "方面", "地方", "情况", "部分", "过程",
    "方法", "方式", "时候", "时间",
    // 高频量词组
    "一个", "一下", "一些", "一点", "很多", "不少",
];

fn stopwords() -> &'static std::collections::HashSet<&'static str> {
    static SET: std::sync::OnceLock<std::collections::HashSet<&'static str>> =
        std::sync::OnceLock::new();
    SET.get_or_init(|| CANDIDATE_STOPWORDS.iter().copied().collect())
}

/// 是否内置停用词（候选提取/聚合的过滤表）
pub fn is_candidate_stopword(word: &str) -> bool {
    stopwords().contains(word)
}

/// 一条候选词：词 + 跨会话累计次数
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateEntry {
    pub word: String,
    pub count: u32,
}

/// 候选文件全量：候选表 + 上次已计数的会话 id（报告重试覆盖写同一历史记录，
/// 同 id 不重复计数）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_id: Option<String>,
    #[serde(default)]
    pub candidates: Vec<CandidateEntry>,
}

fn is_hanzi(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}')
}

// ---------------------------------------------------------------------------
// 纯函数：提取 / 聚合 / 已知词收集
// ---------------------------------------------------------------------------

/// 词频统计：CJK 相邻二字组（bigram）。没有分词模型时的标准近似——
/// 更长的词会以相邻二字片段浮出，最终由用户在设置里人工审核（宁杂不漏）。
pub fn count_chinese_bigrams(texts: &[&str]) -> BTreeMap<String, u32> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for text in texts {
        let mut run: Vec<char> = Vec::new();
        let flush = |run: &mut Vec<char>, counts: &mut BTreeMap<String, u32>| {
            for pair in run.windows(2) {
                let key: String = pair.iter().collect();
                *counts.entry(key).or_insert(0) += 1;
            }
            run.clear();
        };
        for c in text.chars() {
            if is_hanzi(c) {
                run.push(c);
            } else {
                flush(&mut run, &mut counts);
            }
        }
        flush(&mut run, &mut counts);
    }
    counts
}

/// 候选词增量（纯函数）：句子列表 + 已知词表 → 本次会话的词频，
/// 已收录词、单字（天然不构成 bigram）与内置停用词不进候选。
pub fn extract_candidate_counts(
    sentences: &[Sentence],
    known: &HashSet<String>,
) -> BTreeMap<String, u32> {
    let texts: Vec<&str> = sentences.iter().map(|s| s.text.as_str()).collect();
    count_chinese_bigrams(&texts)
        .into_iter()
        .filter(|(w, _)| !known.contains(w) && !is_candidate_stopword(w))
        .collect()
}

/// 聚合（纯函数）：已有累计 + 本次增量 → 新候选表。
/// 已收录词（用户中途加入词库）与内置停用词从累计中剔除；
/// 按次数降序（同次数按词序）取前 200。
pub fn aggregate_candidates(
    existing: &[CandidateEntry],
    delta: &BTreeMap<String, u32>,
    known: &HashSet<String>,
) -> Vec<CandidateEntry> {
    let drop_word = |w: &str| known.contains(w) || is_candidate_stopword(w);
    let mut map: HashMap<String, u32> = existing
        .iter()
        .filter(|e| !drop_word(&e.word))
        .map(|e| (e.word.clone(), e.count))
        .collect();
    for (word, count) in delta {
        if !drop_word(word) {
            *map.entry(word.clone()).or_insert(0) += count;
        }
    }
    let mut out: Vec<CandidateEntry> = map
        .into_iter()
        .map(|(word, count)| CandidateEntry { word, count })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.word.cmp(&b.word)));
    out.truncate(CANDIDATES_CAP);
    out
}

/// 内置词库全部表 + 用户词库的已收录词集合（候选提取的排除表）
pub fn known_words(lex: &LexiconV2, user: &UserLexicon) -> HashSet<String> {
    let mut set: HashSet<String> = HashSet::new();
    set.extend(lex.fillers.high.iter().chain(lex.fillers.medium.iter()).cloned());
    set.extend(lex.hedges.iter().cloned());
    set.extend(lex.vague_to_precise.keys().cloned());
    set.extend(lex.emotion_words.values().flat_map(|g| g.words.keys().cloned()));
    set.extend(lex.time_vague.keys().cloned());
    set.extend(user.vague_to_precise.keys().cloned());
    set.extend(user.fillers.iter().cloned());
    set
}

/// 达到展示阈值（累计 ≥ CANDIDATE_MIN_COUNT）的候选（文件已按频次降序）
pub fn visible_candidates(file: &CandidateFile) -> Vec<CandidateEntry> {
    file.candidates
        .iter()
        .filter(|c| c.count >= CANDIDATE_MIN_COUNT)
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// IO（目录参数化，可单测）
// ---------------------------------------------------------------------------

pub fn load_candidates_file(path: &Path) -> CandidateFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default()
}

pub fn save_candidates_file(path: &Path, file: &CandidateFile) -> Result<(), String> {
    write_json(path, &file, "候选词文件")
}

pub fn load_user_lexicon(path: &Path) -> UserLexicon {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default()
}

pub fn save_user_lexicon(path: &Path, user: &UserLexicon) -> Result<(), String> {
    write_json(path, &user, "用户词库文件")
}

fn write_json<T: Serialize>(path: &Path, value: &T, what: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    let body = serde_json::to_string_pretty(value).map_err(|e| format!("序列化{what}失败：{e}"))?;
    std::fs::write(path, body).map_err(|e| format!("写入{what}失败：{e}"))
}

/// 会话落历史时统计候选增量（目录参数化核心，可单测）。
/// 同一历史记录 id 只计一次（报告重试覆盖写不重复累计）。
pub fn record_candidates_at(dir: &Path, session_id: &str, sentences: &[Sentence]) -> Result<(), String> {
    let c_path = dir.join(CANDIDATES_FILE);
    let u_path = dir.join(USER_LEXICON_FILE);
    let mut file = load_candidates_file(&c_path);
    if file.last_session_id.as_deref() == Some(session_id) {
        return Ok(());
    }
    let known = known_words(&lexicon::lexicon(), &load_user_lexicon(&u_path));
    let delta = extract_candidate_counts(sentences, &known);
    file.candidates = aggregate_candidates(&file.candidates, &delta, &known);
    file.last_session_id = Some(session_id.to_string());
    save_candidates_file(&c_path, &file)
}

/// 加入词库（目录参数化核心）：有替代词 → vagueToPrecise 条目；无替代词 →
/// 自定义口头禅。同时从候选表移除该词。返回更新后的可见候选。
pub fn add_entry_at(
    dir: &Path,
    word: &str,
    alternatives: &[String],
) -> Result<Vec<CandidateEntry>, String> {
    let word = word.trim().to_string();
    if word.is_empty() {
        return Err("词条不能为空".into());
    }
    if word.chars().count() > MAX_WORD_CHARS {
        return Err(format!("词条过长（最多 {MAX_WORD_CHARS} 字）"));
    }
    let mut alternatives: Vec<String> = alternatives
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut seen = HashSet::new();
    alternatives.retain(|a| seen.insert(a.clone()));
    alternatives.truncate(MAX_ALTERNATIVES);

    let u_path = dir.join(USER_LEXICON_FILE);
    let mut user = load_user_lexicon(&u_path);
    if alternatives.is_empty() {
        if !user.fillers.contains(&word) {
            user.fillers.push(word.clone());
        }
    } else {
        user.vague_to_precise.insert(word.clone(), alternatives);
    }
    save_user_lexicon(&u_path, &user)?;

    let c_path = dir.join(CANDIDATES_FILE);
    let mut cfile = load_candidates_file(&c_path);
    cfile.candidates.retain(|c| c.word != word);
    save_candidates_file(&c_path, &cfile)?;
    Ok(visible_candidates(&cfile))
}

/// 忽略候选词（目录参数化核心）：从候选表移除。返回更新后的可见候选。
pub fn dismiss_at(dir: &Path, word: &str) -> Result<Vec<CandidateEntry>, String> {
    let word = word.trim().to_string();
    if word.is_empty() {
        return Err("词条不能为空".into());
    }
    let c_path = dir.join(CANDIDATES_FILE);
    let mut file = load_candidates_file(&c_path);
    file.candidates.retain(|c| c.word != word);
    save_candidates_file(&c_path, &file)?;
    Ok(visible_candidates(&file))
}

// ---------------------------------------------------------------------------
// 命令层（appdata 路径 + 全局词库即时生效）
// ---------------------------------------------------------------------------

pub fn appdata_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn candidates_path(app: &AppHandle) -> PathBuf {
    appdata_dir(app).join(CANDIDATES_FILE)
}

pub fn user_lexicon_path(app: &AppHandle) -> PathBuf {
    appdata_dir(app).join(USER_LEXICON_FILE)
}

/// 读取用户词库并应用进全局生效词库（文件缺失/为空 = 恢复纯内置）。
/// 启动、会话开始与加入词条后调用，保证规则与统计即时看到用户词。
pub fn apply_user_lexicon_from_disk(app: &AppHandle) {
    let user = load_user_lexicon(&user_lexicon_path(app));
    lexicon::apply_user_lexicon(Some(&user));
}

/// 会话落历史时调用（history::save_session_after_report 同机）：
/// 统计候选增量并累计；失败静默（不影响主流程）。
pub fn record_session_candidates(app: &AppHandle, session_id: &str, sentences: &[Sentence]) {
    if let Err(e) = record_candidates_at(&appdata_dir(app), session_id, sentences) {
        eprintln!("lexicon candidates update failed: {e}");
    }
}

/// 本地降级报告结尾提示用：达到阈值的候选词（最多 n 个，按频次降序）
pub fn top_candidate_words(app: &AppHandle, n: usize) -> Vec<String> {
    visible_candidates(&load_candidates_file(&candidates_path(app)))
        .into_iter()
        .take(n)
        .map(|c| c.word)
        .collect()
}

/// 候选词列表（设置页「词库候选」区）：只显示累计 ≥3 次的
#[tauri::command]
pub fn list_lexicon_candidates(app: AppHandle) -> Result<Vec<CandidateEntry>, String> {
    Ok(visible_candidates(&load_candidates_file(&candidates_path(&app))))
}

/// 加入词库：word + 替代词（无替代词时视作自定义口头禅）。
/// 写入 user-lexicon.json、从候选表移除、即时合并进生效词库；
/// 返回更新后的候选列表（前端直接刷新）。
#[tauri::command]
pub fn add_lexicon_entry(
    app: AppHandle,
    word: String,
    alternatives: Vec<String>,
) -> Result<Vec<CandidateEntry>, String> {
    let dir = appdata_dir(&app);
    let visible = add_entry_at(&dir, &word, &alternatives)?;
    // 即时生效（无需重启会话/应用）
    lexicon::apply_user_lexicon(Some(&load_user_lexicon(&dir.join(USER_LEXICON_FILE))));
    Ok(visible)
}

/// 忽略一个候选词：从候选文件移除，返回更新后的候选列表
#[tauri::command]
pub fn dismiss_lexicon_candidate(app: AppHandle, word: String) -> Result<Vec<CandidateEntry>, String> {
    dismiss_at(&appdata_dir(&app), &word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    fn test_dir() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("sm-growth-test-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- 提取（纯函数） ------------------------------------------------------

    #[test]
    fn bigram_counts_across_cjk_runs_only() {
        let counts = count_chinese_bigrams(&["我们对齐颗粒度", "let's go", "A 对齐 B"]);
        // "我们对齐颗粒度" 的全部相邻二字组
        let expect = ["我们", "们对", "齐颗", "颗粒", "粒度"];
        for w in expect {
            assert_eq!(counts.get(w), Some(&1), "{w}");
        }
        // 英文与单字不成词；"A 对齐 B" 只贡献 "对齐"（合计 2 次）
        assert_eq!(counts.get("对齐"), Some(&2));
        assert!(!counts.contains_key("t"));
    }

    #[test]
    fn extract_excludes_known_words() {
        let mut known = HashSet::new();
        known.insert("我们".to_string());
        known.insert("对齐".to_string());
        let sentences = vec![sent(1, "我们要对齐颗粒度"), sent(2, "先对齐再动手"), sent(3, "颗粒度要再细一点")];
        let delta = extract_candidate_counts(&sentences, &known);
        assert!(delta.get("颗粒") == Some(&2)); // 第 1、3 句各一次
        assert!(delta.get("粒度") == Some(&2));
        assert!(!delta.contains_key("我们"));
        assert!(!delta.contains_key("对齐"));
    }

    #[test]
    fn stopwords_filter_typical_grammar_words() {
        // 表内典型停用词全部命中
        for w in ["我们", "他们", "这个", "那个", "然后", "但是", "所以", "因为", "如果",
                  "就是", "的话", "自己", "现在", "时候", "东西", "事情", "问题", "方面",
                  "进行", "通过", "对于", "可以", "可能", "已经", "还是", "而且", "以及",
                  "或者", "方法", "方式", "时间", "很多"] {
            assert!(is_candidate_stopword(w), "{w} 应为停用词");
        }
        // 表外词不是停用词
        for w in ["颗粒", "粒度", "抓手", "底层", "闭环", "赋能"] {
            assert!(!is_candidate_stopword(w), "{w} 不应为停用词");
        }
        // 词表规模约定 60–100
        assert!(
            (60..=100).contains(&CANDIDATE_STOPWORDS.len()),
            "停用词表应在 60–100 个，实际 {}",
            CANDIDATE_STOPWORDS.len()
        );
        // 无重复词条
        let set: std::collections::HashSet<&str> = CANDIDATE_STOPWORDS.iter().copied().collect();
        assert_eq!(set.len(), CANDIDATE_STOPWORDS.len());
    }

    #[test]
    fn extract_filters_stopwords_from_typical_sentence() {
        // 典型练习句：语法词高频出现，但只有实义词该进候选
        let sentences = vec![
            sent(1, "我们这个项目的问题是通过很多数据进行训练"),
            sent(2, "但是这个方法可能会因为数据的事情出现偏差"),
        ];
        let delta = extract_candidate_counts(&sentences, &HashSet::new());
        for stop in ["我们", "这个", "但是", "可能", "因为", "很多", "进行", "通过", "问题", "方法"] {
            assert!(!delta.contains_key(stop), "停用词 {stop} 不应成为候选");
        }
        // 实义词（未收录）仍保留：数据 / 训练 / 偏差 / 出现
        assert!(delta.contains_key("数据"), "实义词应保留：{delta:?}");
        assert!(delta.contains_key("训练"));
        assert!(delta.contains_key("偏差"));
        assert!(delta.contains_key("出现"));
    }

    #[test]
    fn aggregate_drops_stopwords_from_existing_entries() {
        // 历史累计里遗留的停用词（旧版本文件）在聚合时被剔除
        let existing = vec![
            CandidateEntry { word: "我们".into(), count: 9 },
            CandidateEntry { word: "颗粒".into(), count: 4 },
        ];
        let mut delta = BTreeMap::new();
        delta.insert("这个".to_string(), 5);
        delta.insert("颗粒".to_string(), 2);
        let out = aggregate_candidates(&existing, &delta, &HashSet::new());
        assert_eq!(out, vec![CandidateEntry { word: "颗粒".into(), count: 6 }]);
    }

    // --- 聚合（纯函数） ------------------------------------------------------

    #[test]
    fn aggregate_accumulates_and_sorts_desc() {
        let existing = vec![
            CandidateEntry { word: "颗粒".into(), count: 2 },
            CandidateEntry { word: "底层".into(), count: 5 },
        ];
        let mut delta = BTreeMap::new();
        delta.insert("颗粒".to_string(), 2);
        delta.insert("抓手".to_string(), 3);
        let out = aggregate_candidates(&existing, &delta, &HashSet::new());
        // 底层 5、抓手 3、颗粒 4 → 底层、颗粒、抓手
        assert_eq!(
            out,
            vec![
                CandidateEntry { word: "底层".into(), count: 5 },
                CandidateEntry { word: "颗粒".into(), count: 4 },
                CandidateEntry { word: "抓手".into(), count: 3 },
            ]
        );
    }

    #[test]
    fn aggregate_drops_words_known_midway_and_caps_200() {
        let existing: Vec<CandidateEntry> = (0..250)
            .map(|i| CandidateEntry { word: format!("词{i:03}"), count: 1 })
            .collect();
        let mut delta = BTreeMap::new();
        delta.insert("词000".to_string(), 1); // 已在累计中
        let mut known = HashSet::new();
        known.insert("词249".to_string()); // 中途被收录 → 剔除
        let out = aggregate_candidates(&existing, &delta, &known);
        assert_eq!(out.len(), CANDIDATES_CAP);
        // 词000 = 1+1 = 2；被收录的词249 不在；其余 248 个 + 无新增 → 截断 200
        assert!(!out.iter().any(|c| c.word == "词249"));
        assert_eq!(out.iter().find(|c| c.word == "词000").unwrap().count, 2);
        // 全部 count=1 时按词序稳定排序（前 200 = 词000..词199 减去词249 的位置顺延）
        assert!(out.iter().all(|c| c.count >= 1));
    }

    #[test]
    fn visible_candidates_threshold_is_three() {
        let file = CandidateFile {
            last_session_id: None,
            candidates: vec![
                CandidateEntry { word: "甲".into(), count: 5 },
                CandidateEntry { word: "乙".into(), count: 3 },
                CandidateEntry { word: "丙".into(), count: 2 },
            ],
        };
        let visible: Vec<String> = visible_candidates(&file).into_iter().map(|c| c.word).collect();
        assert_eq!(visible, vec!["甲".to_string(), "乙".to_string()]);
    }

    // --- 文件 IO + 跨会话累计 ------------------------------------------------

    #[test]
    fn candidates_file_roundtrip_and_corrupt_falls_back() {
        let dir = test_dir();
        let file = CandidateFile {
            last_session_id: Some("2026-08-30-100000".into()),
            candidates: vec![CandidateEntry { word: "颗粒".into(), count: 4 }],
        };
        save_candidates_file(&dir.join(CANDIDATES_FILE), &file).unwrap();
        assert_eq!(load_candidates_file(&dir.join(CANDIDATES_FILE)), file);
        // 损坏/缺失 → 默认空
        std::fs::write(dir.join(CANDIDATES_FILE), "not json").unwrap();
        assert_eq!(load_candidates_file(&dir.join(CANDIDATES_FILE)), CandidateFile::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_accumulates_across_sessions_and_dedupes_retry() {
        let dir = test_dir();
        // 内置词库已收录「然后」：不应成为候选；「颗粒度」类词累计
        let s1 = vec![sent(1, "我们要对齐颗粒度然后推进"), sent(2, "颗粒度不对齐就没法推进")];
        record_candidates_at(&dir, "2026-08-30-100000", &s1).unwrap();
        let file = load_candidates_file(&dir.join(CANDIDATES_FILE));
        assert_eq!(file.last_session_id.as_deref(), Some("2026-08-30-100000"));
        let entry = file.candidates.iter().find(|c| c.word == "颗粒").unwrap().clone();
        assert_eq!(entry.count, 2);
        assert!(!file.candidates.iter().any(|c| c.word == "然后"));

        // 同一会话 id（报告重试）不重复计数
        record_candidates_at(&dir, "2026-08-30-100000", &s1).unwrap();
        let file = load_candidates_file(&dir.join(CANDIDATES_FILE));
        assert_eq!(file.candidates.iter().find(|c| c.word == "颗粒").unwrap().count, 2);

        // 新会话继续累计
        let s2 = vec![sent(1, "颗粒度是第一件事")];
        record_candidates_at(&dir, "2026-08-30-110000", &s2).unwrap();
        let file = load_candidates_file(&dir.join(CANDIDATES_FILE));
        assert_eq!(file.candidates.iter().find(|c| c.word == "颗粒").unwrap().count, 3);
        // 达到阈值 → 可见
        assert!(visible_candidates(&file).iter().any(|c| c.word == "颗粒"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_filters_words_already_in_user_lexicon() {
        let dir = test_dir();
        let mut user = UserLexicon::default();
        user.vague_to_precise.insert("颗粒".into(), vec!["粒度划分".into()]);
        save_user_lexicon(&dir.join(USER_LEXICON_FILE), &user).unwrap();
        let sentences = vec![sent(1, "对齐颗粒度再动手")];
        record_candidates_at(&dir, "2026-08-30-100000", &sentences).unwrap();
        let file = load_candidates_file(&dir.join(CANDIDATES_FILE));
        assert!(!file.candidates.iter().any(|c| c.word == "颗粒"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- 加入词库 / 忽略（目录参数化核心） -----------------------------------

    #[test]
    fn add_entry_with_alternatives_writes_user_lexicon_and_clears_candidate() {
        let dir = test_dir();
        let file = CandidateFile {
            last_session_id: None,
            candidates: vec![CandidateEntry { word: "颗粒".into(), count: 4 }],
        };
        save_candidates_file(&dir.join(CANDIDATES_FILE), &file).unwrap();

        let visible = add_entry_at(&dir, " 颗粒 ", &["粒度划分".into(), " ".into(), "粒度划分".into()]).unwrap();
        // 去空白、去重后写入用户词库
        let user = load_user_lexicon(&dir.join(USER_LEXICON_FILE));
        assert_eq!(user.vague_to_precise["颗粒"], vec!["粒度划分".to_string()]);
        assert!(user.fillers.is_empty());
        // 候选表已移除该词 → 可见列表为空
        assert!(visible.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_entry_without_alternatives_becomes_custom_filler() {
        let dir = test_dir();
        add_entry_at(&dir, "绝绝子", &[]).unwrap();
        let user = load_user_lexicon(&dir.join(USER_LEXICON_FILE));
        assert_eq!(user.fillers, vec!["绝绝子".to_string()]);
        assert!(user.vague_to_precise.is_empty());
        // 重复添加不重复写入
        add_entry_at(&dir, "绝绝子", &[]).unwrap();
        assert_eq!(load_user_lexicon(&dir.join(USER_LEXICON_FILE)).fillers.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_entry_validates_word_and_caps_alternatives() {
        let dir = test_dir();
        assert!(add_entry_at(&dir, "  ", &["替代".into()]).is_err());
        // 13 字超上限（最多 12 字）
        assert!(add_entry_at(&dir, "这是一个特别特别特别长的词条", &["替代".into()]).is_err());
        let alts: Vec<String> = (0..9).map(|i| format!("替代{i}")).collect();
        add_entry_at(&dir, "词条", &alts).unwrap();
        let user = load_user_lexicon(&dir.join(USER_LEXICON_FILE));
        assert_eq!(user.vague_to_precise["词条"].len(), MAX_ALTERNATIVES);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dismiss_removes_candidate_word() {
        let dir = test_dir();
        let file = CandidateFile {
            last_session_id: Some("s".into()),
            candidates: vec![
                CandidateEntry { word: "颗粒".into(), count: 4 },
                CandidateEntry { word: "抓手".into(), count: 3 },
            ],
        };
        save_candidates_file(&dir.join(CANDIDATES_FILE), &file).unwrap();
        let visible = dismiss_at(&dir, "颗粒").unwrap();
        assert_eq!(
            visible,
            vec![CandidateEntry { word: "抓手".into(), count: 3 }]
        );
        // 不存在的词：文件不变，返回当前可见列表
        assert_eq!(
            dismiss_at(&dir, "不存在的词").unwrap(),
            vec![CandidateEntry { word: "抓手".into(), count: 3 }]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
