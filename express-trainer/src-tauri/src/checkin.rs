//! AI 周期快评（M2 · 产品方案 §5.3，默认关闭）。
//!
//! 前端在会话进行中每 N 秒调一次 `check_in` 命令：取最近 3–5 句 + 更早 1–3 句，
//! 连同主题锚词重合度、进度 hints 发给配置的 LLM（非流式，max_tokens 120），
//! 按行协议解析回复：`OK` 静默；`TOPIC/CONTRA/WRAP 一句话` 转成 aiCheckin
//! 反馈事件插入右栏（受 90 秒同类冷却约束）。

use crate::rules::{FeedbackEvent, FeedbackKind, Sentence};
use crate::settings::Settings;
use crate::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

/// docs/prompts/realtime-checkin.md 的 system prompt 原样内嵌（docs 只读）
pub const CHECKIN_SYSTEM_PROMPT: &str = include_str!("../prompts/checkin.md");

/// 同类快评事件的冷却秒数（规则 UX 纪律：宁可漏报，不可刷屏）
pub const CHECKIN_COOLDOWN_SECS: u64 = 90;

/// 连续失败多少次后自动停用本次会话的快评（请求失败 / 响应解析失败都算）
pub const CHECKIN_MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// 发给 LLM 的最近句数（上限）与更早句拼接长度（上限）
pub const RECENT_MAX_SENTENCES: usize = 5;
pub const RECENT_BEFORE_MAX_SENTENCES: usize = 3;

/// 单条快评消息的显示截断上限（prompt 约定每行 ≤40 字，这里留安全余量）
pub const MESSAGE_MAX_CHARS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckinIssueKind {
    /// 偏题
    Topic,
    /// 前后矛盾
    Contra,
    /// 该收结论
    Wrap,
}

