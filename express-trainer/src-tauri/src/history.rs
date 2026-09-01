//! 成长档案（M3 · 产品方案 §5.6，独有能力 ②）。
//!
//! 会话结束（报告生成成功或本地降级报告时）自动落盘
//! `appdata/sessions/YYYY-MM-DD-HHmmss.json`：
//! `{ id, date, scenario, topic, snapshot(含声音), transcript, report, aiBackendUsed }`。
//!
//! 命令：`list_sessions`（摘要列表）/ `get_session`（全量）/ `delete_session`。
//! 纯 IO 函数以目录为参数（可测），命令层薄封装。
//!
//! 纪律：历史写入失败绝不影响主流程——静默降级，仅发一次 `history_error` 事件提示。

use crate::rules::engine::SessionSnapshot;
use crate::rules::Sentence;
use crate::voice::VoiceMetrics;
use crate::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager};

// ---------------------------------------------------------------------------
// 数据结构
// ---------------------------------------------------------------------------

/// 一次会话的完整落盘记录（camelCase，文件即此 JSON）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    /// 文件名主干（YYYY-MM-DD-HHmmss），同会话重试报告时复用同一 id 覆盖
    pub id: String,
    /// 本地日期时间 "YYYY-MM-DD HH:MM:SS"
    pub date: String,
    /// free / interview / vlog / workreport
    pub scenario: String,
    /// 练习主题/面试题，可为空串
    pub topic: String,
    /// 会话来源：mic（实时麦克风）/ file（从文件练习）；旧记录缺省为 mic
    #[serde(default = "default_source")]
    pub source: String,
    /// 从文件练习时的源音频文件名；麦克风会话省略
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    /// 会话录音 wav 的绝对路径（appdata/sessions/audio/）；
    /// 录音关闭 / 超 30 分钟截断 / 写失败的记录为 None（回放按钮隐藏）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_file: Option<String>,
    /// 报告评分（report 末尾 SCORE 标记解析出的 JSON：{"overall":0-100,维度:分,…}）；
    /// 本地降级报告与旧记录为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scores: Option<Value>,
    /// 报告模式：full（完整八节）/ quick（快速四节，0.2.3 起）；旧记录缺省 full
    #[serde(default = "default_report_mode")]
    pub report_mode: String,
    /// 统计快照（含声音层 voice 字段）
    pub snapshot: SessionSnapshot,
    /// 送报告的逐字稿（用户修正稿优先）
    pub transcript: Vec<Sentence>,
    /// 报告全文（Markdown）
    pub report: String,
    /// 生成报告用的后端：deepseek/openai/groq/ollama/custom，本地降级为 "local"
    pub ai_backend_used: String,
}

fn default_source() -> String {
    "mic".into()
}

fn default_report_mode() -> String {
    "full".into()
}

/// 历史列表的摘要行
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub date: String,
    pub scenario: String,
    pub topic: String,
    /// mic / file
    pub source: String,
    /// 从文件练习时的源音频文件名（麦克风为 null）
    pub file_name: Option<String>,
    /// 会话录音 wav 路径（无录音为 null；列表透传，回放入口在详情页）
    pub audio_file: Option<String>,
    pub duration_ms: u64,
    pub filler_per_minute: f64,
    pub speech_rate: f64,
    /// 失控停顿次数（声音层）
    pub runaway_pause_count: u32,
    /// 最长失控停顿（毫秒）
    pub longest_pause_ms: u64,
    /// 报告总分等评分（无评分的记录为 None；旧记录缺省 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scores: Option<Value>,
}

/// 喂给下一次报告的"上一次会话"摘要（user JSON 内 snake_case）
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PreviousSession {
    pub date: String,
    pub filler_per_minute: f64,
    pub speech_rate: f64,
    pub avg_sentence_chars: f64,
}

impl PreviousSession {
    pub fn from_record(r: &SessionRecord) -> Self {
        Self {
            date: r.date.clone(),
            filler_per_minute: r.snapshot.filler_per_minute,
            speech_rate: r.snapshot.speech_rate,
            avg_sentence_chars: r.snapshot.avg_sentence_chars,
        }
    }
}

// ---------------------------------------------------------------------------
// 纯 IO（目录参数化，可单测）
// ---------------------------------------------------------------------------

pub fn timestamp_id() -> String {
    chrono::Local::now().format("%Y-%m-%d-%H%M%S").to_string()
}

