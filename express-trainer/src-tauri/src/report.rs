//! 报告层：终稿逐字稿获取、AI 报告（OpenAI 兼容流式）、本地降级报告、连接测试。
//!
//! 事件契约（前端 useReport 监听）：
//! - `report_chunk` `{ text }`   流式增量 Markdown 片段
//! - `report_done`   `{ text }`  报告完成，text 为全文
//! - `report_error`  `{ message }` 生成失败

use crate::history::{self, PreviousSession};
use crate::rules::engine::SessionSnapshot;
use crate::rules::{FeedbackEvent, FeedbackKind, Sentence};
use crate::settings::Settings;
use crate::voice::{self, VoiceReport};
use crate::AppState;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

pub const FREE_PROMPT: &str = include_str!("../prompts/free.md");
pub const INTERVIEW_PROMPT: &str = include_str!("../prompts/interview.md");
pub const VLOG_PROMPT: &str = include_str!("../prompts/vlog.md");
pub const WORKREPORT_PROMPT: &str = include_str!("../prompts/workreport.md");
pub const MOCK_INTERVIEW_PROMPT: &str = include_str!("../prompts/mock_interview.md");
/// 快速报告 prompt（四节 ≤400 字）：输入 user JSON 与完整版完全一致，
/// 评分维度沿用当前场景，仅输出长度与节结构不同
pub const QUICK_PROMPT: &str = include_str!("../prompts/quick.md");

/// 报告支持的全部场景（与前端 types.ts Scenario 同步；settings.rs 的 SCENARIOS
/// 仍为四项——mockInterview 只在模拟面试流程内部使用，不进普通场景下拉/设置）
pub const SCENARIOS: &[&str] =
    &["free", "interview", "vlog", "workreport", "mockInterview"];

/// 报告模式（0.2.3）：full = 完整版场景 prompt（八节）；
/// quick = quick.md（四节 ≤400 字，约 1/3 生成时间）
pub const REPORT_MODES: &[&str] = &["full", "quick"];

/// 快速模式的 max_tokens 上限：四节 ≤400 字 + 评分标记折 token 留足余量，
/// 防模型偶尔超长拖慢出稿；截断风险极低（完整版不设上限，行为不变）
pub const QUICK_MAX_TOKENS: u32 = 2_000;

// ---------------------------------------------------------------------------
// Prompt / 模式选择
// ---------------------------------------------------------------------------

pub fn system_prompt_for(scenario: &str) -> Result<&'static str, String> {
    match scenario {
        "free" => Ok(FREE_PROMPT),
        "interview" => Ok(INTERVIEW_PROMPT),
        "vlog" => Ok(VLOG_PROMPT),
        "workreport" => Ok(WORKREPORT_PROMPT),
        "mockInterview" => Ok(MOCK_INTERVIEW_PROMPT),
        other => Err(format!(
            "未知场景：{other}（支持 {}）",
            SCENARIOS.join(" / ")
        )),
    }
}

/// 按模式选 system prompt（纯函数可测）：quick 一律用 quick.md（其内部
/// 按输入 JSON 自行对齐当前场景的评分维度）；full 走场景 prompt。
/// 场景合法性两种模式都校验（历史落盘口径一致）。
pub fn system_prompt_for_mode(scenario: &str, mode: &str) -> Result<&'static str, String> {
    system_prompt_for(scenario)?; // quick 模式也先校验场景（含错误信息统一）
    match mode {
        "quick" => Ok(QUICK_PROMPT),
        "full" => system_prompt_for(scenario),
        other => Err(format!(
            "未知报告模式：{other}（支持 {}）",
            REPORT_MODES.join(" / ")
        )),
    }
}

/// 报告模式解析（纯函数可测）：None / 空白 → "full"（旧前端兼容缺省）；
/// 白名单外的值报错（前端传错立刻暴露，不静默降级成另一种模式）
pub fn parse_report_mode(mode: Option<&str>) -> Result<&'static str, String> {
    match mode.map(str::trim).filter(|m| !m.is_empty()).as_deref() {
        None => Ok("full"),
        Some("full") => Ok("full"),
        Some("quick") => Ok("quick"),
        Some(other) => Err(format!(
            "未知报告模式：{other}（支持 {}）",
            REPORT_MODES.join(" / ")
        )),
    }
}

/// 场景中文名（本地降级报告与历史列表用；未知值回落"自由练习"）
pub fn scenario_label(scenario: &str) -> &'static str {
    match scenario {
        "interview" => "面试回答",
        "vlog" => "口播视频",
        "workreport" => "工作汇报",
        "mockInterview" => "模拟面试",
        _ => "自由练习",
    }
}

// ---------------------------------------------------------------------------
// 逐字稿
// ---------------------------------------------------------------------------

/// 用户在总结页修正后的逐字稿纯文本（一行一句）→ 句子数组。
/// id 即行号，供报告「#id」引用锚定；时间戳未知置 0。
pub fn parse_transcript_text(text: &str) -> Vec<Sentence> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .enumerate()
        .map(|(i, l)| Sentence {
            id: i as u64 + 1,
            text: l.to_string(),
            start_ms: 0,
            end_ms: 0,
        })
        .collect()
}

pub fn transcript_text(sentences: &[Sentence]) -> String {
    sentences
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 非空白字符数（统计口径与快照一致）
pub fn count_chars(sentences: &[Sentence]) -> u64 {
    sentences
        .iter()
        .flat_map(|s| s.text.chars())
        .filter(|c| !c.is_whitespace())
        .count() as u64
}

// ---------------------------------------------------------------------------
// user 消息 / 请求体构建
// ---------------------------------------------------------------------------

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// 组 stats 对象（字段缺失 = 未知，prompt 明确允许）。
/// filler_counts 来自会话快照；total_chars / sentence_count 以实际送审的逐字稿为准；
/// golden_quote_count 为金句候选句数（>0 才写入，供报告 prompt 引用）。
pub fn build_stats(
    sentences: &[Sentence],
    duration_ms: u64,
    filler_counts: &[(String, u32)],
    golden_quote_count: u32,
) -> Value {
    let total_chars = count_chars(sentences);
    let minutes = duration_ms as f64 / 60_000.0;
    let mut stats = json!({
        "duration_sec": duration_ms / 1000,
        "total_chars": total_chars,
        "sentence_count": sentences.len(),
    });
    if golden_quote_count > 0 {
        stats["goldenQuoteCount"] = json!(golden_quote_count);
    }
    if minutes > 0.0 {
        stats["speech_rate"] = json!(round1(total_chars as f64 / minutes));
    }
    if !filler_counts.is_empty() {
        let fillers: Vec<Value> = filler_counts
            .iter()
            .map(|(word, count)| {
                let mut f = json!({ "word": word, "count": count });
                if minutes > 0.0 {
                    f["per_minute"] = json!(round1(*count as f64 / minutes));
                }
                f
            })
            .collect();
        stats["fillers"] = json!(fillers);
    }
    stats
}

/// AI 报告 stats 注入声调偏差计数（纯函数可测）：有标记时写入
/// toneFlagCount（>0 才写入，字段缺失 = 未分析/无发现，prompt 允许）。
pub fn apply_tone_stats(stats: &mut Value, tone_flags: &[crate::tone::ToneFlag]) {
    if !tone_flags.is_empty() {
        stats["toneFlagCount"] = json!(tone_flags.len() as u64);
    }
}

/// 组 user 消息 JSON（snake_case 键名，与 prompt 文档模板一致；自由练习用 topic、面试用 question）。
/// previous = 上一次会话摘要（无历史时省略）。
pub fn build_user_payload(
    scenario: &str,
    topic: Option<&str>,
    sentences: &[Sentence],
    stats: Value,
    previous: Option<&PreviousSession>,
) -> Value {
    let transcript: Vec<Value> = sentences
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "text": s.text,
                "start_ms": s.start_ms,
                "end_ms": s.end_ms,
            })
        })
        .collect();
    let mut payload = json!({ "transcript": transcript, "stats": stats });
    if let Some(t) = topic.map(str::trim).filter(|t| !t.is_empty()) {
        let key = if scenario == "interview" { "question" } else { "topic" };
        payload[key] = json!(t);
    }
    if let Some(p) = previous {
        payload["previous"] = serde_json::to_value(p).unwrap_or(Value::Null);
    }
    payload
}

/// /chat/completions 请求体（OpenAI 兼容，流式）。max_tokens：None = 不设
/// 上限（完整版，行为不变）；Some = 上限（快速模式调小，见 QUICK_MAX_TOKENS）
pub fn build_request_body(
    model: &str,
    system: &str,
    user_content: &str,
    max_tokens: Option<u32>,
) -> Value {
    let mut body = json!({
        "model": model,
        "stream": true,
        "temperature": 0.4,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user_content },
        ],
    });
    if let Some(n) = max_tokens {
        body["max_tokens"] = json!(n);
    }
    body
}

// ---------------------------------------------------------------------------
// 模拟面试（mockInterview）：逐题问答注入
// ---------------------------------------------------------------------------