impl CheckinIssueKind {
    pub fn key(self) -> &'static str {
        match self {
            CheckinIssueKind::Topic => "topic",
            CheckinIssueKind::Contra => "contra",
            CheckinIssueKind::Wrap => "wrap",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinIssue {
    pub kind: CheckinIssueKind,
    pub message: String,
}

/// 按字符数截断（UTF-8 安全），超出加省略号
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// 解析 LLM 行协议回复（纯函数）：
/// - 整体 trim 后等于 `OK`（或全为空白/无有效行）→ 本周期无事
/// - 逐行匹配 `TOPIC ` / `CONTRA ` / `WRAP `（前缀后必须跟非空内容）
/// - 无法识别的行直接忽略（模型偶发抖动不致命）；同一前缀只取第一次出现
/// - 输出顺序固定为 TOPIC、CONTRA、WRAP
pub fn parse_checkin_reply(reply: &str) -> Vec<CheckinIssue> {
    if reply.trim() == "OK" {
        return Vec::new();
    }
    let mut topic: Option<String> = None;
    let mut contra: Option<String> = None;
    let mut wrap: Option<String> = None;
    for line in reply.lines() {
        let line = line.trim();
        if line.is_empty() || line == "OK" {
            continue;
        }
        let take = |slot: &mut Option<String>, msg: &str| {
            if slot.is_none() {
                let msg = msg.trim();
                if !msg.is_empty() {
                    *slot = Some(truncate_chars(msg, MESSAGE_MAX_CHARS));
                }
            }
        };
        if let Some(msg) = line.strip_prefix("TOPIC ") {
            take(&mut topic, msg);
        } else if let Some(msg) = line.strip_prefix("CONTRA ") {
            take(&mut contra, msg);
        } else if let Some(msg) = line.strip_prefix("WRAP ") {
            take(&mut wrap, msg);
        }
        // 其余行忽略
    }
    let mut issues = Vec::new();
    if let Some(m) = topic {
        issues.push(CheckinIssue { kind: CheckinIssueKind::Topic, message: m });
    }
    if let Some(m) = contra {
        issues.push(CheckinIssue { kind: CheckinIssueKind::Contra, message: m });
    }
    if let Some(m) = wrap {
        issues.push(CheckinIssue { kind: CheckinIssueKind::Wrap, message: m });
    }
    issues
}

/// 主题锚词与最近句的字符重合度（集合 Jaccard，0–1）。主题为空返回 None。
pub fn topic_match_ratio(topic: &str, sentences: &[Sentence]) -> Option<f64> {
    let topic: String = topic.trim().chars().filter(|c| !c.is_whitespace()).collect();
    if topic.is_empty() {
        return None;
    }
    let anchors: std::collections::HashSet<char> = topic.chars().collect();
    let recent: std::collections::HashSet<char> = sentences
        .iter()
        .rev()
        .take(3)
        .flat_map(|s| s.text.chars().filter(|c| !c.is_whitespace()))
        .collect();
    let inter = anchors.intersection(&recent).count() as f64;
    let union = anchors.union(&recent).count() as f64;
    if union == 0.0 {
        return None;
    }
    Some((inter / union * 100.0).round() / 100.0)
}

/// 更早 1–3 句的拼接文本（Q2 矛盾判断用），格式如「第 28–30 句：……」；无更早句返回 None
pub fn recent_before_text(sentences: &[Sentence], recent_count: usize) -> Option<String> {
    if sentences.len() <= recent_count {
        return None;
    }
    let mut before: Vec<&Sentence> = sentences
        .iter()
        .rev()
        .skip(recent_count)
        .take(RECENT_BEFORE_MAX_SENTENCES)
        .collect();
    before.reverse();
    if before.is_empty() {
        return None;
    }
    let first = before.first()?.id;
    let last = before.last()?.id;
    let joined: Vec<&str> = before.iter().map(|s| s.text.as_str()).collect();
    Some(format!("第 {first}–{last} 句：{}", joined.join("")))
}

/// 快评 user 消息 JSON（纯函数；字段与 docs/prompts/realtime-checkin.md 约定一致）
pub fn build_checkin_user_payload(
    topic: Option<&str>,
    recent: &[Sentence],
    before: Option<&str>,
    elapsed_sec: Option<u64>,
    planned_sec: Option<u64>,
    sentence_count_total: Option<u64>,
) -> Value {
    let clean_topic = topic.map(str::trim).filter(|t| !t.is_empty());
    let sentences: Vec<Value> = recent
        .iter()
        .map(|s| json!({ "id": s.id, "text": s.text }))
        .collect();
    let mut hints = json!({});
    if let Some(e) = elapsed_sec {
        hints["elapsed_sec"] = json!(e);
    }
    if let Some(p) = planned_sec {
        hints["planned_sec"] = json!(p);
    }
    if let Some(ratio) = clean_topic.and_then(|t| topic_match_ratio(t, recent)) {
        hints["topic_match_last3"] = json!(ratio);
    }
    if let Some(c) = sentence_count_total {
        hints["sentence_count_total"] = json!(c);
    }
    let mut payload = json!({ "topic": clean_topic, "sentences": sentences, "hints": hints });
    if let Some(b) = before {
        payload["recent_before"] = json!(b);
    }
    payload
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

/// 每次会话的快评运行时状态（开始会话时重置）
#[derive(Default)]
pub struct CheckinRuntime {
    /// 开始界面填写的计划时长（秒），hints 用
    pub planned_sec: Option<u64>,
    /// 同类 issue 的上次触发时间（90 秒冷却）
    pub last_fired: HashMap<String, Instant>,
    /// 连败计数 + 自动停用状态
    pub fails: FailTracker,
}

/// 连续失败计数器（纯状态机，可单测）：达到阈值返回 Some(reason) 表示
/// 「刚刚触发停用」（调用方据此发 checkin_disabled 事件）；已停用后不再重复触发。
#[derive(Debug, Default, PartialEq)]
pub struct FailTracker {
    streak: u32,
    disabled: bool,
}

impl FailTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// 记录一次失败：未停用且达到阈值 → 停用并返回 Some(原因)（只返回这一次）
    pub fn record_failure(&mut self, reason: &str) -> Option<String> {
        if self.disabled {
            return None;
        }
        self.streak += 1;
        if self.streak >= CHECKIN_MAX_CONSECUTIVE_FAILURES {
            self.disabled = true;
            return Some(reason.to_string());
        }
        None
    }

    /// 记录一次成功：清零连败（停用状态不因此恢复，需设置重新开启）
    pub fn record_success(&mut self) {
        self.streak = 0;
    }

    /// 设置重新开启：清零并恢复（会话中即时生效）
    pub fn reset(&mut self) {
        self.streak = 0;
        self.disabled = false;
    }
}

fn skipped(reason: &str) -> Value {
    json!({ "skipped": true, "reason": reason, "issues": [] })
}

/// 周期快评：把最近几句交给 LLM 只问三件事（跑偏/矛盾/该收结论了吗）。
/// 返回 `{ skipped, reason?, issues: [{kind, message}] }`；命中的 issue
/// 同时以 aiCheckin 反馈事件经 `analysis_update` 插入右栏。
#[tauri::command]
pub async fn check_in(
    app: AppHandle,
    state: State<'_, AppState>,
    topic: Option<String>,
    recent_sentence_ids: Vec<u64>,
) -> Result<Value, String> {
    // 没有新句子：跳过，避免重复付费请求
    if recent_sentence_ids.is_empty() {
        return Ok(skipped("no_new_sentences"));
    }
    let settings = crate::settings::load(&app).normalized();
    if !settings.realtime_checkin_enabled {
        return Ok(skipped("disabled"));
    }
    // 连续失败自动停用（本次会话内不再请求；设置重新开启即恢复）
    if state.checkin.lock().unwrap().fails.is_disabled() {
        return Ok(skipped("auto_disabled"));
    }
    if !settings.can_call_remote() {
        return Ok(skipped("no_remote_backend"));
    }

    let (sentences, snapshot) = state.transcript();
    if sentences.is_empty() {
        return Ok(skipped("no_sentences"));
    }
    let recent_count = RECENT_MAX_SENTENCES.min(sentences.len());
    let recent: Vec<Sentence> = sentences[sentences.len() - recent_count..].to_vec();
    let before = recent_before_text(&sentences, recent_count);
    let planned_sec = state.checkin.lock().unwrap().planned_sec;
    let user_payload = build_checkin_user_payload(
        topic.as_deref(),
        &recent,
        before.as_deref(),
        Some(snapshot.duration_ms / 1000),
        planned_sec,
        Some(snapshot.sentence_count),
    );

    let reply = match request_reply(&settings, CHECKIN_SYSTEM_PROMPT, &user_payload.to_string()).await
    {
        Ok(r) => r,
        Err(e) => {
            // 请求/解析失败计入连败；达 3 次自动停用并广播（错误照常返回，
            // 前端会把句子 id 放回下轮重试——停用后下轮会直接 skip）
            let just_disabled = {
                let mut runtime = state.checkin.lock().unwrap();
                runtime.fails.record_failure(&e)
            };
            if let Some(reason) = just_disabled {
                let _ = app.emit("checkin_disabled", json!({ "reason": reason }));
            }
            return Err(e);
        }
    };
    // 成功一次即清零连败（停用状态不变——停用后不会再走到这里）
    state.checkin.lock().unwrap().fails.record_success();
    let issues = parse_checkin_reply(&reply);

    // 同类 issue 90 秒冷却
    let surviving: Vec<CheckinIssue> = {
        let mut runtime = state.checkin.lock().unwrap();
        let now = Instant::now();
        issues
            .into_iter()
            .filter(|i| {
                let last = runtime.last_fired.get(i.kind.key());
                let cool = match last {
                    Some(t) => now.duration_since(*t) >= Duration::from_secs(CHECKIN_COOLDOWN_SECS),
                    None => true,
                };
                if cool {
                    runtime.last_fired.insert(i.kind.key().to_string(), now);
                }
                cool
            })
            .collect()
    };

    let events: Vec<FeedbackEvent> = surviving
        .iter()
        .map(|i| FeedbackEvent {
            kind: FeedbackKind::AiCheckin,
            sentence_id: None,
            message: i.message.clone(),
            payload: json!({ "issueType": i.kind.key() }),
        })
        .collect();
    if !events.is_empty() {
        let _ = app.emit(
            "analysis_update",
            json!({ "events": events, "snapshot": snapshot }),
        );
    }
    Ok(json!({ "skipped": false, "issues": surviving }))
}

/// 设置页把「AI 周期快评」重新打开时调用：清零连败并解除本次会话的自动停用。
/// （快评开关本身的判定在每次 check_in 里读最新设置，无需前端传参。）
#[tauri::command]
pub fn reset_checkin(state: State<'_, AppState>) {
    state.checkin.lock().unwrap().fails.reset();
}

/// 非流式 OpenAI 兼容请求，返回 assistant 文本
async fn request_reply(
    settings: &Settings,
    system: &str,
    user_content: &str,
) -> Result<String, String> {
    let (base_url, model) = settings
        .resolve_endpoint()
        .ok_or_else(|| "后端地址未配置（自定义后端需填写 baseURL）".to_string())?;
    let body = json!({
        "model": model,
        "stream": false,
        "temperature": 0.0,
        "max_tokens": 120,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user_content },
        ],
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body.to_string());
    if !settings.api_key.trim().is_empty() {
        req = req.bearer_auth(settings.api_key.trim());
    }
    let resp = req.send().await.map_err(|e| format!("快评请求失败：{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("快评 HTTP {status}：{}", truncate_chars(text.trim(), 200)));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("快评响应读取失败：{e}"))?;
    let v: Value =
        serde_json::from_str(&text).map_err(|e| format!("快评响应解析失败：{e}"))?;
    Ok(v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str) -> Sentence {
        Sentence { id, text: text.into(), start_ms: 0, end_ms: id * 1000 }
    }

    #[test]
    fn embedded_prompt_present() {
        assert!(CHECKIN_SYSTEM_PROMPT.contains("实时陪练检查器"));
        assert!(CHECKIN_SYSTEM_PROMPT.contains("宁可漏报，不可刷屏"));
        assert!(CHECKIN_SYSTEM_PROMPT.contains("TOPIC"));
    }

    #[test]
    fn parse_ok_reply_is_silent() {
        assert!(parse_checkin_reply("OK").is_empty());
        assert!(parse_checkin_reply("  \nOK\n").is_empty());
        assert!(parse_checkin_reply("").is_empty());
        assert!(parse_checkin_reply("   ").is_empty());
    }

    #[test]
    fn parse_three_lines_in_fixed_order() {
        let reply = "TOPIC 这几句在讲室友矛盾，与主题无关\n\
                     CONTRA 第 28 句说每天六点起床，第 33 句说睡到自然醒\n\
                     WRAP 观点已讲透且开始重复，建议收结论";
        let issues = parse_checkin_reply(reply);
        assert_eq!(issues.len(), 3);
        assert_eq!(issues[0].kind, CheckinIssueKind::Topic);
        assert_eq!(issues[1].kind, CheckinIssueKind::Contra);
        assert_eq!(issues[2].kind, CheckinIssueKind::Wrap);
        assert!(issues[0].message.contains("室友矛盾"));
    }

    #[test]
    fn parse_ignores_malformed_lines() {
        // 无前缀、前缀无空格、前缀后为空、OK 混排 → 全部忽略
        let reply = "好的，检查结果如下\nTOPIC\nCONTRA\n随便一行\nOK\nTOPIC 只有这行有效";
        let issues = parse_checkin_reply(reply);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, CheckinIssueKind::Topic);
        assert_eq!(issues[0].message, "只有这行有效");
    }

    #[test]
    fn parse_truncates_overlong_line() {
        let long = "字".repeat(100);
        let reply = format!("WRAP {long}");
        let issues = parse_checkin_reply(&reply);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].message.chars().count(), MESSAGE_MAX_CHARS + 1); // 60 字 + 省略号
        assert!(issues[0].message.ends_with('…'));
    }

