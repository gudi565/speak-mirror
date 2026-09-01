use super::imagery::IMAGERY_MARKERS;
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};

/// 每会话最多提示的金句句数（宁缺毋滥：宁可漏掉，不可刷屏）
pub const GOLDEN_QUOTE_MAX_PER_SESSION: usize = 3;

/// 条件④的句长区间（非空白字符数，含标点）
pub const GOLDEN_QUOTE_MIN_CHARS: usize = 12;
pub const GOLDEN_QUOTE_MAX_CHARS: usize = 30;

/// 条件②的"结论词"：与数字连用才构成信号
const CONCLUSION_MARKERS: &[&str] = &["就", "才", "能", "会", "等于"];

/// 条件④的强调结构对子：(前段, 后段)，两段都出现才算
const EMPHASIS_PAIRS: &[(&str, &str)] = &[("不是", "而是"), ("越", "越"), ("只有", "才")];

/// 汉字数字（口语里 "三个""百分之三十" 都算数字）
const HAN_DIGITS: &[char] = &[
    '一', '二', '两', '三', '四', '五', '六', '七', '八', '九', '十', '百', '千', '万', '亿', '零',
];

fn is_hanzi(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}')
}

fn is_digit_char(c: char) -> bool {
    c.is_ascii_digit() || HAN_DIGITS.contains(&c)
}

/// 条件①：含比喻标记（就像/好比/相当于/仿佛/如同）
fn has_metaphor_marker(text: &str) -> bool {
    IMAGERY_MARKERS.iter().any(|m| text.contains(m))
}

/// 条件②：含数字 + 结论词（就/才/能/会/等于）
fn has_number_and_conclusion(text: &str) -> bool {
    text.chars().any(is_digit_char) && CONCLUSION_MARKERS.iter().any(|m| text.contains(m))
}

/// 条件③：两个及以上"四字格"——按非汉字（标点/空格/字母）切段后，
/// 恰好 4 个汉字的连续段（简单启发式：脚踏实实地、一目了然）。
fn four_char_segment_count(text: &str) -> usize {
    let mut count = 0usize;
    let mut run_len = 0usize;
    for c in text.chars().chain(std::iter::once('\0')) {
        if is_hanzi(c) {
            run_len += 1;
        } else {
            if run_len == 4 {
                count += 1;
            }
            run_len = 0;
        }
    }
    count
}

/// 条件④：句长 12–30 字（非空白字符）且含强调结构（"不是…而是""越…越""只有…才"）
fn length_ok_with_emphasis(text: &str) -> bool {
    let len = text.chars().filter(|c| !c.is_whitespace()).count();
    (GOLDEN_QUOTE_MIN_CHARS..=GOLDEN_QUOTE_MAX_CHARS).contains(&len)
        && EMPHASIS_PAIRS.iter().any(|(a, b)| text.contains(a) && text.contains(b))
}

/// 命中的条件短名（正向展示用）
pub fn golden_quote_reasons(text: &str) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if has_metaphor_marker(text) {
        reasons.push("比喻");
    }
    if has_number_and_conclusion(text) {
        reasons.push("数字结论");
    }
    if four_char_segment_count(text) >= 2 {
        reasons.push("四字对仗");
    }
    if length_ok_with_emphasis(text) {
        reasons.push("强调结构");
    }
    reasons
}

/// 金句捕捉规则（正向）：一句同时满足 ≥2 个启发式信号即候选——
/// ①比喻标记 ②数字+结论词 ③两个及以上四字格 ④句长 12–30 且含强调结构。
/// 每会话最多提示 3 句（规则内部上限）；句间冷却由引擎统一节流（同类 3 句冷却）。
pub struct GoldenQuoteRule {
    produced: usize,
}

impl GoldenQuoteRule {
    pub fn new() -> Self {
        Self { produced: 0 }
    }
}

impl Default for GoldenQuoteRule {
    fn default() -> Self {
        Self::new()
    }
}

impl Rule for GoldenQuoteRule {
    fn on_sentence(&mut self, sentence: &Sentence, _ctx: &SessionContext) -> Vec<FeedbackEvent> {
        if self.produced >= GOLDEN_QUOTE_MAX_PER_SESSION {
            return Vec::new();
        }
        let reasons = golden_quote_reasons(&sentence.text);
        if reasons.len() < 2 {
            return Vec::new();
        }
        self.produced += 1;
        vec![FeedbackEvent {
            kind: FeedbackKind::GoldenQuote,
            sentence_id: Some(sentence.id),
            message: format!("金句信号：{}——这句值得保留复用", reasons.join("+")),
            payload: serde_json::json!({ "reasons": reasons, "quote": sentence.text }),
        }]
    }