/// 一道模拟面试题的用户作答（前端逐题累积、可编辑后传入）
#[derive(Debug, Clone, serde::Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QaItem {
    pub question: String,
    pub intent: String,
    /// 用户（修正后）的回答逐字稿，一行一句；空串 = 未作答/跳过
    pub answer: String,
    /// 该题回答时长（毫秒，来自该题会话快照；缺失/0 = 未知）
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// 多题回答合并为一份逐字稿（非空行，全场连续编号——parse_transcript_text
/// 每次从 1 起编号，这里统一重排）。返回 (句子, 各题行数)。
pub fn merge_qa_answers(qa: &[QaItem]) -> (Vec<Sentence>, Vec<usize>) {
    let mut sentences = Vec::new();
    let mut counts = Vec::with_capacity(qa.len());
    for item in qa {
        let parsed = parse_transcript_text(&item.answer);
        counts.push(parsed.len());
        sentences.extend(parsed);
    }
    for (i, s) in sentences.iter_mut().enumerate() {
        s.id = i as u64 + 1;
    }
    (sentences, counts)
}

/// 模拟面试报告的 user 消息 JSON：qa 数组带题号/题目/考察点/句子区间/时长，
/// transcript 为全场合并（id 连续编号，供 prompt「#句id」引用锚定）。
pub fn build_mock_interview_payload(
    role: Option<&str>,
    qa: &[QaItem],
    stats: Value,
    previous: Option<&PreviousSession>,
) -> Value {
    let mut id = 0u64;
    let mut qa_json = Vec::with_capacity(qa.len());
    for (i, item) in qa.iter().enumerate() {
        let line_count = parse_transcript_text(&item.answer).len();
        let (first, last) = if line_count > 0 {
            (id + 1, id + line_count as u64)
        } else {
            (0, 0)
        };
        id += line_count as u64;
        let mut entry = json!({
            "index": i + 1,
            "question": item.question,
            "intent": item.intent,
            "skipped": line_count == 0,
        });
        if line_count > 0 {
            entry["sentence_ids"] = json!([first, last]);
            if let Some(ms) = item.duration_ms.filter(|ms| *ms > 0) {
                entry["duration_sec"] = json!(ms / 1000);
            }
        }
        qa_json.push(entry);
    }
    let (sentences, _) = merge_qa_answers(qa);
    let transcript: Vec<Value> = sentences
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "text": s.text,
                "start_ms": s.start_ms,
                "end_ms": s.end_ms,
            })
        })
        .collect();
    let mut payload = json!({ "qa": qa_json, "transcript": transcript, "stats": stats });
    if let Some(r) = role.map(str::trim).filter(|r| !r.is_empty()) {
        payload["role"] = json!(r);
    }
    if let Some(p) = previous {
        payload["previous"] = serde_json::to_value(p).unwrap_or(Value::Null);
    }
    payload
}

/// 全场时长（毫秒）：各题回答时长之和（无时长信息的题为 0）
pub fn qa_total_duration_ms(qa: &[QaItem]) -> u64 {
    qa.iter().filter_map(|q| q.duration_ms).sum()
}

/// 合并快照（mockInterview 专用）：对全场合并逐字稿重放规则引擎得到统计，
/// 再用各题时长之和重算与时长相关的派生指标（语速 / 口头禅频率）。
pub fn merged_mock_snapshot(sentences: &[Sentence], duration_ms: u64) -> SessionSnapshot {
    let mut engine = crate::rules::engine::RuleEngine::new();
    engine.start(0);
    for s in sentences {
        engine.ingest(s.clone());
    }
    let mut snap = engine.snapshot();
    snap.duration_ms = duration_ms;
    let minutes = duration_ms as f64 / 60_000.0;
    if minutes > 0.0 {
        let total_fillers: u32 = snap.filler_counts.iter().map(|(_, c)| c).sum();
        snap.speech_rate = round1(snap.total_chars as f64 / minutes);
        snap.filler_per_minute = round1(total_fillers as f64 / minutes);
    }
    snap
}

fn chat_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

// ---------------------------------------------------------------------------
// 评分标记（B：报告入档）
// ---------------------------------------------------------------------------

/// 报告末尾评分标记的前缀（五份报告 prompt 统一约定）
pub const SCORE_MARKER_PREFIX: &str = "<!--SCORE:";

/// 解析报告末尾的评分标记：`<!--SCORE:{"overall":0-100,"维度":分,…}-->`。
/// 取首个标记；容忍缺失 / 畸形 JSON / 非 object —— 一律返回 None 不报错。
pub fn parse_score_marker(text: &str) -> Option<Value> {
    let start = text.find(SCORE_MARKER_PREFIX)?;
    let rest = &text[start + SCORE_MARKER_PREFIX.len()..];
    let end = rest.find("-->")?;
    let inner = rest[..end].trim();
    let v: Value = serde_json::from_str(inner).ok()?;
    if v.is_object() { Some(v) } else { None }
}

/// 从报告正文里移除评分标记行（展示与导出用干净文本；scores 已单独立档）
pub fn strip_score_marker(text: &str) -> String {
    let Some(start) = text.find(SCORE_MARKER_PREFIX) else {
        return text.to_string();
    };
    let Some(len) = text[start..].find("-->").map(|i| i + "-->".len()) else {
        // 有前缀无结尾（流式中途）：整个尾部裁掉
        return text[..start].trim_end().to_string();
    };
    let mut out = format!("{}{}", &text[..start], &text[start + len..]);
    // 标记独占一行时把残留的空行收掉
    out = out.replace("\n\n\n", "\n\n");
    out.trim_end().to_string() + "\n"
}

// ---------------------------------------------------------------------------
// SSE 解析
// ---------------------------------------------------------------------------

/// 增量 SSE 解析器：容忍任意字节边界切割（含 UTF-8 多字节字符被劈开的情况）。
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// 喂入一块响应字节，返回其中已完整行的 data 负载（跳过 [DONE] 与注释行）
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = &line[..line.len() - 1]; // 去掉 \n
            let line = if line.last() == Some(&b'\r') {
                &line[..line.len() - 1]
            } else {
                line
            };
            if let Some(data) = line.strip_prefix(b"data:") {
                let data = String::from_utf8_lossy(data).trim().to_string();
                if !data.is_empty() && data != "[DONE]" {
                    out.push(data);
                }
            }
        }
        out
    }
}

/// 从一条 SSE data JSON 中取出 choices[0].delta.content
pub fn extract_delta(data: &str) -> Option<String> {
    let v: Value = serde_json::from_str(data).ok()?;
    v.get("choices")?
        .get(0)?
        .get("delta")?
        .get("content")?
        .as_str()
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// 本地降级报告
// ---------------------------------------------------------------------------

/// 对送审逐字稿重放规则引擎，收集全部规则事件（规则是确定性的，重放结果与实时一致）
pub fn collect_rule_events(sentences: &[Sentence]) -> Vec<FeedbackEvent> {
    let mut engine = crate::rules::engine::RuleEngine::new();
    engine.start(0);
    sentences
        .iter()
        .cloned()
        .flat_map(|s| engine.ingest(s))
        .collect()
}

fn kind_label(kind: &FeedbackKind) -> &'static str {
    match kind {
        FeedbackKind::FillerWord => "口头禅",
        FeedbackKind::WordPrecision => "词汇精确度",
        FeedbackKind::Repetition => "重复",
        FeedbackKind::ConclusionMissing => "结论缺失",
        FeedbackKind::ExampleMissing => "缺举例",
        FeedbackKind::Emotion => "情感词",
        FeedbackKind::Hedge => "立场模糊",
        FeedbackKind::TimeVague => "时间模糊",
        FeedbackKind::Imagery => "画面感",
        FeedbackKind::GoldenQuote => "金句",
        FeedbackKind::AiCheckin => "AI 快评",
    }
}

fn format_duration(duration_ms: u64) -> String {
    let total_sec = duration_ms / 1000;
    let (m, s) = (total_sec / 60, total_sec % 60);
    if m > 0 {
        format!("{m} 分 {s:02} 秒")
    } else {
        format!("{s} 秒")
    }
}

/// 本地降级报告的"怎么说的（声音）"小节；无声音数据返回空串
fn local_voice_section(voice_json: &Value) -> String {
    if !voice_json.is_object() {
        return String::new();
    }
    let mut md = String::from("## 怎么说的（声音）\n\n");
    let pauses = voice_json["pause_count"].as_u64().unwrap_or(0);
    let longest = voice_json["longest_pause_sec"].as_f64().unwrap_or(0.0);
    md.push_str(&format!("- 失控停顿：{pauses} 次"));
    if longest > 0.0 {
        md.push_str(&format!("（静音超 2 秒计，最长 {longest} 秒）"));
    }
    md.push('\n');
    if let Some(dr) = voice_json["volume_dynamic_range_db"].as_f64() {
        md.push_str(&format!("- 音量动态范围：{dr} dB（会话内相对值）\n"));
    }
    if let Some(st) = voice_json["energy_stability"].as_f64() {
        md.push_str(&format!("- 能量稳定性：相邻句能量方差 {st}\n"));
    }
    let rate_a = voice_json["speech_rate_first_half"].as_f64();
    let rate_b = voice_json["speech_rate_second_half"].as_f64();
    let vol_a = voice_json["volume_db_first_half"].as_f64();
    let vol_b = voice_json["volume_db_second_half"].as_f64();
    if rate_a.is_some() || vol_a.is_some() {
        md.push_str("- 前后半对比：");
        if let (Some(a), Some(b)) = (rate_a, rate_b) {
            md.push_str(&format!("语速 {a} → {b} 字/分钟"));
        }
        if let (Some(a), Some(b)) = (vol_a, vol_b) {
            if rate_a.is_some() {
                md.push_str("；");
            }
            md.push_str(&format!("相对音量 {a} → {b} dB"));
        }
        md.push('\n');
    }
    md.push_str("\n（声音指标为会话内相对值，用于观察起伏与稳定，不可跨设备比较。）\n\n");
    md
}

