//! AI 模拟面试：出题命令（LLM 优先，离线题库降级）。
//!
//! 流程：前端驱动逐题复用现有会话管线（start_session / stop_session），
//! 每题的逐字稿与快照在前端累积；全部题目答完后走 generate_report
//! （scenario = mockInterview + qa 注入）出面试报告。本模块只负责出题：
//! - 配置了远端后端：按岗位 + JD 调 LLM 出题（非流式），解析失败重试一次；
//! - 无 Key / 请求失败 / 两次解析失败：内置离线题库随机抽取（interview_bank）。
//! 返回值带 source 字段（"ai" / "offline"），供前端提示当前题库来源。

use crate::interview_bank::{self, BankQuestion};
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

/// docs/prompts 约定的出题 system prompt 原样内嵌
pub const INTERVIEW_QUESTIONS_PROMPT: &str = include_str!("../prompts/interview_questions.md");

/// 岗位方向白名单（与前端 src/lib/interview.ts 的 INTERVIEW_ROLES 同步）
pub const ROLES: &[&str] = &["general", "backend", "frontend", "product", "ops", "management", "custom"];

/// 题数白名单
pub const QUESTION_COUNTS: &[u32] = &[3, 5, 8];

/// 一道面试题（前端逐题展示用；index 从 1 开始）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InterviewQuestion {
    pub index: u32,
    pub question: String,
    pub intent: String,
}

/// generate_interview_questions 的返回值
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InterviewQuestionsResult {
    /// "ai" = LLM 出题；"offline" = 内置离线题库
    pub source: String,
    pub questions: Vec<InterviewQuestion>,
    pub role: String,
}

/// 岗位中文名（user payload 与前端提示用；custom 走 JD 语境）
pub fn role_label(role: &str) -> &'static str {
    match role {
        "backend" => "后端开发",
        "frontend" => "前端开发",
        "product" => "产品经理",
        "ops" => "运营",
        "management" => "管理岗位",
        "custom" => "自定义岗位",
        _ => "通用面试",
    }
}