    #[test]
    fn parse_keeps_first_occurrence_of_duplicated_prefix() {
        let reply = "TOPIC 第一次\nTOPIC 第二次";
        let issues = parse_checkin_reply(reply);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].message, "第一次");
    }

    #[test]
    fn topic_match_ratio_variants() {
        let s = vec![sent(1, "我每天六点起床"), sent(2, "自律靠环境设计"), sent(3, "闹钟放到客厅")];
        // 主题锚词「自律」的字符全部出现
        let ratio = topic_match_ratio("自律", &s).unwrap();
        assert!(ratio > 0.0 && ratio <= 1.0);
        // 完全无重合 → 0
        assert_eq!(topic_match_ratio("量子纠缠", &[sent(1, "今天天气不错")]).unwrap(), 0.0);
        // 空主题 → None（Q1 永远为否）
        assert!(topic_match_ratio("  ", &s).is_none());
        assert!(topic_match_ratio("", &s).is_none());
    }

    #[test]
    fn recent_before_text_joins_earlier_sentences() {
        let s: Vec<Sentence> = (1..=8).map(|i| sent(i, &format!("第{i}句内容"))).collect();
        let text = recent_before_text(&s, 5).unwrap();
        assert!(text.starts_with("第 1–3 句："));
        assert!(text.contains("第3句内容"));
        // 更早句不足时返回 None
        assert!(recent_before_text(&s[..5], 5).is_none());
    }

    #[test]
    fn build_payload_shape_matches_prompt_contract() {
        let recent = vec![sent(28, "我每天六点起床"), sent(29, "这个习惯让我清醒")];
        let payload = build_checkin_user_payload(
            Some("自律"),
            &recent,
            Some("第 25–27 句：……"),
            Some(95),
            Some(180),
            Some(31),
        );
        assert_eq!(payload["topic"], "自律");
        assert_eq!(payload["sentences"][0]["id"], 28);
        assert_eq!(payload["recent_before"], "第 25–27 句：……");
        assert_eq!(payload["hints"]["elapsed_sec"], 95);
        assert_eq!(payload["hints"]["planned_sec"], 180);
        assert!(payload["hints"].get("topic_match_last3").is_some());
        assert_eq!(payload["hints"]["sentence_count_total"], 31);
    }

    #[test]
    fn build_payload_omits_optionals() {
        let payload = build_checkin_user_payload(None, &[], None, None, None, None);
        assert!(payload["topic"].is_null());
        assert!(payload.get("recent_before").is_none());
        assert!(payload["hints"].as_object().unwrap().is_empty());
    }

    #[test]
    fn truncate_chars_handles_short_and_long() {
        assert_eq!(truncate_chars("短句", 10), "短句");
        let cut = truncate_chars(&"字".repeat(80), 60);
        assert_eq!(cut.chars().count(), 61);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn fail_tracker_disables_exactly_at_threshold() {
        let mut t = FailTracker::new();
        assert!(!t.is_disabled());
        // 两次失败：只计数不停用
        assert_eq!(t.record_failure("超时"), None);
        assert_eq!(t.record_failure("HTTP 500"), None);
        assert!(!t.is_disabled());
        // 第三次：触发停用，返回这一次的原因（供 checkin_disabled 事件）
        assert_eq!(t.record_failure("连接失败"), Some("连接失败".into()));
        assert!(t.is_disabled());
        // 已停用后再失败：不重复触发
        assert_eq!(t.record_failure("又失败"), None);
        assert!(t.is_disabled());
    }

    #[test]
    fn fail_tracker_success_resets_streak_but_not_disabled() {
        let mut t = FailTracker::new();
        t.record_failure("a");
        t.record_failure("b");
        t.record_success(); // 连胜清零
        assert_eq!(t.record_failure("c"), None); // 重新从 1 计
        assert_eq!(t.record_failure("d"), None);
        assert_eq!(t.record_failure("e"), Some("e".into())); // 第 3 次才停用
        assert!(t.is_disabled());
        // 停用后成功不清停用（只有设置重新开启 = reset 才恢复）
        t.record_success();
        assert!(t.is_disabled());
        t.reset();
        assert!(!t.is_disabled());
    }

    #[test]
    fn fail_tracker_never_fires_below_threshold_after_reset() {
        let mut t = FailTracker::new();
        t.record_failure("x");
        t.record_failure("y");
        t.reset();
        assert!(!t.is_disabled());
        assert_eq!(t.record_failure("z"), None);
        assert!(!t.is_disabled());
        assert_eq!(CHECKIN_MAX_CONSECUTIVE_FAILURES, 3);
    }
}