/// 本地降级报告的"对比上次"小节；无上次会话返回空串
fn local_previous_section(previous: &PreviousSession, snapshot: &SessionSnapshot) -> String {
    let date = previous.date.split(' ').next().unwrap_or(&previous.date);
    format!(
        "## 对比上次\n\n上次（{date}）：口头禅 {:.1} → 本次 {:.1} 次/分钟；语速 {:.1} → {:.1} 字/分钟；平均句长 {:.1} → {:.1} 字。\n\n",
        previous.filler_per_minute,
        snapshot.filler_per_minute,
        previous.speech_rate,
        snapshot.speech_rate,
        previous.avg_sentence_chars,
        snapshot.avg_sentence_chars,
    )
}

/// 结论句启发标记（本地降级报告的机械判断：出现即算"有结论意识"）
const CONCLUSION_MARKERS: &[&str] = &[
    "总之", "综上", "因此", "所以", "结论是", "总结一下", "一句话总结", "我的结论", "我认为",
];

pub fn has_conclusion_sentence(sentences: &[Sentence]) -> bool {
    sentences
        .iter()
        .any(|s| CONCLUSION_MARKERS.iter().any(|m| s.text.contains(m)))
}

/// 模拟面试本地降级报告的「逐题速览」：每题字数 / 口头禅 / 是否有结论句
/// 的机械统计（口头禅按规则引擎重放计数；结论句按标记词检出）
pub fn local_mock_qa_section(qa: &[QaItem]) -> String {
    let mut md = String::from("## 逐题速览\n\n");
    md.push_str("| 题 | 字数 | 口头禅 | 结论句 |\n|---|---|---|---|\n");
    for (i, item) in qa.iter().enumerate() {
        let sentences = parse_transcript_text(&item.answer);
        if sentences.is_empty() {
            md.push_str(&format!("| {}. {} | — | — | 未作答 |\n", i + 1, truncate(&item.question, 24)));
            continue;
        }
        let chars = count_chars(&sentences);
        let snap = merged_mock_snapshot(&sentences, 0);
        let fillers: u32 = snap.filler_counts.iter().map(|(_, c)| c).sum();
        let conclusion = if has_conclusion_sentence(&sentences) { "有" } else { "未检出" };
        md.push_str(&format!(
            "| {}. {} | {chars} | {fillers} 次 | {conclusion} |\n",
            i + 1,
            truncate(&item.question, 24)
        ));
    }
    md.push_str("\n（机械统计：口头禅按内置词表计数、结论句按结论标记词检出，仅供无 AI 时参考。）\n\n");
    md
}