/// 从 LLM 回复中解析题目数组（纯函数，容忍围栏/前后杂文字）：
/// - 剥掉 ``` 围栏，取首个 `[` 到末个 `]` 的片段按 JSON 数组解析；
/// - 每项须有非空 question（intent 缺省补"综合考察"），index 统一重排为 1..=n；
/// - 数量必须恰好等于 expected_count，题面去重后仍须等于 expected_count；
/// - 任何一步失败返回 None（调用方重试一次再降级）。
pub fn parse_questions_reply(reply: &str, expected_count: u32) -> Option<Vec<InterviewQuestion>> {
    let trimmed = reply.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .strip_suffix("```")
        .unwrap_or(trimmed)
        .trim();
    let start = trimmed.find('[')?;
    let end = trimmed.rfind(']')?;
    if end <= start {
        return None;
    }
    let raw = &trimmed[start..=end];
    let items: Vec<Value> = serde_json::from_str(raw).ok()?;
    let mut out: Vec<InterviewQuestion> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for v in items {
        let question = v.get("question")?.as_str()?.trim().to_string();
        if question.is_empty() || !seen.insert(question.clone()) {
            return None;
        }
        let intent = v
            .get("intent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("综合考察")
            .to_string();
        out.push(InterviewQuestion { index: 0, question, intent });
    }
    if out.len() as u32 != expected_count {
        return None;
    }
    for (i, q) in out.iter_mut().enumerate() {
        q.index = i as u32 + 1;
    }
    Some(out)
}

/// 出题 user 消息 JSON（纯函数；字段与 interview_questions.md 约定一致）
pub fn build_questions_payload(role: &str, count: u32, jd: Option<&str>) -> Value {
    let mut payload = json!({
        "role": role,
        "role_label": role_label(role),
        "number": count,
    });
    let jd = jd.map(str::trim).filter(|s| !s.is_empty());
    if let Some(jd) = jd {
        payload["jd"] = json!(jd);
    }
    payload
}

/// 非流式出题请求（OpenAI 兼容；出题要多样性，temperature 高于报告）
async fn request_questions_reply(
    settings: &Settings,
    user_content: &str,
) -> Result<String, String> {
    let (base_url, model) = settings
        .resolve_endpoint()
        .ok_or_else(|| "后端地址未配置（自定义后端需填写 baseURL）".to_string())?;
    let body = json!({
        "model": model,
        "stream": false,
        "temperature": 0.7,
        "max_tokens": 2400,
        "messages": [
            { "role": "system", "content": INTERVIEW_QUESTIONS_PROMPT },
            { "role": "user", "content": user_content },
        ],
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
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
    let resp = req.send().await.map_err(|e| format!("出题请求失败：{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("出题 HTTP {status}：{}", text.trim().chars().take(200).collect::<String>()));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("出题响应读取失败：{e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("出题响应解析失败：{e}"))?;
    Ok(v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// LLM 出题（含一次重试；两次失败返回 Err 由调用方降级）
async fn fetch_ai_questions(
    settings: &Settings,
    role: &str,
    count: u32,
    jd: Option<&str>,
) -> Result<Vec<InterviewQuestion>, String> {
    let payload = build_questions_payload(role, count, jd).to_string();
    let mut last_err = String::new();
    for _ in 0..2 {
        match request_questions_reply(settings, &payload).await {
            Ok(reply) => match parse_questions_reply(&reply, count) {
                Some(qs) => return Ok(qs),
                None => last_err = "出题结果解析失败（模型未按 JSON 数组返回）".into(),
            },
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// 离线题库随机抽取（seed 取自系统时钟；custom 无内置库，回落通用库）
pub fn offline_questions(role: &str, count: u32, seed: u64) -> Vec<InterviewQuestion> {
    let bank: &[BankQuestion] = interview_bank::bank_for(role).unwrap_or(interview_bank::GENERAL);
    interview_bank::pick_offline(bank, count as usize, seed)
        .into_iter()
        .map(|(index, question, intent)| InterviewQuestion {
            index,
            question: question.to_string(),
            intent: intent.to_string(),
        })
        .collect()
}

/// 生成模拟面试题目：LLM 优先（解析失败重试一次），无 Key / 两次失败走内置离线题库。
/// 离线降级不报错——返回 source="offline"，由前端给一行灰字提示。
#[tauri::command]
pub async fn generate_interview_questions(
    app: AppHandle,
    role: String,
    count: u32,
    jd: Option<String>,
) -> Result<InterviewQuestionsResult, String> {
    if !ROLES.contains(&role.as_str()) {
        return Err(format!("未知岗位方向：{role}（支持 {}）", ROLES.join(" / ")));
    }
    if !QUESTION_COUNTS.contains(&count) {
        return Err(format!("题数仅支持 3 / 5 / 8（收到 {count}）"));
    }
    if role == "custom" && jd.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_none() {
        return Err("自定义岗位必须填写职位描述（JD）".into());
    }

    let settings = crate::settings::load(&app).normalized();
    if settings.can_call_remote() {
        if let Ok(questions) = fetch_ai_questions(&settings, &role, count, jd.as_deref()).await {
            return Ok(InterviewQuestionsResult { source: "ai".into(), questions, role });
        }
        // 两次失败：静默降级离线题库（下方统一返回）
    }
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    Ok(InterviewQuestionsResult {
        source: "offline".into(),
        questions: offline_questions(&role, count, seed),
        role,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_prompt_present() {
        assert!(INTERVIEW_QUESTIONS_PROMPT.contains("模拟面试出题官"));
        assert!(INTERVIEW_QUESTIONS_PROMPT.contains("JSON 数组"));
        assert!(INTERVIEW_QUESTIONS_PROMPT.contains("number"));
        assert!(INTERVIEW_QUESTIONS_PROMPT.contains("jd"));
        assert!(INTERVIEW_QUESTIONS_PROMPT.contains("intent"));
    }

    #[test]
    fn role_labels_cover_whitelist() {
        assert_eq!(role_label("general"), "通用面试");
        assert_eq!(role_label("backend"), "后端开发");
        assert_eq!(role_label("custom"), "自定义岗位");
        assert_eq!(role_label("unknown"), "通用面试");
        assert_eq!(ROLES.len(), 7);
        assert_eq!(QUESTION_COUNTS, &[3, 5, 8]);
    }

    #[test]
    fn parse_plain_array_and_reindexes() {
        let reply = r#"[
            {"index": 5, "question": "第一题？", "intent": "自我介绍"},
            {"index": 9, "question": "第二题？", "intent": "项目深挖"}
        ]"#;
        let qs = parse_questions_reply(reply, 2).unwrap();
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].index, 1); // index 统一重排
        assert_eq!(qs[1].index, 2);
        assert_eq!(qs[0].question, "第一题？");
        assert_eq!(qs[1].intent, "项目深挖");
    }

    #[test]
    fn parse_tolerates_fences_and_surrounding_text() {
        let reply = "好的，以下是题目：\n```json\n[{\"question\":\"题一？\",\"intent\":\"动机\"},{\"question\":\"题二？\"}]\n```\n以上。";
        let qs = parse_questions_reply(reply, 2).unwrap();
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[1].intent, "综合考察"); // intent 缺省补默认
    }

    #[test]
    fn parse_rejects_bad_outputs() {
        // 数量不符
        assert!(parse_questions_reply(r#"[{"question":"只有一道？","intent":"a"}]"#, 3).is_none());
        // 空 question / 重复题面
        assert!(parse_questions_reply(r#"[{"question":"  ","intent":"a"},{"question":"x？","intent":"b"}]"#, 2).is_none());
        assert!(parse_questions_reply(r#"[{"question":"同题？","intent":"a"},{"question":"同题？","intent":"b"}]"#, 2).is_none());
        // 非 JSON / 无数组 / 畸形 JSON
        assert!(parse_questions_reply("不会出题", 3).is_none());
        assert!(parse_questions_reply(r#"{"question":"x？"}"#, 3).is_none());
        assert!(parse_questions_reply("[{bad json}]", 3).is_none());
        assert!(parse_questions_reply("", 3).is_none());
    }

    #[test]
    fn questions_payload_shape() {
        let p = build_questions_payload("backend", 5, Some(" 负责推荐系统 "));
        assert_eq!(p["role"], "backend");
        assert_eq!(p["role_label"], "后端开发");
        assert_eq!(p["number"], 5);
        assert_eq!(p["jd"], "负责推荐系统");
        // jd 缺省 / 空白：字段省略
        let p = build_questions_payload("general", 3, None);
        assert!(p.get("jd").is_none());
        let p = build_questions_payload("general", 3, Some("   "));
        assert!(p.get("jd").is_none());
    }

    #[test]
    fn offline_questions_use_role_bank_and_cap_count() {
        let qs = offline_questions("backend", 5, 42);
        assert_eq!(qs.len(), 5);
        assert_eq!(qs[0].index, 1);
        // 题面确实来自后端题库
        let bank_q: std::collections::HashSet<&str> =
            interview_bank::BACKEND.iter().map(|b| b.question).collect();
        assert!(qs.iter().all(|q| bank_q.contains(q.question.as_str())));
        // custom 回落通用库
        let general = offline_questions("custom", 3, 7);
        assert_eq!(general.len(), 3);
        let general_q: std::collections::HashSet<&str> =
            interview_bank::GENERAL.iter().map(|b| b.question).collect();
        assert!(general.iter().all(|q| general_q.contains(q.question.as_str())));
    }
}