pub fn local_datetime_now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// id 只允许 [A-Za-z0-9-_]，防路径穿越
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn record_path(dir: &Path, id: &str) -> Option<PathBuf> {
    if valid_id(id) {
        Some(dir.join(format!("{id}.json")))
    } else {
        None
    }
}

/// 落盘一条会话记录（返回 id）；目录不存在则创建
pub fn save_record(dir: &Path, record: &SessionRecord) -> Result<String, String> {
    let path = record_path(dir, &record.id)
        .ok_or_else(|| format!("非法会话 id：{}", record.id))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("创建历史目录失败：{e}"))?;
    let body = serde_json::to_string_pretty(record)
        .map_err(|e| format!("序列化历史记录失败：{e}"))?;
    std::fs::write(&path, body).map_err(|e| format!("写入历史文件失败：{e}"))?;
    Ok(record.id.clone())
}

/// 读取一条完整记录
pub fn read_record(dir: &Path, id: &str) -> Result<SessionRecord, String> {
    let path = record_path(dir, id).ok_or_else(|| format!("非法会话 id：{id}"))?;
    let body = std::fs::read_to_string(&path).map_err(|_| format!("历史记录不存在：{id}"))?;
    serde_json::from_str(&body).map_err(|e| format!("历史记录解析失败：{e}"))
}

/// 删除一条记录
pub fn delete_record(dir: &Path, id: &str) -> Result<(), String> {
    let path = record_path(dir, id).ok_or_else(|| format!("非法会话 id：{id}"))?;
    std::fs::remove_file(&path).map_err(|_| format!("删除失败（文件不存在？）：{id}"))
}