/// 未配置 AI 后端时的本地降级报告：纯统计 + 规则事件汇总（+ 亮点金句
/// + 声音小节 + 对比上次 + 词库候选提示）。
/// candidate_words：词库自生长检测到的高频未收录词（空 = 不提示）。
/// qa：模拟面试的逐题问答（Some 时追加「逐题速览」机械统计小节）。
pub fn build_local_report(
    scenario: &str,
    topic: Option<&str>,
    sentences: &[Sentence],
    snapshot: &SessionSnapshot,
    events: &[FeedbackEvent],
    previous: Option<&PreviousSession>,
    voice: Option<&VoiceReport>,
    candidate_words: &[String],
    qa: Option<&[QaItem]>,
) -> String {
    let scenario_label = scenario_label(scenario);
    let mut md = String::new();
    md.push_str("# 表达训练 · 本地报告\n\n");
    md.push_str("> 当前未配置 AI 后端，这是基于本地统计与规则的降级报告。");
    md.push_str("在「设置」中配置 API Key 后，可生成含逐句改写与行为模式分析的完整 AI 报告。\n\n");
    md.push_str(&format!("- 场景：{scenario_label}\n"));
    if let Some(t) = topic.map(str::trim).filter(|t| !t.is_empty()) {
        md.push_str(&format!("- 主题：{t}\n"));
    }
    md.push('\n');

    if sentences.is_empty() {
        md.push_str("本次会话没有识别到句子，无法生成统计。请重新开始练习。\n");
        return md;
    }

    // 数据统计
    md.push_str("## 数据统计\n\n");
    md.push_str("| 指标 | 数值 |\n|---|---|\n");
    md.push_str(&format!("| 时长 | {} |\n", format_duration(snapshot.duration_ms)));
    md.push_str(&format!("| 句数 | {} |\n", snapshot.sentence_count));
    md.push_str(&format!("| 字数 | {} |\n", snapshot.total_chars));
    if snapshot.speech_rate > 0.0 {
        md.push_str(&format!("| 语速 | {} 字/分钟 |\n", snapshot.speech_rate));
    }
    if snapshot.avg_sentence_chars > 0.0 {
        md.push_str(&format!("| 平均句长 | {} 字 |\n", snapshot.avg_sentence_chars));
    }
    md.push('\n');

    // 模拟面试：逐题速览（每题字数 / 口头禅 / 结论句的机械统计）
    if let Some(qa) = qa {
        md.push_str(&local_mock_qa_section(qa));
    }

    // 怎么说的（声音层，会话内相对值）
    if let Some(v) = voice.and_then(|v| voice::build_voice_json(v, sentences, snapshot.duration_ms)) {
        md.push_str(&local_voice_section(&v));
    }

    // 声调（词典 + 基音轨迹的离线启发检查；无标记时省略）
    if !snapshot.tone_flags.is_empty() {
        md.push_str("## 声调\n\n");
        for f in snapshot.tone_flags.iter().take(8) {
            md.push_str(&format!(
                "- 第 {} 句「{}」应为{}声（听感偏{}）\n",
                f.sentence_id,
                f.char,
                crate::tone::tone_number_cn(f.expected_tone),
                crate::tone::shape_label_cn(f.detected_shape)
            ));
        }
        if snapshot.tone_flags.len() > 8 {
            md.push_str(&format!("\n（其余 {} 处略）\n", snapshot.tone_flags.len() - 8));
        }
        md.push_str("\n（基于基音轮廓与词典声调的离线启发判断，仅供参考；可在总结页点对应句回放对照。）\n\n");
    }

    // 口头禅
    if !snapshot.filler_counts.is_empty() {
        let minutes = snapshot.duration_ms as f64 / 60_000.0;
        let total: u32 = snapshot.filler_counts.iter().map(|(_, c)| c).sum();
        md.push_str("## 口头禅\n\n");
        md.push_str("| 词 | 次数 | 频率 |\n|---|---|---|\n");
        for (word, count) in snapshot.filler_counts.iter().take(10) {
            let freq = if minutes > 0.0 {
                format!("{} 次/分钟", round1(*count as f64 / minutes))
            } else {
                "—".into()
            };
            md.push_str(&format!("| {word} | {count} | {freq} |\n"));
        }
        let total_freq = if minutes > 0.0 {
            format!("，总频率 {} 次/分钟", snapshot.filler_per_minute)
        } else {
            String::new()
        };
        md.push_str(&format!("\n合计 {total} 次{total_freq}。\n\n"));
    }

    // 立场模糊词（hedges，无论是否触发提醒都计入统计）
    if snapshot.hedge_total > 0 {
        md.push_str("## 立场模糊词\n\n");
        md.push_str(&format!(
            "共出现 {} 次；单句堆叠 ≥2 个会实时提醒。\n\n",
            snapshot.hedge_total
        ));
        md.push_str("| 词 | 次数 |\n|---|---|\n");
        for (word, count) in snapshot.hedge_counts.iter().take(10) {
            md.push_str(&format!("| {word} | {count} |\n"));
        }
        md.push('\n');
    }

    // 亮点（金句）：有金句候选时引用展示（与规则事件表分开，正向区）
    let golden: Vec<&FeedbackEvent> = events
        .iter()
        .filter(|e| e.kind == FeedbackKind::GoldenQuote)
        .collect();
    if !golden.is_empty() {
        md.push_str("## 亮点\n\n");
        for e in golden.iter().take(3) {
            let id = e.sentence_id.map(|i| format!("#{i}")).unwrap_or_else(|| "—".into());
            md.push_str(&format!("- 「{id}」{}（原文见下方逐字稿）\n", e.message));
        }
        md.push('\n');
    }

    // 规则事件汇总（口头禅逐条出现已在上方计数，金句在"亮点"小节，均不再逐条罗列）
    let notable: Vec<&FeedbackEvent> = events
        .iter()
        .filter(|e| e.kind != FeedbackKind::FillerWord && e.kind != FeedbackKind::GoldenQuote)
        .collect();
    md.push_str("## 规则事件汇总\n\n");
    if notable.is_empty() {
        md.push_str("本次练习未触发规则提醒。\n\n");
    } else {
        md.push_str("| 类型 | 句 | 提示 |\n|---|---|---|\n");
        for e in notable.iter().take(40) {
            let id = e.sentence_id.map(|i| format!("#{i}")).unwrap_or_else(|| "—".into());
            md.push_str(&format!("| {} | {} | {} |\n", kind_label(&e.kind), id, e.message));
        }
        if notable.len() > 40 {
            md.push_str(&format!("\n（其余 {} 条略）\n", notable.len() - 40));
        }
        md.push('\n');
    }

    // 对比上次（成长档案：有历史时给一句对比）
    if let Some(prev) = previous {
        md.push_str(&local_previous_section(prev, snapshot));
    }

    // 逐字稿
    md.push_str("## 逐字稿\n\n");
    for s in sentences {
        md.push_str(&format!("{}. {}\n", s.id, s.text));
    }

    // 词库候选提示（词库自生长：高频词未收录时引导到设置补充）
    if !candidate_words.is_empty() {
        md.push_str(&format!(
            "\n> 发现高频词 {} 未收录，可到「设置 → 词库候选」补充替代词。\n",
            candidate_words.join("、")
        ));
    }
    md
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

/// 终稿逐字稿（id/text/startMs/endMs）+ 扩展统计快照
#[tauri::command]
pub fn get_transcript(state: State<AppState>) -> Value {
    let (sentences, snapshot) = state.transcript();
    json!({ "sentences": sentences, "snapshot": snapshot })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// 生成会话结束报告。
/// - 配置了 API Key（或 Ollama）：调 OpenAI 兼容 /chat/completions 流式接口，
///   逐 delta 发 `report_chunk`，结束发 `report_done`；报告末尾的 SCORE
///   评分标记被解析入库（scores）并从展示文本中剥离。
/// - 未配置 Key：返回本地降级报告 Markdown（同样发 report_chunk / report_done；
///   无 SCORE 标记，scores = None）。本地降级不受 mode 影响（本来就秒出），
///   但 mode 仍记入历史（reportMode 字段）。
/// - mode："full"（缺省，完整八节）|"quick"（快速四节 ≤400 字，quick.md +
///   max_tokens 调小）。SCORE 解析两种模式通用。
/// - scenario = mockInterview（模拟面试）：qa 必填；逐字稿由 qa 合并生成
///   （全场连续编号），统计为各题合并口径；transcript 参数在该场景下不使用。
/// 返回值为报告全文（已剥离评分标记）。
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn generate_report(
    app: AppHandle,
    state: State<'_, AppState>,
    scenario: String,
    topic: Option<String>,
    transcript: Option<String>,
    qa: Option<Vec<QaItem>>,
    mode: Option<String>,
) -> Result<String, String> {
    let report_mode = parse_report_mode(mode.as_deref()).map_err(|msg| {
        let _ = app.emit("report_error", json!({ "message": msg }));
        msg
    })?;
    let qa = qa.filter(|q| !q.is_empty());
    if scenario == "mockInterview" && qa.is_none() {
        let msg = "模拟面试报告需要逐题问答内容（qa 为空）";
        let _ = app.emit("report_error", json!({ "message": msg }));
        return Err(msg.into());
    }
    // qa 仅在 mockInterview 场景生效（其余场景忽略该参数）
    let qa = if scenario == "mockInterview" { qa.as_deref() } else { None };

    let (engine_sentences, engine_snapshot) = state.transcript();
    // 落盘所需的会话侧数据在入口一次性捕获：流式报告可持续数分钟，
    // 期间用户可能已开始下一场会话（这些字段会被 launch_session 重置），
    // 若在 await 之后再读 AppState，会把旧报告错记到新会话名下
    let persist_info = history::capture_session_persist_info(&state);
    // 逐字稿与统计：mockInterview 用 qa 合并（与逐题区间严格一致）；其余场景
    // 优先用户在总结页的修正稿
    let (sentences, snapshot) = if let Some(qa) = qa.as_deref() {
        let (s, _) = merge_qa_answers(qa);
        let duration = qa_total_duration_ms(qa);
        let snap = merged_mock_snapshot(&s, duration);
        (s, snap)
    } else {
        let sentences = match transcript.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            Some(text) => parse_transcript_text(text),
            None => engine_sentences,
        };
        (sentences, engine_snapshot)
    };
    if sentences.is_empty() {
        let msg = "没有可用的逐字稿（本此会话未识别到句子，也未提供修正稿）";
        let _ = app.emit("report_error", json!({ "message": msg }));
        return Err(msg.into());
    }

    let settings: Settings = crate::settings::load(&app);

    // 声音层终值（搭便车采集的会话内相对指标；mockInterview 时为最后一题的
    // 回答期间采集值）与"上一次会话"摘要（成长档案）
    let voice_report = state.voice.lock().unwrap().report();
    let exclude_id = persist_info.existing_id.clone();
    let previous =
        history::previous_session(&history::sessions_dir(&app), exclude_id.as_deref());

    // 本地降级
    if !settings.can_call_remote() {
        let events = collect_rule_events(&sentences);
        let candidate_words = crate::growth::top_candidate_words(&app, crate::growth::REPORT_HINT_WORDS);
        let report = build_local_report(
            &scenario,
            topic.as_deref(),
            &sentences,
            &snapshot,
            &events,
            previous.as_ref(),
            Some(&voice_report),
            &candidate_words,
            qa.as_deref(),
        );
        let _ = app.emit("report_chunk", json!({ "text": report.clone() }));
        let _ = app.emit("report_done", json!({ "text": report.clone() }));
        // 落盘成长档案（失败不影响主流程）；本地降级无评分标记 → scores = None
        history::save_session_after_report(
            &app,
            &state,
            &persist_info,
            &scenario,
            topic.as_deref(),
            &sentences,
            &snapshot,
            &voice_report.metrics,
            &report,
            "local",
            None,
            report_mode,
        );
        return Ok(report);
    }

    // 远端流式
    let result = stream_remote_report(
        &app,
        &settings,
        &scenario,
        topic.as_deref(),
        &sentences,
        &snapshot,
        &voice_report,
        previous.as_ref(),
        qa.as_deref(),
        report_mode,
    )
    .await;
    match result {
        Ok(text) => {
            // 评分入档：解析末尾 SCORE 标记；展示文本剥离标记
            let scores = parse_score_marker(&text);
            let display = strip_score_marker(&text);
            let _ = app.emit("report_done", json!({ "text": display.clone() }));
            history::save_session_after_report(
                &app,
                &state,
                &persist_info,
                &scenario,
                topic.as_deref(),
                &sentences,
                &snapshot,
                &voice_report.metrics,
                &display,
                &settings.ai_backend,
                scores.as_ref(),
                report_mode,
            );
            Ok(display)
        }
        Err(e) => {
            let _ = app.emit("report_error", json!({ "message": e }));
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_remote_report(
    app: &AppHandle,
    settings: &Settings,
    scenario: &str,
    topic: Option<&str>,
    sentences: &[Sentence],
    snapshot: &SessionSnapshot,
    voice: &VoiceReport,
    previous: Option<&PreviousSession>,
    qa: Option<&[QaItem]>,
    mode: &str,
) -> Result<String, String> {
    let system = system_prompt_for_mode(scenario, mode)?;
    // 快速模式收紧 max_tokens（四节 ≤400 字，出稿更快也更省）；
    // 完整版不设上限（行为与 0.2.2 之前一致）
    let max_tokens = if mode == "quick" { Some(QUICK_MAX_TOKENS) } else { None };
    let (base_url, model) = settings
        .resolve_endpoint()
        .ok_or_else(|| "后端地址未配置（自定义后端需填写 baseURL）".to_string())?;
    let mut stats = build_stats(
        sentences,
        snapshot.duration_ms,
        &snapshot.filler_counts,
        snapshot.golden_quote_count,
    );
    if let Some(v) = voice::build_voice_json(voice, sentences, snapshot.duration_ms) {
        stats["voice"] = v;
    }
    // 声调偏差计数（tone.rs 离线分析结果；>0 才写入）
    apply_tone_stats(&mut stats, &snapshot.tone_flags);
    // mockInterview 注入逐题 qa（含句子区间锚点）；其余场景沿用 topic/question 键
    let user_payload = if let Some(qa) = qa {
        build_mock_interview_payload(topic, qa, stats, previous)
    } else {
        build_user_payload(scenario, topic, sentences, stats, previous)
    };
    let body = build_request_body(&model, system, &user_payload.to_string(), max_tokens);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;
    let mut req = client
        .post(chat_url(&base_url))
        .header("Content-Type", "application/json")
        .body(body.to_string());
    if !settings.api_key.trim().is_empty() {
        req = req.bearer_auth(settings.api_key.trim());
    }

    let resp = req.send().await.map_err(|e| format!("请求失败：{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}：{}", truncate(text.trim(), 300)));
    }

    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::new();
    let mut full = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("流式读取中断：{e}"))?;
        for data in parser.push(&chunk) {
            if let Some(delta) = extract_delta(&data) {
                if !delta.is_empty() {
                    full.push_str(&delta);
                    let _ = app.emit("report_chunk", json!({ "text": delta }));
                }
            }
        }
    }
    if full.trim().is_empty() {
        return Err("模型返回了空报告".into());
    }
    Ok(full)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestConnectionResult {
    pub ok: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// 用当前配置发一个最小请求，返回成功/失败与延迟
#[tauri::command]
pub async fn test_connection(app: AppHandle) -> Result<TestConnectionResult, String> {
    let settings = crate::settings::load(&app);
    let fail = |msg: String| TestConnectionResult {
        ok: false,
        latency_ms: 0,
        error: Some(msg),
    };
    if !settings.can_call_remote() {
        return Ok(fail("未配置 API Key".into()));
    }
    let Some((base_url, model)) = settings.resolve_endpoint() else {
        return Ok(fail("后端地址未配置（自定义后端需填写 baseURL）".into()));
    };
    let body = json!({
        "model": model,
        "stream": false,
        "max_tokens": 1,
        "messages": [{ "role": "user", "content": "ping" }],
    });
    let started = Instant::now();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;
    let mut req = client
        .post(chat_url(&base_url))
        .header("Content-Type", "application/json")
        .body(body.to_string());
    if !settings.api_key.trim().is_empty() {
        req = req.bearer_auth(settings.api_key.trim());
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => Ok(TestConnectionResult {
            ok: true,
            latency_ms: started.elapsed().as_millis() as u64,
            error: None,
        }),
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            Ok(fail(format!("HTTP {status}：{}", truncate(text.trim(), 200))))
        }
        Err(e) => Ok(fail(format!("连接失败：{e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::engine::RuleEngine;

    fn sent(id: u64, text: &str, start_ms: u64, end_ms: u64) -> Sentence {
        Sentence { id, text: text.into(), start_ms, end_ms }
    }

    #[test]
    fn embedded_prompts_present() {
        assert!(FREE_PROMPT.contains("表达镜 SpeakMirror"));
        assert!(FREE_PROMPT.contains("## 一、总评"));
        assert!(FREE_PROMPT.contains("## 六、怎么说的"));
        assert!(FREE_PROMPT.contains("## 八、下次练习重点"));
        assert!(FREE_PROMPT.contains("previous"));
        assert!(FREE_PROMPT.contains("voice"));
        assert!(INTERVIEW_PROMPT.contains("STAR 完整性"));
        assert!(INTERVIEW_PROMPT.contains("## 六、怎么说的"));
        assert!(INTERVIEW_PROMPT.contains("## 八、下次练习重点"));
        assert!(INTERVIEW_PROMPT.contains("previous"));
        // 口播视频：钩子 / CTA / 信息密度 维度齐全，红线（不编造数据）在位
        assert!(VLOG_PROMPT.contains("开场钩子"));
        assert!(VLOG_PROMPT.contains("CTA"));
        assert!(VLOG_PROMPT.contains("信息密度"));
        assert!(VLOG_PROMPT.contains("节奏感"));
        assert!(VLOG_PROMPT.contains("禁止替用户编造数据"));
        assert!(VLOG_PROMPT.contains("## 一、总评"));
        assert!(VLOG_PROMPT.contains("## 六、怎么说的"));
        assert!(VLOG_PROMPT.contains("## 八、下次练习重点"));
        assert!(VLOG_PROMPT.contains("previous"));
        assert!(VLOG_PROMPT.contains("voice"));
        // 工作汇报：金字塔结论先行 / 数据支撑 / 四要素结构 / 听众视角
        assert!(WORKREPORT_PROMPT.contains("结论先行"));
        assert!(WORKREPORT_PROMPT.contains("数据支撑"));
        assert!(WORKREPORT_PROMPT.contains("背景 / 进展 / 风险 / 下一步"));
        assert!(WORKREPORT_PROMPT.contains("听众视角"));
        assert!(WORKREPORT_PROMPT.contains("禁止替用户编造数据"));
        assert!(WORKREPORT_PROMPT.contains("## 一、总评"));
        assert!(WORKREPORT_PROMPT.contains("## 六、怎么说的"));
        assert!(WORKREPORT_PROMPT.contains("## 八、下次练习重点"));
        assert!(WORKREPORT_PROMPT.contains("previous"));
        assert!(WORKREPORT_PROMPT.contains("voice"));
        // 模拟面试：逐题 STAR / 句子区间锚点 / 未作答处理
        assert!(MOCK_INTERVIEW_PROMPT.contains("表达镜 SpeakMirror"));
        assert!(MOCK_INTERVIEW_PROMPT.contains("逐题点评"));
        assert!(MOCK_INTERVIEW_PROMPT.contains("sentence_ids"));
        assert!(MOCK_INTERVIEW_PROMPT.contains("STAR"));
        assert!(MOCK_INTERVIEW_PROMPT.contains("未作答"));
        assert!(MOCK_INTERVIEW_PROMPT.contains("q1_star"));
        assert!(MOCK_INTERVIEW_PROMPT.contains("previous"));
        assert!(MOCK_INTERVIEW_PROMPT.contains("voice"));
        // 四份场景 prompt 的八节标题一字不差、顺序一致
        for prompt in [FREE_PROMPT, INTERVIEW_PROMPT, VLOG_PROMPT, WORKREPORT_PROMPT] {
            for section in [
                "## 一、总评",
                "## 二、亮点",
                "## 三、逐句改写",
                "## 四、可替换词汇表",
                "## 五、行为模式分析",
                "## 六、怎么说的",
                "## 七、数据区",
                "## 八、下次练习重点",
            ] {
                assert!(prompt.contains(section), "缺小节 {section}");
            }
        }
        // 模拟面试：八节结构沿用，仅第三节改为逐题点评
        for section in [
            "## 一、总评",
            "## 二、亮点",
            "## 三、逐题点评",
            "## 四、可替换词汇表",
            "## 五、行为模式分析",
            "## 六、怎么说的",
            "## 七、数据区",
            "## 八、下次练习重点",
        ] {
            assert!(MOCK_INTERVIEW_PROMPT.contains(section), "模拟面试缺小节 {section}");
        }
        // 五份报告 prompt 统一要求末尾 SCORE 评分标记，且声明其后不得再有内容
        for prompt in [
            FREE_PROMPT,
            INTERVIEW_PROMPT,
            VLOG_PROMPT,
            WORKREPORT_PROMPT,
            MOCK_INTERVIEW_PROMPT,
        ] {
            assert!(prompt.contains("<!--SCORE:"), "缺 SCORE 标记约定");
            assert!(prompt.contains("其后不得再有任何内容"), "缺「标记后无内容」约定");
        }
    }

    #[test]
    fn system_prompt_for_maps_scenarios() {
        assert_eq!(system_prompt_for("free").unwrap(), FREE_PROMPT);
        assert_eq!(system_prompt_for("interview").unwrap(), INTERVIEW_PROMPT);
        assert_eq!(system_prompt_for("vlog").unwrap(), VLOG_PROMPT);
        assert_eq!(system_prompt_for("workreport").unwrap(), WORKREPORT_PROMPT);
        assert_eq!(system_prompt_for("mockInterview").unwrap(), MOCK_INTERVIEW_PROMPT);
        assert!(system_prompt_for("other").is_err());
    }

    #[test]
    fn quick_prompt_embedded_with_full_version_contract() {
        // 身份与角色
        assert!(QUICK_PROMPT.contains("表达镜 SpeakMirror"));
        assert!(QUICK_PROMPT.contains("快速报告"));
        // 输入结构与完整版一致：topic/question、previous、transcript、stats、qa、voice 全在
        assert!(QUICK_PROMPT.contains("结构与完整版报告完全一致"));
        assert!(QUICK_PROMPT.contains("previous"));
        assert!(QUICK_PROMPT.contains("start_ms"));
        assert!(QUICK_PROMPT.contains("sentence_ids"));
        assert!(QUICK_PROMPT.contains("volume_dynamic_range_db"));
        // ASR 错字纪律保留
        assert!(QUICK_PROMPT.contains("同音/近音错字"));
        assert!(QUICK_PROMPT.contains("禁止把 ASR 错字当作用户的表达问题来批评"));
        // 长度纪律改为 400 字
        assert!(QUICK_PROMPT.contains("不超过 400 字"));
        // 四节结构（骨架渲染按 ## 标题分节归位，标题一字不差）
        for section in [
            "## 一、总评",
            "## 二、亮点",
            "## 三、最需要改进的一个问题",
            "## 四、下次重点",
        ] {
            assert!(QUICK_PROMPT.contains(section), "快速报告缺小节 {section}");
        }
        // SCORE 标记照旧 + 评分维度沿用完整版（按场景）
        assert!(QUICK_PROMPT.contains("<!--SCORE:"));
        assert!(QUICK_PROMPT.contains("其后不得再有任何内容"));
        assert!(QUICK_PROMPT.contains("评分维度同完整版"));
        assert!(QUICK_PROMPT.contains("STAR完整性"));
        assert!(QUICK_PROMPT.contains("CTA行动号召"));
        // 证据引用铁律保留
        assert!(QUICK_PROMPT.contains("「#句id」"));
    }

    #[test]
    fn report_mode_whitelist_and_defaults() {
        // 缺省 / 空白 → full（旧前端兼容）
        assert_eq!(parse_report_mode(None).unwrap(), "full");
        assert_eq!(parse_report_mode(Some("")).unwrap(), "full");
        assert_eq!(parse_report_mode(Some("   ")).unwrap(), "full");
        // 白名单两值；空白被裁剪
        assert_eq!(parse_report_mode(Some("full")).unwrap(), "full");
        assert_eq!(parse_report_mode(Some("quick")).unwrap(), "quick");
        assert_eq!(parse_report_mode(Some(" quick ")).unwrap(), "quick");
        // 白名单外报错（不静默降级）
        assert!(parse_report_mode(Some("fast")).is_err());
        assert!(parse_report_mode(Some("FULL")).is_err());
        let err = parse_report_mode(Some("fast")).unwrap_err();
        assert!(err.contains("full / quick"), "错误信息应列出支持的模式：{err}");
    }

    #[test]
    fn system_prompt_for_mode_selects_quick_or_scenario() {
        // quick：任何合法场景都用 quick.md（场景仍需校验）
        for scenario in SCENARIOS {
            assert_eq!(system_prompt_for_mode(scenario, "quick").unwrap(), QUICK_PROMPT);
        }
        // quick 模式下非法场景同样报错（历史落盘口径一致）
        assert!(system_prompt_for_mode("other", "quick").is_err());
        // full：按场景选
        assert_eq!(system_prompt_for_mode("free", "full").unwrap(), FREE_PROMPT);
        assert_eq!(system_prompt_for_mode("vlog", "full").unwrap(), VLOG_PROMPT);
        // 未知模式报错
        assert!(system_prompt_for_mode("free", "turbo").is_err());
    }

    #[test]
    fn scenario_labels_cover_all_scenarios() {
        assert_eq!(scenario_label("free"), "自由练习");
        assert_eq!(scenario_label("interview"), "面试回答");
        assert_eq!(scenario_label("vlog"), "口播视频");
        assert_eq!(scenario_label("workreport"), "工作汇报");
        assert_eq!(scenario_label("mockInterview"), "模拟面试");
        assert_eq!(scenario_label("unknown"), "自由练习");
    }

    #[test]
    fn parse_transcript_text_assigns_line_ids_and_skips_blanks() {
        let s = parse_transcript_text("第一句\r\n\r\n  \n第二句\n");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].id, 1);
        assert_eq!(s[0].text, "第一句");
        assert_eq!(s[1].id, 2);
        assert!(s.iter().all(|x| x.start_ms == 0 && x.end_ms == 0));
    }

    #[test]
    fn transcript_text_roundtrips() {
        let text = "第一句\n第二句";
        assert_eq!(transcript_text(&parse_transcript_text(text)), text);
    }

    #[test]
    fn count_chars_ignores_whitespace() {
        let s = vec![sent(1, "你好 世界", 0, 1000)];
        assert_eq!(count_chars(&s), 4);
    }

    #[test]
    fn build_stats_computes_rates_when_duration_known() {
        let s = vec![
            sent(1, "五个字啊哈", 0, 30_000),
            sent(2, "再来五个字", 30_000, 60_000),
        ];
        let fillers = vec![("就是".to_string(), 3u32), ("然后".to_string(), 1u32)];
        let stats = build_stats(&s, 60_000, &fillers, 0);
        assert_eq!(stats["duration_sec"], 60);
        assert_eq!(stats["total_chars"], 10);
        assert_eq!(stats["sentence_count"], 2);
        assert_eq!(stats["speech_rate"], 10.0);
        assert_eq!(stats["fillers"][0]["per_minute"], 3.0);
        assert_eq!(stats["fillers"][1]["per_minute"], 1.0);
        // 无金句：字段省略（prompt 按"缺失=未知"处理）
        assert!(stats.get("goldenQuoteCount").is_none());
    }

    #[test]
    fn build_stats_includes_golden_quote_count_when_present() {
        let s = vec![sent(1, "这就像把 3 个月的工作压缩到 3 周", 0, 60_000)];
        let stats = build_stats(&s, 60_000, &[], 2);
        assert_eq!(stats["goldenQuoteCount"], 2);
    }

    #[test]
    fn apply_tone_stats_writes_count_only_when_present() {
        // 无标记：字段缺失（= 未分析/无发现，prompt 允许）
        let mut stats = json!({ "total_chars": 10 });
        apply_tone_stats(&mut stats, &[]);
        assert!(stats.get("toneFlagCount").is_none());
        // 有标记：写入计数
        let flags = vec![
            crate::tone::ToneFlag {
                sentence_id: 1,
                char_index: 0,
                char: "妈".into(),
                expected_tone: 1,
                detected_shape: 4,
            },
            crate::tone::ToneFlag {
                sentence_id: 2,
                char_index: 1,
                char: "骂".into(),
                expected_tone: 4,
                detected_shape: 2,
            },
        ];
        apply_tone_stats(&mut stats, &flags);
        assert_eq!(stats["toneFlagCount"], 2);
        assert_eq!(stats["total_chars"], 10, "已有字段不受影响");
    }

    #[test]
    fn build_stats_omits_rates_when_duration_unknown() {
        let s = vec![sent(1, "修正稿没有时间戳", 0, 0)];
        let stats = build_stats(&s, 0, &[], 0);
        assert!(stats.get("speech_rate").is_none());
        assert!(stats.get("fillers").is_none());
        assert_eq!(stats["total_chars"], 8);
    }

    #[test]
    fn user_payload_free_uses_topic_and_snake_case() {
        let s = vec![sent(1, "内容", 120, 3400)];
        let payload = build_user_payload("free", Some("自律"), &s, json!({}), None);
        assert_eq!(payload["topic"], "自律");
        assert!(payload.get("question").is_none());
        assert!(payload.get("previous").is_none());
        assert_eq!(payload["transcript"][0]["start_ms"], 120);
        assert_eq!(payload["transcript"][0]["end_ms"], 3400);
    }

    #[test]
    fn user_payload_interview_uses_question_and_skips_empty_topic() {
        let payload = build_user_payload("interview", Some("  "), &[], json!({}), None);
        assert!(payload.get("question").is_none());
        assert!(payload.get("topic").is_none());
        let payload = build_user_payload("interview", Some("讲一次推动协作的经历"), &[], json!({}), None);
        assert_eq!(payload["question"], "讲一次推动协作的经历");
    }

    #[test]
    fn user_payload_includes_previous_when_present() {
        let prev = PreviousSession {
            date: "2026-08-29 10:00:00".into(),
            filler_per_minute: 4.2,
            speech_rate: 230.0,
            avg_sentence_chars: 18.0,
        };
        let payload = build_user_payload("free", None, &[], json!({}), Some(&prev));
        assert_eq!(payload["previous"]["date"], "2026-08-29 10:00:00");
        assert_eq!(payload["previous"]["filler_per_minute"], 4.2);
        assert_eq!(payload["previous"]["speech_rate"], 230.0);
        assert_eq!(payload["previous"]["avg_sentence_chars"], 18.0);
    }

    #[test]
    fn request_body_is_streaming_with_json_string_user_content() {
        let body = build_request_body("deepseek-chat", "SYS", r#"{ "topic": "x" }"#, None);
        assert_eq!(body["model"], "deepseek-chat");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        let user = body["messages"][1]["content"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(user).unwrap();
        assert_eq!(parsed["topic"], "x");
        // 完整版不设 max_tokens（字段省略，行为与 0.2.2 之前一致）
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn request_body_quick_mode_sets_smaller_max_tokens() {
        let body = build_request_body("deepseek-chat", "SYS", "{}", Some(QUICK_MAX_TOKENS));
        assert_eq!(body["max_tokens"], QUICK_MAX_TOKENS);
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn chat_url_joins_without_double_slash() {
        assert_eq!(chat_url("https://api.x.com/v1/"), "https://api.x.com/v1/chat/completions");
    }

    #[test]
    fn sse_parser_handles_multi_chunk_and_utf8_split() {
        let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n\
                   data: {\"choices\":[{\"delta\":{\"content\":\"世界\"}}]}\n\n\
                   data: [DONE]\n\n";
        // 按一个字节一个字节喂，模拟最恶劣的切割
        let mut p = SseParser::new();
        let mut datas = Vec::new();
        for b in raw.as_bytes() {
            datas.extend(p.push(&[*b]));
        }
        assert_eq!(datas.len(), 2);
        assert!(datas[0].contains("你好"));
        assert!(datas[1].contains("世界"));
        assert!(!datas.iter().any(|d| d.contains("DONE")));
    }

    #[test]
    fn sse_parser_handles_crlf_and_ignores_comment_lines() {
        let raw = ": keep-alive\r\ndata: {\"x\":1}\r\n\r\ndata: [DONE]\r\n";
        let mut p = SseParser::new();
        let datas = p.push(raw.as_bytes());
        assert_eq!(datas, vec![r#"{"x":1}"#.to_string()]);
    }

    #[test]
    fn extract_delta_reads_content() {
        let data = r#"{"choices":[{"delta":{"content":"评分"}}]}"#;
        assert_eq!(extract_delta(data).as_deref(), Some("评分"));
        // role-only 首 chunk、无 delta、非 JSON
        assert_eq!(extract_delta(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#), None);
        assert_eq!(extract_delta(r#"{"choices":[{"finish_reason":"stop"}]}"#), None);
        assert_eq!(extract_delta("not json"), None);
    }

    #[test]
    fn collect_rule_events_replays_deterministically() {
        let sentences = vec![
            sent(1, "这个系统可以实时分析你的表达问题", 0, 1000),
            sent(2, "这个系统可以实时分析你的表达问题啊", 1000, 2000),
            sent(3, "然后我想说很多", 2000, 3000),
        ];
        let events = collect_rule_events(&sentences);
        assert!(events.iter().any(|e| e.kind == FeedbackKind::Repetition));
        assert!(events.iter().any(|e| e.kind == FeedbackKind::FillerWord));
        assert!(events.iter().any(|e| e.kind == FeedbackKind::WordPrecision));
    }

    #[test]
    fn local_report_renders_stats_fillers_and_events() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "然后我想要很多东西", 0, 30_000));
        engine.ingest(sent(2, "然后这个系统可以实时分析你的表达问题啊", 30_000, 60_000));
        engine.ingest(sent(3, "这个系统可以实时分析你的表达问题呢", 60_000, 90_000));
        let snapshot = engine.snapshot();
        let sentences = vec![
            sent(1, "然后我想要很多东西", 0, 30_000),
            sent(2, "然后这个系统可以实时分析你的表达问题啊", 30_000, 60_000),
            sent(3, "这个系统可以实时分析你的表达问题呢", 60_000, 90_000),
        ];
        let events = collect_rule_events(&sentences);
        let md = build_local_report(
            "free",
            Some("自律"),
            &sentences,
            &snapshot,
            &events,
            None,
            None,
            &[],
            None,
        );

        assert!(md.contains("# 表达训练 · 本地报告"));
        assert!(md.contains("- 场景：自由练习"));
        assert!(md.contains("- 主题：自律"));
        assert!(md.contains("## 数据统计"));
        assert!(md.contains("## 口头禅"));
        assert!(md.contains("| 然后 | 2 |"));
        assert!(md.contains("## 规则事件汇总"));
        assert!(md.contains("重复"));
        assert!(md.contains("词汇精确度"));
        assert!(md.contains("## 逐字稿"));
        assert!(md.contains("1. 然后我想要很多东西"));
        // 口头禅逐条出现不进事件表
        assert!(!md.contains("| 口头禅 |"));
        // 无声音数据、无历史、无金句、无候选：四小节均省略
        assert!(!md.contains("怎么说的"));
        assert!(!md.contains("对比上次"));
        assert!(!md.contains("## 亮点"));
        assert!(!md.contains("词库候选"));
    }

    #[test]
    fn local_report_handles_empty_session() {
        let engine = RuleEngine::new();
        let md = build_local_report("interview", None, &[], &engine.snapshot(), &[], None, None, &[], None);
        assert!(md.contains("- 场景：面试回答"));
        assert!(md.contains("没有识别到句子"));
        assert!(!md.contains("## 数据统计"));
    }

    #[test]
    fn local_report_labels_new_scenarios() {
        let engine = RuleEngine::new();
        let md = build_local_report("vlog", None, &[], &engine.snapshot(), &[], None, None, &[], None);
        assert!(md.contains("- 场景：口播视频"));
        let md = build_local_report("workreport", Some("Q3 进展"), &[], &engine.snapshot(), &[], None, None, &[], None);
        assert!(md.contains("- 场景：工作汇报"));
        assert!(md.contains("- 主题：Q3 进展"));
    }

    #[test]
    fn local_report_lists_tone_flags_when_present() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "妈妈骂马", 0, 30_000));
        let sentences = vec![sent(1, "妈妈骂马", 0, 30_000)];
        let mut snapshot = engine.snapshot();
        snapshot.tone_flags = vec![
            crate::tone::ToneFlag {
                sentence_id: 1,
                char_index: 2,
                char: "骂".into(),
                expected_tone: 4,
                detected_shape: 2,
            },
            crate::tone::ToneFlag {
                sentence_id: 1,
                char_index: 3,
                char: "马".into(),
                expected_tone: 3,
                detected_shape: 1,
            },
        ];
        let md = build_local_report("free", None, &sentences, &snapshot, &[], None, None, &[], None);
        assert!(md.contains("## 声调"));
        assert!(md.contains("第 1 句「骂」应为四声（听感偏升调）"));
        assert!(md.contains("第 1 句「马」应为三声（听感偏高平）"));
        // 无标记的普通报告不含声调小节
        let plain =
            build_local_report("free", None, &sentences, &engine.snapshot(), &[], None, None, &[], None);
        assert!(!plain.contains("## 声调"));
    }

    #[test]
    fn local_report_renders_golden_quotes_and_candidate_hint() {
        let sentences = vec![
            sent(1, "这就像把 3 个月的工作压缩到 3 周", 0, 60_000),
            sent(2, "写周报就像照镜子，一目了然、毫不留情", 60_000, 120_000),
            sent(3, "普通的一句过渡", 120_000, 150_000),
        ];
        let events = collect_rule_events(&sentences);
        assert!(events.iter().any(|e| e.kind == FeedbackKind::GoldenQuote));
        let engine = RuleEngine::new();
        let md = build_local_report(
            "free",
            None,
            &sentences,
            &engine.snapshot(),
            &events,
            None,
            None,
            &["颗粒度".to_string(), "抓手".to_string()],
            None,
        );
        // 亮点小节引用金句（句 id 锚定），且不重复进规则事件表
        assert!(md.contains("## 亮点"));
        assert!(md.contains("「#1」"));
        assert!(md.contains("金句信号"));
        assert!(!md.contains("| 金句 |"));
        // 结尾候选提示一行
        assert!(md.contains("发现高频词 颗粒度、抓手 未收录"));
        assert!(md.contains("设置 → 词库候选"));
        // 打印样例供人工核对（--nocapture 时可见）
        println!("\n===== 含亮点/候选提示的本地报告样例 =====\n{md}\n==========================");
    }

    #[test]
    fn local_report_renders_voice_and_previous_sections() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "然后我想聊聊知识管理", 0, 120_000));
        let mut snapshot = engine.snapshot();
        snapshot.speech_rate = 200.0;
        snapshot.filler_per_minute = 3.0;
        snapshot.avg_sentence_chars = 16.0;
        let sentences = vec![sent(1, "然后我想聊聊知识管理", 0, 120_000)];
        let events = collect_rule_events(&sentences);

        // 声音数据：两句（0 dB / −6 dB）+ 一次 2.1 秒失控停顿
        let mut voice = crate::voice::VoiceAnalyzer::new(16_000);
        voice.push_chunk(&vec![0.5; 48_000], true);
        voice.close_sentence();
        voice.push_chunk(&vec![0.25; 16_000], true);
        voice.close_sentence();
        voice.push_chunk(&vec![0.0; 33_600], false);
        let voice_report = voice.report();

        let prev = PreviousSession {
            date: "2026-08-29 10:00:00".into(),
            filler_per_minute: 4.2,
            speech_rate: 230.0,
            avg_sentence_chars: 18.0,
        };
        let md = build_local_report(
            "free",
            None,
            &sentences,
            &snapshot,
            &events,
            Some(&prev),
            Some(&voice_report),
            &[],
            None,
        );
        assert!(md.contains("## 怎么说的（声音）"));
        assert!(md.contains("失控停顿：1 次"));
        assert!(md.contains("音量动态范围"));
        assert!(md.contains("会话内相对值"));
        assert!(md.contains("## 对比上次"));
        assert!(md.contains("上次（2026-08-29）"));
        assert!(md.contains("口头禅 4.2 → 本次 3.0 次/分钟"));
        assert!(md.contains("语速 230.0 → 200.0 字/分钟"));
        assert!(md.contains("平均句长 18.0 → 16.0 字"));
    }

    #[test]
    fn format_duration_variants() {
        assert_eq!(format_duration(0), "0 秒");
        assert_eq!(format_duration(59_000), "59 秒");
        assert_eq!(format_duration(187_400), "3 分 07 秒");
    }

    /// 验收路径：会话结束（get_transcript 数据源）→ 用户修正错字 → 本地降级报告
    #[test]
    fn degraded_flow_from_engine_through_correction_to_local_report() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "然后我想聊聊只是管理这个话题", 0, 62_100));
        engine.ingest(sent(2, "就是很多人觉得只是管理就是记笔记", 62_100, 124_800));

        // get_transcript 的数据源
        let sentences = engine.sentences().to_vec();
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.sentence_count, 2);
        assert!(snapshot.total_chars > 0);
        assert!(snapshot.speech_rate > 0.0);

        // 用户在总结页把 ASR 错字"只是"修正为"知识"
        let corrected_text = transcript_text(&sentences).replace("只是", "知识");
        let corrected = parse_transcript_text(&corrected_text);
        assert!(corrected.iter().all(|s| s.text.contains("知识")));

        // 未配置 Key 的降级路径
        let events = collect_rule_events(&corrected);
        let md = build_local_report(
            "free",
            Some("知识管理"),
            &corrected,
            &snapshot,
            &events,
            None,
            None,
            &[],
            None,
        );
        assert!(md.contains("知识管理"));
        assert!(md.contains("## 数据统计"));
        assert!(md.contains("## 口头禅"));
        assert!(md.contains("## 逐字稿"));
        // 修正稿进入逐字稿与规则重放
        assert!(md.contains("知识管理这个话题"));
        // 打印样例供人工核对（--nocapture 时可见）
        println!("\n===== 本地降级报告样例 =====\n{md}\n==========================");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate("短", 5), "短");
    }

    // --- 模拟面试：qa 注入 user JSON ----------------------------------------

    fn qa_item(question: &str, intent: &str, answer: &str, duration_ms: u64) -> QaItem {
        QaItem {
            question: question.into(),
            intent: intent.into(),
            answer: answer.into(),
            duration_ms: Some(duration_ms),
        }
    }

    #[test]
    fn merge_qa_answers_renumbers_globally_and_counts_lines() {
        let qa = vec![
            qa_item("第一题？", "开场", "第一题答句甲\n第一题答句乙", 60_000),
            qa_item("第二题？", "深挖", "  \n第二题答句丙", 0),
            qa_item("第三题？", "跳过", "", 0),
        ];
        let (sentences, counts) = merge_qa_answers(&qa);
        assert_eq!(counts, vec![2, 1, 0]);
        // 全场连续编号 1..=3（parse_transcript_text 每题从 1 起，这里重排）
        assert_eq!(sentences.iter().map(|s| s.id).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(sentences[2].text, "第二题答句丙");
    }

    #[test]
    fn mock_interview_payload_injects_qa_with_ranges_and_durations() {
        let qa = vec![
            qa_item("第一题？", "自我介绍", "第一题答句甲\n第一题答句乙", 95_000),
            qa_item("第二题？", "项目深挖", "", 30_000), // 未作答：区间/时长均不注入
            qa_item("第三题？", "压力题", "第三题答句丙", 40_000),
        ];
        let stats = json!({ "total_chars": 30 });
        let prev = PreviousSession {
            date: "2026-08-29 10:00:00".into(),
            filler_per_minute: 4.2,
            speech_rate: 230.0,
            avg_sentence_chars: 18.0,
        };
        let payload = build_mock_interview_payload(Some("后端开发"), &qa, stats, Some(&prev));

        assert_eq!(payload["role"], "后端开发");
        assert_eq!(payload["qa"].as_array().unwrap().len(), 3);
        // 第 1 题：句子区间 [1,2]，时长 95 秒
        assert_eq!(payload["qa"][0]["index"], 1);
        assert_eq!(payload["qa"][0]["question"], "第一题？");
        assert_eq!(payload["qa"][0]["intent"], "自我介绍");
        assert_eq!(payload["qa"][0]["sentence_ids"], json!([1, 2]));
        assert_eq!(payload["qa"][0]["duration_sec"], 95);
        assert_eq!(payload["qa"][0]["skipped"], false);
        // 第 2 题：未作答 → 无区间、skipped=true、无时长
        assert_eq!(payload["qa"][1]["skipped"], true);
        assert!(payload["qa"][1].get("sentence_ids").is_none());
        assert!(payload["qa"][1].get("duration_sec").is_none());
        // 第 3 题：区间 [3,3]
        assert_eq!(payload["qa"][2]["sentence_ids"], json!([3, 3]));
        // 合并 transcript 连续编号，previous 透传
        assert_eq!(payload["transcript"].as_array().unwrap().len(), 3);
        assert_eq!(payload["transcript"][2]["id"], 3);
        assert_eq!(payload["previous"]["filler_per_minute"], 4.2);
        // role 缺省时字段省略
        let bare = build_mock_interview_payload(None, &qa, json!({}), None);
        assert!(bare.get("role").is_none());
        assert!(bare.get("previous").is_none());
    }

    #[test]
    fn qa_total_duration_sums_known_durations() {
        let qa = vec![
            qa_item("a？", "x", "答", 60_000),
            QaItem { question: "b？".into(), intent: "y".into(), answer: String::new(), duration_ms: None },
            qa_item("c？", "z", "答", 30_000),
        ];
        assert_eq!(qa_total_duration_ms(&qa), 90_000);
    }

    #[test]
    fn merged_mock_snapshot_recomputes_rates_from_total_duration() {
        let sentences = vec![
            sent(1, "然后我想聊聊这个项目", 0, 0),
            sent(2, "然后这个项目救了我们团队", 0, 0),
        ];
        let snap = merged_mock_snapshot(&sentences, 60_000);
        assert_eq!(snap.sentence_count, 2);
        assert_eq!(snap.duration_ms, 60_000);
        assert!(snap.filler_counts.iter().any(|(w, _)| w == "然后"));
        // 时长未知（0）：派生速率保持 0（未知口径，prompt 按"缺失=未知"处理）
        let unknown = merged_mock_snapshot(&sentences, 0);
        assert_eq!(unknown.speech_rate, 0.0);
        assert_eq!(unknown.filler_per_minute, 0.0);
        // 已知时长：语速 = 总字数 / 分钟（10 字 + 12 字 = 22）
        assert_eq!(snap.speech_rate, round1(22.0));
    }

    // --- 评分标记（SCORE）---------------------------------------------------

    #[test]
    fn parse_score_marker_valid_cases() {
        let text = "## 八、下次练习重点\n1. 先给结论。\n\n<!--SCORE:{\"overall\":78,\"表达效率\":80}-->";
        let v = parse_score_marker(text).unwrap();
        assert_eq!(v["overall"], 78);
        assert_eq!(v["表达效率"], 80);
        // 标记前允许空白 / 内部允许空格
        let spaced = "正文\n<!--SCORE: {\"overall\": 60} -->尾";
        assert_eq!(parse_score_marker(spaced).unwrap()["overall"], 60);
        // 多个标记取第一个
        let twice = "<!--SCORE:{\"overall\":1}-->\n<!--SCORE:{\"overall\":2}-->";
        assert_eq!(parse_score_marker(twice).unwrap()["overall"], 1);
    }

    #[test]
    fn parse_score_marker_tolerates_missing_and_malformed() {
        assert!(parse_score_marker("没有任何标记的报告").is_none());
        assert!(parse_score_marker("").is_none());
        // 畸形 JSON / 非 object / 未闭合 → None 不报错
        assert!(parse_score_marker("<!--SCORE:{overall:78}-->").is_none());
        assert!(parse_score_marker("<!--SCORE:[1,2]-->").is_none());
        assert!(parse_score_marker("<!--SCORE:78-->").is_none());
        assert!(parse_score_marker("<!--SCORE:{\"overall\":78-->").is_none());
        assert!(parse_score_marker("<!--SCORE:{\"overall\":78}").is_none());
    }

    #[test]
    fn strip_score_marker_removes_marker_and_keeps_body() {
        let with = "## 八、下次练习重点\n1. 先给结论。\n\n<!--SCORE:{\"overall\":78}-->";
        let stripped = strip_score_marker(with);
        assert_eq!(stripped, "## 八、下次练习重点\n1. 先给结论。\n");
        assert!(!stripped.contains("SCORE"));
        // 无标记原样返回；未闭合前缀（流式中途）裁掉尾部
        let plain = "普通报告\n内容";
        assert_eq!(strip_score_marker(plain), plain);
        assert_eq!(strip_score_marker("正文\n<!--SCORE:{\"ov"), "正文");
        // 标记后有违规多余内容：标记移除、多余内容保留（不吞正文）
        assert_eq!(strip_score_marker("a\n<!--SCORE:{}-->b"), "a\nb\n");
    }

    // --- 模拟面试本地降级 ---------------------------------------------------

    #[test]
    fn local_mock_qa_section_renders_mechanical_stats() {
        let qa = vec![
            qa_item("请自我介绍？", "开场", "我做了三年后端。\n所以我的优势是稳。", 60_000),
            qa_item("讲个失败经历？", "复盘", "", 0), // 未作答
        ];
        let md = local_mock_qa_section(&qa);
        assert!(md.contains("## 逐题速览"));
        assert!(md.contains("| 1. 请自我介绍？ |"));
        assert!(md.contains("| 有 |")); // 第二句含"所以" → 结论句检出
        assert!(md.contains("| 2. 讲个失败经历？ | — | — | 未作答 |"));
        assert!(md.contains("机械统计"));
    }

    #[test]
    fn local_report_with_qa_renders_mock_section_and_scenario_label() {
        let qa = vec![qa_item("为什么离职？", "动机", "我想找更大的舞台。", 45_000)];
        let (sentences, counts) = merge_qa_answers(&qa);
        assert_eq!(counts, vec![1]);
        let snap = merged_mock_snapshot(&sentences, 45_000);
        let events = collect_rule_events(&sentences);
        let md = build_local_report(
            "mockInterview",
            Some("后端开发"),
            &sentences,
            &snap,
            &events,
            None,
            None,
            &[],
            Some(&qa),
        );
        assert!(md.contains("- 场景：模拟面试"));
        assert!(md.contains("- 主题：后端开发"));
        assert!(md.contains("## 逐题速览"));
        assert!(md.contains("为什么离职"));
        // 无 qa 的普通场景不含逐题速览
        let plain = build_local_report(
            "free",
            None,
            &sentences,
            &snap,
            &events,
            None,
            None,
            &[],
            None,
        );
        assert!(!plain.contains("逐题速览"));
    }
}
