//! 句子级语言检测（纯函数）：中文词库/规则与英文词库的路由地基。
//!
//! 判定口径：CJK 字符占「CJK + 拉丁字母」总数的比例（数字、标点、空白不参与）——
//! - 占比 ≥ 30% → 中文句（走中文词库与中文特有规则）；
//! - 占比 < 30% 且有字母 → 英文句（走英文词库；情绪词/时间模糊/画面感/金句跳过）；
//! - 一个字母都没有（纯数字/符号/空白）→ Neutral，词库类规则整体跳过。
//!
//! 混合句（两种文字并存）由 [`lexicon_routes`] 放行「两套词库都跑」——
//! 双语模型（如中英 Paraformer）的输出常混两种文字，两侧口头禅都该数。

/// CJK 占比阈值：≥ 该值视为中文句（词库路由 / 报告 prompt 选取共用）
pub const CJK_RATIO_THRESHOLD: f64 = 0.3;

/// 一句话（或一份逐字稿整体）的语言判定
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentenceLang {
    /// 中文（含中英混合且 CJK 占比达标）
    Chinese,
    /// 英文（有字母但 CJK 占比不达标）
    English,
    /// 纯数字/符号/空白：无字母内容，词库类规则跳过
    Neutral,
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}')
}

fn is_ascii_letter(c: char) -> bool {
    c.is_ascii_alphabetic()
}

/// 统计 (CJK 字符数, 拉丁字母数)
fn count_scripts(text: &str) -> (usize, usize) {
    let mut cjk = 0usize;
    let mut latin = 0usize;
    for c in text.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else if is_ascii_letter(c) {
            latin += 1;
        }
    }
    (cjk, latin)
}

/// CJK 字符占「CJK + 拉丁字母」的比例；无字母内容时为 0.0
pub fn cjk_ratio(text: &str) -> f64 {
    let (cjk, latin) = count_scripts(text);
    let denom = cjk + latin;
    if denom == 0 {
        0.0
    } else {
        cjk as f64 / denom as f64
    }
}

pub fn has_cjk(text: &str) -> bool {
    text.chars().any(is_cjk)
}

pub fn has_ascii_letter(text: &str) -> bool {
    text.chars().any(is_ascii_letter)
}

/// 句子语言判定（纯函数）
pub fn detect_lang(text: &str) -> SentenceLang {
    let (cjk, latin) = count_scripts(text);
    match cjk + latin {
        0 => SentenceLang::Neutral,
        n if cjk as f64 / n as f64 >= CJK_RATIO_THRESHOLD => SentenceLang::Chinese,
        _ => SentenceLang::English,
    }
}

/// 词库类规则（口头禅/词汇精确度/立场模糊）的路由：(跑中文词库, 跑英文词库)。
/// 纯数字/符号句两套都跳过；混合句（两种文字并存）两套都跑——两套词表文字
/// 不相交（中文词全 CJK、英文词全 ASCII），并跑不会重复计数。
pub fn lexicon_routes(text: &str) -> (bool, bool) {
    (has_cjk(text), has_ascii_letter(text))
}

/// 逐字稿整体语言（报告 prompt 选取）：全部句子拼接后按同一阈值判定；
/// 无任何字母内容（含空逐字稿）按中文处理（保持既有行为）。
pub fn detect_transcript_lang(sentences: &[super::Sentence]) -> SentenceLang {
    let mut cjk = 0usize;
    let mut latin = 0usize;
    for s in sentences {
        let (c, l) = count_scripts(&s.text);
        cjk += c;
        latin += l;
    }
    match cjk + latin {
        0 => SentenceLang::Chinese,
        n if cjk as f64 / n as f64 >= CJK_RATIO_THRESHOLD => SentenceLang::Chinese,
        _ => SentenceLang::English,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_sentence_detected() {
        assert_eq!(detect_lang("今天介绍产品设计的三个要点"), SentenceLang::Chinese);
        // 中英混合但 CJK 占主导
        assert_eq!(detect_lang("然后用 React 做前端界面"), SentenceLang::Chinese);
    }

    #[test]
    fn english_sentence_detected() {
        assert_eq!(detect_lang("I think this is a great idea"), SentenceLang::English);
        // 少量中文夹杂在英文里：占比不足 30%
        assert_eq!(
            detect_lang("We shipped the feature last week, basically on time, 顺便修了 bug"),
            SentenceLang::English
        );
    }

    #[test]
    fn neutral_sentence_has_no_letters() {
        assert_eq!(detect_lang("123 456"), SentenceLang::Neutral);
        assert_eq!(detect_lang("……？！"), SentenceLang::Neutral);
        assert_eq!(detect_lang("   "), SentenceLang::Neutral);
        assert_eq!(detect_lang(""), SentenceLang::Neutral);
        assert_eq!(cjk_ratio(""), 0.0);
    }

    #[test]
    fn ratio_boundary_at_threshold() {
        // 3 CJK + 7 拉丁 = 恰好 30%：达标走中文
        let text = "中文句 abcdefg";
        let (cjk, latin) = count_scripts(text);
        assert_eq!((cjk, latin), (3, 7));
        assert!((cjk_ratio(text) - 0.3).abs() < 1e-9);
        assert_eq!(detect_lang(text), SentenceLang::Chinese);
        // 2 CJK + 7 拉丁 ≈ 22% < 30%：走英文
        assert_eq!(detect_lang("中文 abcdefg"), SentenceLang::English);
    }

    #[test]
    fn lexicon_routes_skip_pure_symbols_and_run_both_for_mixed() {
        // 纯中文：只跑中文
        assert_eq!(lexicon_routes("然后那个就是这样"), (true, false));
        // 纯英文：只跑英文
        assert_eq!(lexicon_routes("you know, like, basically"), (false, true));
        // 混合句：两套都跑（双语模型输出常混）
        assert_eq!(lexicon_routes("然后 um 我们 continue 吧"), (true, true));
        // 纯数字/符号：都跳过
        assert_eq!(lexicon_routes("666 666"), (false, false));
    }

    #[test]
    fn transcript_lang_aggregates_across_sentences() {
        let s = |id: u64, text: &str| super::super::Sentence {
            id,
            text: text.into(),
            start_ms: 0,
            end_ms: 0,
        };
        // 整体中文逐字稿
        assert_eq!(
            detect_transcript_lang(&[s(1, "第一句中文"), s(2, "第二句也是中文")]),
            SentenceLang::Chinese
        );
        // 整体英文逐字稿（单句也英文）
        assert_eq!(
            detect_transcript_lang(&[s(1, "First sentence"), s(2, "Second one here")]),
            SentenceLang::English
        );
        // 空逐字稿 / 无字母：按中文（保持旧行为）
        assert_eq!(detect_transcript_lang(&[]), SentenceLang::Chinese);
        assert_eq!(
            detect_transcript_lang(&[s(1, "123 456")]),
            SentenceLang::Chinese
        );
    }
}