    fn name(&self) -> &'static str {
        "golden_quote"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    fn run(rule: &mut GoldenQuoteRule, text: &str) -> Vec<FeedbackEvent> {
        let ctx = SessionContext::default();
        rule.on_sentence(&sent(1, text), &ctx)
    }

    // --- 四条触发路径（每句至少两条信号；每个用例让一条"目标信号"起决定作用） ---

    #[test]
    fn metaphor_plus_number_conclusion_triggers() {
        let mut rule = GoldenQuoteRule::new();
        let events = run(&mut rule, "这就像把 3 个月的工作压缩到 3 周");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, FeedbackKind::GoldenQuote);
        let reasons = events[0].payload["reasons"].as_array().unwrap();
        assert!(reasons.contains(&serde_json::json!("比喻")));
        assert!(reasons.contains(&serde_json::json!("数字结论")));
    }

    #[test]
    fn number_conclusion_plus_emphasis_triggers() {
        let mut rule = GoldenQuoteRule::new();
        // 数字(1) + 结论词(就/能)；句长 14 字 + "不是…而是"
        let events = run(&mut rule, "1 页纸就能讲清，不是写多而是写少");
        assert_eq!(events.len(), 1);
        let reasons = events[0].payload["reasons"].as_array().unwrap();
        assert!(reasons.contains(&serde_json::json!("数字结论")));
        assert!(reasons.contains(&serde_json::json!("强调结构")));
    }

    #[test]
    fn four_char_pairs_plus_number_triggers() {
        let mut rule = GoldenQuoteRule::new();
        // 三个四字格（季度复盘/纲举目张/事半功倍，标点切段）+ 数字(3)+结论词(就)
        let events = run(&mut rule, "季度复盘：纲举目张、事半功倍，3 天就出结论");
        assert_eq!(events.len(), 1);
        let reasons = events[0].payload["reasons"].as_array().unwrap();
        assert!(reasons.contains(&serde_json::json!("四字对仗")));
        assert!(reasons.contains(&serde_json::json!("数字结论")));
    }

    #[test]
    fn metaphor_plus_four_char_pairs_triggers() {
        let mut rule = GoldenQuoteRule::new();
        // 比喻(就像) + 两个四字格（一目了然/毫不留情）
        let events = run(&mut rule, "写周报就像照镜子，一目了然、毫不留情");
        assert_eq!(events.len(), 1);
        let reasons = events[0].payload["reasons"].as_array().unwrap();
        assert!(reasons.contains(&serde_json::json!("比喻")));
        assert!(reasons.contains(&serde_json::json!("四字对仗")));
    }

    // --- 单信号不触发（宁缺毋滥） ---

    #[test]
    fn metaphor_alone_does_not_trigger() {
        let mut rule = GoldenQuoteRule::new();
        assert!(run(&mut rule, "这个方案就像之前讨论过的那样").is_empty());
    }

    #[test]
    fn number_conclusion_alone_does_not_trigger() {
        let mut rule = GoldenQuoteRule::new();
        // 有数字+就，但句长 10 字、无四字格、无比喻
        assert!(run(&mut rule, "我们 3 天就完成了开发").is_empty());
    }

    #[test]
    fn four_char_pairs_alone_do_not_trigger() {
        let mut rule = GoldenQuoteRule::new();
        // 两个四字格，但无数字、无比喻、无强调结构
        assert!(run(&mut rule, "脚踏实地、埋头苦干是我们的底色").is_empty());
    }

    #[test]
    fn emphasis_alone_does_not_trigger() {
        let mut rule = GoldenQuoteRule::new();
        // "不是…而是" + 句长 13 字，但无其他信号
        assert!(run(&mut rule, "这件事不是我不懂，而是我不想").is_empty());
    }

    #[test]
    fn plain_sentence_no_event() {
        let mut rule = GoldenQuoteRule::new();
        assert!(run(&mut rule, "然后我们进入下一项议题的讨论").is_empty());
    }

    // --- 上限 ---

    #[test]
    fn at_most_three_quotes_per_session() {
        let mut rule = GoldenQuoteRule::new();
        let golden = "这就像把 3 个月的工作压缩到 3 周";
        let ctx = SessionContext::default();
        for i in 1..=3 {
            assert_eq!(rule.on_sentence(&sent(i, golden), &ctx).len(), 1);
        }
        // 第 4 句即使命中也不再产出
        assert!(rule.on_sentence(&sent(4, golden), &ctx).is_empty());
        assert!(rule.on_sentence(&sent(5, golden), &ctx).is_empty());
    }

    #[test]
    fn reasons_helper_counts_conditions() {
        let r = golden_quote_reasons("这就像把 3 个月的工作压缩到 3 周");
        assert_eq!(r, vec!["比喻", "数字结论"]);
        assert!(golden_quote_reasons("普通的一句话").is_empty());
    }
}