/// 删除记录并连带删除其会话录音 wav。音频删除是尽力而为：
/// 路径缺失 / 不在 `<dir>/audio` 目录内（防篡改记录指向任意文件）/ 删除失败
/// 都不影响记录本身的删除。
pub fn delete_record_and_audio(dir: &Path, id: &str) -> Result<(), String> {
    let audio = read_record(dir, id).ok().and_then(|r| r.audio_file);
    delete_record(dir, id)?;
    if let Some(audio) = audio {
        let audio_dir = dir.join("audio");
        let path = std::path::Path::new(&audio);
        // 只删本应用录音目录里的文件（记录被篡改时不能变成任意文件删除器）
        if path.parent() == Some(audio_dir.as_path()) {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

/// 摘要列表：解析失败的文件静默跳过；按 id 倒序（新→旧）
pub fn list_summaries(dir: &Path) -> Vec<SessionSummary> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut summaries: Vec<SessionSummary> = entries
        .flatten()
        .filter_map(|e| e.path().to_str().map(|s| s.to_string()))
        .filter(|p| p.ends_with(".json"))
        .filter_map(|p| std::fs::read_to_string(&p).ok())
        .filter_map(|body| serde_json::from_str::<SessionRecord>(&body).ok())
        .map(summary_of)
        .collect();
    summaries.sort_by(|a, b| b.id.cmp(&a.id));
    summaries
}

fn summary_of(r: SessionRecord) -> SessionSummary {
    let (pauses, longest) = r
        .snapshot
        .voice
        .map(|v| (v.runaway_pause_count, v.longest_pause_ms))
        .unwrap_or((0, 0));
    SessionSummary {
        id: r.id,
        date: r.date,
        scenario: r.scenario,
        topic: r.topic,
        source: r.source,
        file_name: r.file_name,
        audio_file: r.audio_file,
        duration_ms: r.snapshot.duration_ms,
        filler_per_minute: r.snapshot.filler_per_minute,
        speech_rate: r.snapshot.speech_rate,
        runaway_pause_count: pauses,
        longest_pause_ms: longest,
        scores: r.scores,
    }
}

/// 上一次会话（供报告 previous）：按 id 倒序取第一条，排除当前会话已落盘的记录
pub fn previous_session(dir: &Path, exclude_id: Option<&str>) -> Option<PreviousSession> {
    // 摘要行缺 avg_sentence_chars，需读完整记录
    let ids: Vec<String> = list_summaries(dir)
        .into_iter()
        .filter(|s| Some(s.id.as_str()) != exclude_id)
        .map(|s| s.id)
        .collect();
    let first = ids.first()?;
    read_record(dir, first).ok().map(|r| PreviousSession::from_record(&r))
}

// ---------------------------------------------------------------------------
// 孤儿音频清理（技术债 C2）
// ---------------------------------------------------------------------------

/// 全部历史记录引用的录音路径集合（小写规范化，Windows 大小写不敏感）。
/// 模拟面试逐题会话不落历史、录音只用最后一题——其余题的录音即孤儿，
/// 由 cleanup_orphan_audio 按超龄规则清理。
pub fn referenced_audio_files(dir: &Path) -> HashSet<String> {
    list_summaries(dir)
        .into_iter()
        .filter_map(|s| s.audio_file)
        .map(|p| p.to_lowercase())
        .collect()
}

/// 删除录音目录下：未被引用 且 修改时间早于 max_age 的 wav。
/// 返回删除个数；任何失败静默跳过（清理绝不影响启动）。
pub fn cleanup_orphan_audio(
    audio_dir: &Path,
    referenced: &HashSet<String>,
    max_age: Duration,
    now: SystemTime,
) -> usize {
    let Ok(entries) = std::fs::read_dir(audio_dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_wav = path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("wav"))
            .unwrap_or(false);
        if !is_wav {
            continue;
        }
        if referenced.contains(&path.to_string_lossy().to_lowercase()) {
            continue; // 被历史记录引用：不删
        }
        let over_age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age >= max_age);
        if over_age && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 应用启动时调用：清理 sessions/audio/ 下不被任何历史引用且超 7 天的 wav。
pub fn cleanup_orphan_audio_on_startup(sessions_dir: &Path) {
    let referenced = referenced_audio_files(sessions_dir);
    cleanup_orphan_audio(
        &sessions_dir.join("audio"),
        &referenced,
        Duration::from_secs(7 * 24 * 3600),
        SystemTime::now(),
    );
}

// ---------------------------------------------------------------------------
// 命令层
// ---------------------------------------------------------------------------

pub fn sessions_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("sessions")
}

/// 报告落盘所需的一次性会话快照：generate_report 命令入口处捕获。
/// 流式报告可持续数分钟，期间用户可能已开始下一场会话（AppState 里的
/// 历史 id / 来源 / 录音路径会被新会话重置）——落盘必须用报告开始时的
/// 捕获值，否则会把旧报告错记到新会话名下（来源/录音错配、重复建档）。
#[derive(Debug, Clone, Default)]
pub struct SessionPersistInfo {
    /// 本次会话已落盘的历史 id（报告重试覆盖用；首报为 None）
    pub existing_id: Option<String>,
    /// 会话来源（mic/file + 文件名）
    pub meta: crate::SessionMeta,
    /// 会话录音 wav 路径（无录音为 None）
    pub audio_file: Option<String>,
}

/// 从 AppState 捕获落盘所需的会话侧数据（generate_report 入口调用一次）
pub fn capture_session_persist_info(state: &crate::AppState) -> SessionPersistInfo {
    SessionPersistInfo {
        existing_id: state
            .current_history_id
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone(),
        meta: state
            .session_meta
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone(),
        audio_file: state
            .last_audio
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone(),
    }
}

/// 由捕获的会话数据 + 报告内容组装历史记录（纯函数，可单测）。
/// 只读入参（不碰 AppState）：报告生成期间 AppState 已可能被新会话重置。
#[allow(clippy::too_many_arguments)]
pub fn build_session_record(
    info: &SessionPersistInfo,
    scenario: &str,
    topic: Option<&str>,
    sentences: &[Sentence],
    snapshot: &SessionSnapshot,
    voice: &VoiceMetrics,
    report: &str,
    ai_backend_used: &str,
    scores: Option<&Value>,
    report_mode: &str,
) -> SessionRecord {
    let mut snap = snapshot.clone();
    snap.voice = Some(voice.clone());
    SessionRecord {
        id: info.existing_id.clone().unwrap_or_else(timestamp_id),
        date: local_datetime_now(),
        scenario: scenario.to_string(),
        topic: topic.unwrap_or("").trim().to_string(),
        source: info.meta.source.clone(),
        file_name: info.meta.file_name.clone(),
        audio_file: info.audio_file.clone(),
        scores: scores.cloned(),
        report_mode: report_mode.to_string(),
        snapshot: snap,
        transcript: sentences.to_vec(),
        report: report.to_string(),
        ai_backend_used: ai_backend_used.to_string(),
    }
}

#[tauri::command]
pub fn list_sessions(app: AppHandle) -> Result<Vec<SessionSummary>, String> {
    Ok(list_summaries(&sessions_dir(&app)))
}

#[tauri::command]
pub fn get_session(app: AppHandle, id: String) -> Result<SessionRecord, String> {
    read_record(&sessions_dir(&app), &id)
}

#[tauri::command]
pub fn delete_session(app: AppHandle, id: String) -> Result<(), String> {
    delete_record_and_audio(&sessions_dir(&app), &id)
}

/// 报告生成成功后调用：落盘本次会话；失败静默降级 + `history_error` 事件提示一次。
/// 同一会话重试报告复用同一 id 覆盖写入（不产生重复历史）。
/// `info` 必须传 generate_report 入口捕获的快照（见 SessionPersistInfo），
/// 不能在这里现读 AppState——报告流式生成期间新会话可能已重置这些字段。
#[allow(clippy::too_many_arguments)]
pub fn save_session_after_report(
    app: &AppHandle,
    state: &AppState,
    info: &SessionPersistInfo,
    scenario: &str,
    topic: Option<&str>,
    sentences: &[Sentence],
    snapshot: &SessionSnapshot,
    voice: &VoiceMetrics,
    report: &str,
    ai_backend_used: &str,
    scores: Option<&Value>,
    report_mode: &str,
) {
    let record = build_session_record(
        info,
        scenario,
        topic,
        sentences,
        snapshot,
        voice,
        report,
        ai_backend_used,
        scores,
        report_mode,
    );
    let result = save_record(&sessions_dir(app), &record);
    match result {
        Ok(id) => {
            *state
                .current_history_id
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = Some(id.clone());
            // 词库自生长：落历史同机统计逐字稿候选词（同 id 重试不重复计数；失败静默）
            crate::growth::record_session_candidates(app, &id, sentences);
        }
        Err(e) => {
            // 主流程不受影响：仅提示一次
            let _ = app.emit("history_error", json!({ "message": e }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        // 每个测试独立目录（并行执行不能共享 temp 路径）
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("sm-history-test-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_record(id: &str, chars: u64, filler_pm: f64) -> SessionRecord {
        sample_record_with_source(id, chars, filler_pm, "mic", None)
    }

    fn sample_record_with_source(
        id: &str,
        chars: u64,
        filler_pm: f64,
        source: &str,
        file_name: Option<&str>,
    ) -> SessionRecord {
        sample_record_full(id, chars, filler_pm, source, file_name, None)
    }

    fn sample_record_full(
        id: &str,
        chars: u64,
        filler_pm: f64,
        source: &str,
        file_name: Option<&str>,
        audio_file: Option<&str>,
    ) -> SessionRecord {
        let mut engine = crate::rules::engine::RuleEngine::new();
        engine.start(0);
        engine.ingest(Sentence {
            id: 1,
            text: "然后我想说几句话".into(),
            start_ms: 0,
            end_ms: 60_000,
        });
        let mut snap = engine.snapshot();
        snap.total_chars = chars;
        snap.filler_per_minute = filler_pm;
        snap.speech_rate = 210.0;
        snap.avg_sentence_chars = 18.0;
        snap.voice = Some(VoiceMetrics {
            baseline_calibrated: true,
            volume_dynamic_range_db: Some(8.4),
            energy_stability: Some(1.2),
            runaway_pause_count: 2,
            longest_pause_ms: 3_100,
        });
        SessionRecord {
            id: id.into(),
            date: format!("2026-08-30 1{}:00:00", id.chars().last().unwrap()),
            scenario: "free".into(),
            topic: "自律".into(),
            source: source.into(),
            file_name: file_name.map(str::to_string),
            audio_file: audio_file.map(str::to_string),
            scores: None,
            report_mode: "full".into(),
            snapshot: snap,
            transcript: vec![Sentence {
                id: 1,
                text: "然后我想说几句话".into(),
                start_ms: 0,
                end_ms: 60_000,
            }],
            report: "# 报告\n\n内容".into(),
            ai_backend_used: "local".into(),
        }
    }

    // --- 报告落盘：入口捕获的会话快照 ----------------------------------------

    #[test]
    fn build_session_record_uses_captured_info_not_live_state() {
        // generate_report 入口捕获：来自文件会话、已有历史 id（报告重试）、有录音
        let info = SessionPersistInfo {
            existing_id: Some("2026-08-30-100000".into()),
            meta: crate::SessionMeta {
                source: "file".into(),
                file_name: Some("lei-jun-test.wav".into()),
            },
            audio_file: Some("X:/sessions/audio/a.wav".into()),
        };
        let sentences = vec![Sentence {
            id: 1,
            text: "然后我想说".into(),
            start_ms: 0,
            end_ms: 30_000,
        }];
        let mut snap = sample_record("x", 10, 1.0).snapshot;
        snap.voice = None; // 声音终值由 save 时并入
        let voice = VoiceMetrics {
            baseline_calibrated: true,
            volume_dynamic_range_db: None,
            energy_stability: None,
            runaway_pause_count: 1,
            longest_pause_ms: 2_400,
        };
        let record = build_session_record(
            &info,
            "free",
            Some("  自律  "),
            &sentences,
            &snap,
            &voice,
            "# 报告",
            "local",
            None,
            "quick",
        );
        // 记录内容全部来自捕获快照
        assert_eq!(record.id, "2026-08-30-100000");
        assert_eq!(record.source, "file");
        assert_eq!(record.file_name.as_deref(), Some("lei-jun-test.wav"));
        assert_eq!(record.audio_file.as_deref(), Some("X:/sessions/audio/a.wav"));
        assert_eq!(record.topic, "自律");
        assert_eq!(record.transcript.len(), 1);
        // 报告模式原样入档
        assert_eq!(record.report_mode, "quick");
        // 声音终值并入快照
        assert_eq!(record.snapshot.voice.as_ref().unwrap().runaway_pause_count, 1);

        // 捕获之后 AppState 被新会话重置（id 清空、来源换 mic、录音清空）
        // 不影响已捕获的 info 与由它组装的记录（结构上只读入参，这里锁行为）
        let stale_info = info.clone();
        let _ = &stale_info;
        assert_eq!(stale_info.existing_id.as_deref(), Some("2026-08-30-100000"));
    }

    #[test]
    fn build_session_record_mints_fresh_id_when_no_existing() {
        // 首次报告：无 existing_id → 新时间戳 id（合法、非空）
        let info = SessionPersistInfo {
            existing_id: None,
            meta: crate::SessionMeta::default(),
            audio_file: None,
        };
        let sentences = vec![Sentence { id: 1, text: "一句话".into(), start_ms: 0, end_ms: 1_000 }];
        let snap = sample_record("x", 10, 1.0).snapshot;
        let record =
            build_session_record(&info, "free", None, &sentences, &snap, &VoiceMetrics::default(), "# r", "local", None, "full");
        assert!(!record.id.is_empty());
        assert!(valid_id(&record.id), "新 id 必须可作文件名：{}", record.id);
        // 麦克风默认来源 + topic None → 空串
        assert_eq!(record.source, "mic");
        assert_eq!(record.topic, "");
        assert_eq!(record.file_name, None);
        assert_eq!(record.report_mode, "full");
    }

    #[test]
    fn save_list_read_roundtrip() {
        let dir = test_dir();
        let record = sample_record("2026-08-30-100000", 300, 3.5);
        let id = save_record(&dir, &record).unwrap();
        assert_eq!(id, "2026-08-30-100000");

        let summaries = list_summaries(&dir);
        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.id, "2026-08-30-100000");
        assert_eq!(s.scenario, "free");
        assert_eq!(s.duration_ms, 60_000);
        assert_eq!(s.filler_per_minute, 3.5);
        assert_eq!(s.speech_rate, 210.0);
        assert_eq!(s.runaway_pause_count, 2);
        assert_eq!(s.longest_pause_ms, 3_100);

        let back = read_record(&dir, &id).unwrap();
        assert_eq!(back, record); // 全量 round-trip（含 snapshot.voice）
        assert_eq!(back.report, "# 报告\n\n内容");
        assert_eq!(back.ai_backend_used, "local");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summaries_sorted_newest_first_and_corrupt_files_skipped() {
        let dir = test_dir();
        save_record(&dir, &sample_record("2026-08-28-090000", 100, 5.0)).unwrap();
        save_record(&dir, &sample_record("2026-08-30-100000", 200, 3.0)).unwrap();
        save_record(&dir, &sample_record("2026-08-29-090000", 150, 4.0)).unwrap();
        std::fs::write(dir.join("corrupt.json"), "not json").unwrap();
        std::fs::write(dir.join("2026-08-27-empty.json"), "{}").unwrap(); // 缺字段 → 跳过
        let ids: Vec<String> = list_summaries(&dir).into_iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            vec![
                "2026-08-30-100000".to_string(),
                "2026-08-29-090000".to_string(),
                "2026-08-28-090000".to_string(),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn previous_session_picks_latest_excluding_current() {
        let dir = test_dir();
        assert!(previous_session(&dir, None).is_none()); // 空历史 → None
        save_record(&dir, &sample_record("2026-08-28-090000", 100, 5.0)).unwrap();
        save_record(&dir, &sample_record("2026-08-30-100000", 200, 3.0)).unwrap();
        // 无排除 → 最新
        let prev = previous_session(&dir, None).unwrap();
        assert_eq!(prev.filler_per_minute, 3.0);
        assert_eq!(prev.speech_rate, 210.0);
        assert_eq!(prev.avg_sentence_chars, 18.0);
        assert!(prev.date.starts_with("2026-08-30"));
        // 排除最新 → 上上一条
        let prev2 = previous_session(&dir, Some("2026-08-30-100000")).unwrap();
        assert_eq!(prev2.filler_per_minute, 5.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_file_and_missing_errors() {
        let dir = test_dir();
        let id = save_record(&dir, &sample_record("2026-08-30-100000", 10, 1.0)).unwrap();
        delete_record(&dir, &id).unwrap();
        assert!(read_record(&dir, &id).is_err());
        assert!(delete_record(&dir, &id).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_traversal_ids_rejected() {
        assert!(!valid_id(""));
        assert!(!valid_id("../evil"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id("a\\b"));
        assert!(!valid_id("a.b"));
        assert!(valid_id("2026-08-30-100000"));
        assert!(valid_id("backup_2"));
        let dir = test_dir();
        assert!(read_record(&dir, "..\\evil").is_err());
        assert!(read_record(&dir, "../evil").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwrite_same_id_keeps_single_record() {
        let dir = test_dir();
        save_record(&dir, &sample_record("2026-08-30-100000", 100, 5.0)).unwrap();
        let mut updated = sample_record("2026-08-30-100000", 100, 2.0);
        updated.report = "# 重试后的报告".into();
        save_record(&dir, &updated).unwrap();
        let summaries = list_summaries(&dir);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].filler_per_minute, 2.0);
        assert_eq!(read_record(&dir, "2026-08-30-100000").unwrap().report, "# 重试后的报告");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn previous_serializes_snake_case_for_prompt() {
        let r = sample_record("2026-08-30-100000", 100, 3.0);
        let p = PreviousSession::from_record(&r);
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["filler_per_minute"], 3.0);
        assert_eq!(v["speech_rate"], 210.0);
        assert_eq!(v["avg_sentence_chars"], 18.0);
        assert!(v["date"].as_str().unwrap().starts_with("2026-08-30"));
    }

    #[test]
    fn file_source_roundtrips_into_summary_and_detail() {
        let dir = test_dir();
        let record =
            sample_record_with_source("2026-08-30-110000", 300, 3.5, "file", Some("lei-jun-test.wav"));
        save_record(&dir, &record).unwrap();

        let summaries = list_summaries(&dir);
        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.source, "file");
        assert_eq!(s.file_name.as_deref(), Some("lei-jun-test.wav"));

        // 全量记录 round-trip：source / fileName 原样保留
        let back = read_record(&dir, "2026-08-30-110000").unwrap();
        assert_eq!(back.source, "file");
        assert_eq!(back.file_name.as_deref(), Some("lei-jun-test.wav"));

        // 麦克风记录的 fileName 序列化时省略
        let mic = sample_record("2026-08-30-120000", 10, 1.0);
        let v = serde_json::to_value(&mic).unwrap();
        assert!(v.get("fileName").is_none());
        save_record(&dir, &mic).unwrap();
        let s = list_summaries(&dir)
            .into_iter()
            .find(|s| s.id == "2026-08-30-120000")
            .unwrap();
        assert_eq!(s.source, "mic");
        assert_eq!(s.file_name, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_records_without_source_default_to_mic() {
        let dir = test_dir();
        // M5 之前的记录 JSON 没有 source / fileName 字段
        let legacy = r##"{
            "id": "2026-08-01-090000",
            "date": "2026-08-01 09:00:00",
            "scenario": "free",
            "topic": "",
            "snapshot": { "sentenceCount": 1, "fillerCounts": [], "fillerPerMinute": 0.0,
                          "emotionCounts": [], "hedgeCounts": [], "hedgeTotal": 0,
                          "durationMs": 60000, "totalChars": 10, "speechRate": 10.0,
                          "avgSentenceChars": 10.0 },
            "transcript": [],
            "report": "# 旧报告",
            "aiBackendUsed": "local"
        }"##;
        std::fs::write(dir.join("2026-08-01-090000.json"), legacy).unwrap();
        let back = read_record(&dir, "2026-08-01-090000").unwrap();
        assert_eq!(back.source, "mic");
        assert_eq!(back.file_name, None);
        // 旧记录同样没有 audioFile → None（无回放入口）
        assert_eq!(back.audio_file, None);
        let s = &list_summaries(&dir)[0];
        assert_eq!(s.source, "mic");
        assert_eq!(s.file_name, None);
        assert_eq!(s.audio_file, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audio_file_roundtrips_into_record_and_summary() {
        let dir = test_dir();
        let audio = dir.join("audio").join("2026-08-30-100000.wav");
        let record = sample_record_full(
            "2026-08-30-100000",
            300,
            3.5,
            "mic",
            None,
            Some(audio.to_str().unwrap()),
        );
        save_record(&dir, &record).unwrap();

        // 详情与列表都透传；无录音记录序列化时省略该字段
        let back = read_record(&dir, "2026-08-30-100000").unwrap();
        assert_eq!(back.audio_file.as_deref(), Some(audio.to_str().unwrap()));
        assert_eq!(list_summaries(&dir)[0].audio_file, back.audio_file);
        let v = serde_json::to_value(&sample_record("2026-08-30-130000", 10, 1.0)).unwrap();
        assert!(v.get("audioFile").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_audio_next_to_record_but_not_arbitrary_paths() {        let dir = test_dir();
        let audio_dir = dir.join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let wav = audio_dir.join("2026-08-30-100000.wav");
        std::fs::write(&wav, b"RIFF....").unwrap();
        let record = sample_record_full(
            "2026-08-30-100000",
            10,
            1.0,
            "mic",
            None,
            Some(wav.to_str().unwrap()),
        );
        save_record(&dir, &record).unwrap();

        delete_record_and_audio(&dir, "2026-08-30-100000").unwrap();
        assert!(read_record(&dir, "2026-08-30-100000").is_err());
        assert!(!wav.exists(), "会话录音应随记录一并删除");

        // 录音目录外的路径不删（防篡改记录变任意删除器）
        let outside = dir.join("elsewhere.wav");
        std::fs::write(&outside, b"keep me").unwrap();
        let evil = sample_record_full(
            "2026-08-30-110000",
            10,
            1.0,
            "mic",
            None,
            Some(outside.to_str().unwrap()),
        );
        save_record(&dir, &evil).unwrap();
        delete_record_and_audio(&dir, "2026-08-30-110000").unwrap();
        assert!(outside.exists(), "录音目录外的文件必须保留");
        assert!(read_record(&dir, "2026-08-30-110000").is_err());

        // 无 audioFile / 音频文件已不存在的记录：删除仍成功
        let plain = sample_record("2026-08-30-120000", 10, 1.0);
        save_record(&dir, &plain).unwrap();
        delete_record_and_audio(&dir, "2026-08-30-120000").unwrap();
        assert!(read_record(&dir, "2026-08-30-120000").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- 评分入档（B）-------------------------------------------------------

    #[test]
    fn scores_roundtrip_into_record_and_summary() {
        let dir = test_dir();
        let mut record = sample_record("2026-08-30-100000", 300, 3.5);
        record.scores = Some(serde_json::json!({ "overall": 78, "表达效率": 80, "结构": 70 }));
        save_record(&dir, &record).unwrap();

        // 全量记录与摘要行都透传；无评分记录序列化时省略该字段
        let back = read_record(&dir, "2026-08-30-100000").unwrap();
        assert_eq!(back.scores, record.scores);
        let s = &list_summaries(&dir)[0];
        assert_eq!(s.scores, record.scores);

        save_record(&dir, &sample_record("2026-08-30-110000", 10, 1.0)).unwrap();
        let v = serde_json::to_value(sample_record("2026-08-30-130000", 10, 1.0)).unwrap();
        assert!(v.get("scores").is_none());
        let plain = list_summaries(&dir)
            .into_iter()
            .find(|s| s.id == "2026-08-30-110000")
            .unwrap();
        assert_eq!(plain.scores, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_records_without_scores_default_to_none() {
        let dir = test_dir();
        // B 之前的记录 JSON 没有 scores 字段
        let legacy = r##"{
            "id": "2026-08-01-090000",
            "date": "2026-08-01 09:00:00",
            "scenario": "free",
            "topic": "",
            "snapshot": { "sentenceCount": 1, "fillerCounts": [], "fillerPerMinute": 0.0,
                          "emotionCounts": [], "hedgeCounts": [], "hedgeTotal": 0,
                          "durationMs": 60000, "totalChars": 10, "speechRate": 10.0,
                          "avgSentenceChars": 10.0 },
            "transcript": [],
            "report": "# 旧报告",
            "aiBackendUsed": "local"
        }"##;
        std::fs::write(dir.join("2026-08-01-090000.json"), legacy).unwrap();
        let back = read_record(&dir, "2026-08-01-090000").unwrap();
        assert_eq!(back.scores, None);
        assert_eq!(list_summaries(&dir)[0].scores, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- 报告模式（0.2.3：quick 快速报告）-----------------------------------

    #[test]
    fn report_mode_roundtrips_into_record() {
        let dir = test_dir();
        let mut record = sample_record("2026-08-30-100000", 300, 3.5);
        record.report_mode = "quick".into();
        save_record(&dir, &record).unwrap();
        // quick 模式原样保留（详情页据此显示「快速报告」标签）
        let back = read_record(&dir, "2026-08-30-100000").unwrap();
        assert_eq!(back.report_mode, "quick");
        // full 模式同样原样保留
        let mut full = sample_record("2026-08-30-110000", 10, 1.0);
        full.report_mode = "full".into();
        save_record(&dir, &full).unwrap();
        assert_eq!(read_record(&dir, "2026-08-30-110000").unwrap().report_mode, "full");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_records_without_report_mode_default_to_full() {
        let dir = test_dir();
        // 0.2.3 之前的记录 JSON 没有 reportMode 字段
        let legacy = r##"{
            "id": "2026-08-01-090000",
            "date": "2026-08-01 09:00:00",
            "scenario": "free",
            "topic": "",
            "snapshot": { "sentenceCount": 1, "fillerCounts": [], "fillerPerMinute": 0.0,
                          "emotionCounts": [], "hedgeCounts": [], "hedgeTotal": 0,
                          "durationMs": 60000, "totalChars": 10, "speechRate": 10.0,
                          "avgSentenceChars": 10.0 },
            "transcript": [],
            "report": "# 旧报告",
            "aiBackendUsed": "local"
        }"##;
        std::fs::write(dir.join("2026-08-01-090000.json"), legacy).unwrap();
        let back = read_record(&dir, "2026-08-01-090000").unwrap();
        assert_eq!(back.report_mode, "full");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- 孤儿音频清理（C2）--------------------------------------------------

    #[test]
    fn orphan_cleanup_three_states_referenced_over_age_and_fresh() {
        let dir = test_dir();
        let audio_dir = dir.join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();

        // 被历史引用的录音（无论多老都不删）
        let referenced_wav = audio_dir.join("2026-08-30-100000.wav");
        std::fs::write(&referenced_wav, b"RIFF....").unwrap();
        let record = sample_record_full(
            "2026-08-30-100000",
            10,
            1.0,
            "mic",
            None,
            Some(referenced_wav.to_str().unwrap()),
        );
        save_record(&dir, &record).unwrap();

        // 未引用的孤儿 wav 与一个非 wav 文件
        let orphan = audio_dir.join("orphan.wav");
        std::fs::write(&orphan, b"RIFF....").unwrap();
        let not_wav = audio_dir.join("orphan.txt");
        std::fs::write(&not_wav, b"junk").unwrap();

        let referenced = referenced_audio_files(&dir);
        assert!(referenced.contains(&referenced_wav.to_string_lossy().to_lowercase()));
        let now = SystemTime::now(); // 取"此刻"：文件刚写入，age >= 0 恒真（max_age=0 即超龄）

        // 超龄态：max_age = 0（刚写入的文件也视为超龄）→ 引用保留、孤儿删除、非 wav 不动
        let removed = cleanup_orphan_audio(&audio_dir, &referenced, Duration::ZERO, now);
        assert_eq!(removed, 1, "只删超龄未引用的 wav");
        assert!(referenced_wav.exists(), "被引用的录音不删");
        assert!(!orphan.exists(), "超龄孤儿应删除");
        assert!(not_wav.exists(), "非 wav 文件不动");

        // 未超龄态：max_age = 365 天（一切文件都算新鲜）→ 什么都不删
        std::fs::write(&orphan, b"RIFF....").unwrap();
        let removed_fresh = cleanup_orphan_audio(
            &audio_dir,
            &referenced,
            Duration::from_secs(365 * 24 * 3600),
            now,
        );
        assert_eq!(removed_fresh, 0, "未超龄孤儿保留");
        assert!(orphan.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn orphan_cleanup_missing_dir_is_silent_zero() {
        let dir = test_dir().join("no-such-audio");
        assert_eq!(
            cleanup_orphan_audio(&dir, &HashSet::new(), Duration::from_secs(1), SystemTime::now()),
            0
        );
    }
}
