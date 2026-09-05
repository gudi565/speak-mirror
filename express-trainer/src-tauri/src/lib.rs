pub mod audio;
pub mod checkin;
pub mod decode;
pub mod downloader;
pub mod export;
pub mod growth;
pub mod history;
pub mod interview;
pub mod interview_bank;
pub mod paniclog;
pub mod report;
pub mod rules;
pub mod secrets;
pub mod session;
pub mod settings;
pub mod tone;
pub mod voice;

use rules::engine::{EngineConfig, RuleEngine, SessionSnapshot};
use rules::Sentence;
use secrets::SecureStore;
use serde_json::json;
use settings::Settings;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tauri::{AppHandle, Emitter, Manager, State};

/// 当前会话来源（历史落盘用）："mic"（实时麦克风）/ "file"（从文件练习）
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub source: String,
    pub file_name: Option<String>,
}

impl Default for SessionMeta {
    fn default() -> Self {
        Self { source: "mic".into(), file_name: None }
    }
}

/// 停止信号与会话代 id 绑定：会话线程退出时只清理属于自己的代，
/// 防止「旧会话停止收尾期间新会话已启动」时旧线程误清新会话的
/// 停止信号（否则新会话永远无法停止，stop_session join 挂死）。
#[derive(Clone)]
pub struct StopSignal {
    pub generation: u64,
    pub tx: Sender<()>,
}

/// 会话线程退出时的清理（锁内判定 + 只清自己的代）：
/// 返回「stop 槽是否仍属本会话」——调用方据此决定是否广播本会话的
/// session_error（旧会话迟到的错误不该打断新会话的界面状态）。
fn session_exit_cleanup(stop_arc: &Mutex<Option<StopSignal>>, generation: u64) -> bool {
    let mut guard = stop_arc.lock().unwrap();
    let ours = guard.as_ref().map(|s| s.generation) == Some(generation);
    if ours {
        *guard = None;
    }
    ours
}

pub struct AppState {
    engine: Arc<Mutex<RuleEngine>>,
    stop: Arc<Mutex<Option<StopSignal>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
    /// 会话代计数器（每次 launch_session 递增）：标记 stop 槽当前属主，
    /// 供会话线程退出时判定「槽里还是不是自己的停止信号」；
    /// 声调分析线程也用它判定结果是否仍属当前会话（Arc 供后台线程持有）
    session_generation: Arc<AtomicU64>,
    /// AI 周期快评的会话级状态（计划时长 + 同类冷却时间戳）
    pub checkin: Mutex<checkin::CheckinRuntime>,
    /// 声音层分析器：会话线程独占写入（搭便车），stop/报告时读取
    pub voice: Arc<Mutex<voice::VoiceAnalyzer>>,
    /// 本次会话已落盘的历史记录 id（报告重试时覆盖同一文件；新会话重置）
    pub current_history_id: Mutex<Option<String>>,
    /// 本次会话来源（mic/file + 文件名），历史落盘时读取
    pub session_meta: Mutex<SessionMeta>,
    /// 本次会话录音的落盘路径（会话线程结束时写入；关闭录音/截断/写失败为 None）
    pub last_audio: Arc<Mutex<Option<String>>>,
    /// 声调偏差标记（v0）：会话停止后由后台分析线程写入，
    /// get_snapshot/transcript（报告与历史落盘）时并入快照
    pub tone_flags: Arc<Mutex<Vec<tone::ToneFlag>>>,
    /// 应用内模型下载进行中标志（首启引导；并发保护）
    pub downloading: AtomicBool,
}

impl AppState {
    /// 取全部终稿句与统计快照（返回克隆，锁立即释放，供异步报告命令使用）。
    /// toneFlags 并入当前已完成的声调分析结果（分析未完成时为空数组）
    pub fn transcript(&self) -> (Vec<Sentence>, SessionSnapshot) {
        let eng = self.engine.lock().unwrap();
        let mut snap = eng.snapshot();
        snap.tone_flags = self
            .tone_flags
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        (eng.sentences().to_vec(), snap)
    }
}

/// 由设置构建规则引擎配置：规则开关、自定义口头禅、口头禅高频阈值
pub fn engine_config_from_settings(s: &Settings) -> EngineConfig {
    EngineConfig {
        enabled: rules::engine::ALL_RULES
            .iter()
            .filter(|name| s.rule_enabled(name))
            .map(|name| name.to_string())
            .collect::<HashSet<String>>(),
        custom_fillers: settings::parse_custom_fillers(&s.custom_fillers),
        filler_medium_threshold_per_min: s.filler_high_threshold,
    }
}

/// 文件练习倍速白名单（UI 与后端一致）：三档实时 + 极速（不限速）
pub const FILE_SPEEDS: &[f64] = &[1.0, 1.5, 2.0, FILE_SPEED_MAX];

/// 极速档哨兵值：JSON 无法序列化 f64::INFINITY，以 99.0 表示「不限速」——
/// 喂入线程不等时刻表、连续 send，消费端（解码/VAD/识别）跑多快算多快。
/// 与前端 src/types.ts 的 FILE_SPEED_MAX 保持一致。
pub const FILE_SPEED_MAX: f64 = 99.0;

/// 是否极速档（不限速）：极速下 partial 完全跳过（跑得比实时快，字幕滚动
/// 无意义），时间戳/统计仍按内容时钟（样本位置），统计正确性不变
pub fn is_max_speed(speed: f64) -> bool {
    speed == FILE_SPEED_MAX
}

/// 倍速参数校验（纯函数可测）：None = 1.0；仅接受白名单值
pub fn validate_speed(speed: Option<f64>) -> Result<f64, String> {
    match speed {
        None => Ok(1.0),
        Some(s) if FILE_SPEEDS.contains(&s) => Ok(s),
        Some(s) => Err(format!(
            "倍速仅支持 1.0 / 1.5 / 2.0 / 极速（收到 {s}）"
        )),
    }
}

/// 由设置构建会话热词：写临时文件（一行一词），None = 未配置/写失败（静默降级）
pub fn build_hotwords_from_settings(s: &Settings) -> Option<session::AsrHotwords> {
    let words = settings::parse_hotwords(&s.hotwords);
    if words.is_empty() {
        return None;
    }
    let path = std::env::temp_dir().join(format!("speakmirror-hotwords-{}.txt", std::process::id()));
    let body = words.join("\n");
    match std::fs::write(&path, body) {
        Ok(()) => Some(session::AsrHotwords {
            file: path,
            score: s.hotwords_score.unwrap_or(settings::DEFAULT_HOTWORDS_SCORE) as f32,
        }),
        Err(e) => {
            eprintln!("hotwords temp file write failed: {e}");
            None
        }
    }
}

/// 会话启动公共路径（麦克风 / 文件共用）：互斥校验 → 模型目录 → 按设置重建引擎
/// 与快评/声音/落盘状态 → 派生会话线程。音频源由 `source` 决定，之后管线完全一致。
fn launch_session(
    app: &AppHandle,
    state: &AppState,
    planned_sec: Option<u64>,
    source: session::AudioSource,
    meta: SessionMeta,
) -> Result<(), String> {
    let mut stop_guard = state.stop.lock().unwrap();
    if stop_guard.is_some() {
        return Err("会话已在进行中".into());
    }

    // 模型目录解析统一走 downloader（首启引导下载到 appdata；开发模式回退仓库 models）
    let models_dir: PathBuf = match downloader::find_models_dir(app) {
        Some(dir) => dir,
        None => {
            return Err(
                "识别模型未就绪：请完成首启引导中的模型下载，或重新启动应用后再试".into(),
            )
        }
    };

    // 按当前设置构建规则集（规则开关 / 自定义口头禅 / 阈值），并重置快评运行时
    // 与声音层分析器、历史落盘标记。仅在所有校验通过之后、派生线程之前执行。
    // 词库自生长：会话开始前合并用户词库（user-lexicon.json），规则与统计即时生效。
    growth::apply_user_lexicon_from_disk(app);
    let s = settings::load(app).normalized();
    let options = session::SessionOptions {
        record_audio: s.record_audio,
        hotwords: build_hotwords_from_settings(&s),
        corrections: settings::parse_correction_map(&s.asr_corrections),
        precision_finals: s.precision_finals,
        vad_sensitivity: s.vad_sensitivity.clone(),
        enhance_audio: s.enhance_audio,
    };
    *state.engine.lock().unwrap() = RuleEngine::from_config(engine_config_from_settings(&s));
    *state.checkin.lock().unwrap() = checkin::CheckinRuntime {
        planned_sec: planned_sec.filter(|p| *p > 0),
        last_fired: Default::default(),
        fails: checkin::FailTracker::new(),
    };
    *state.voice.lock().unwrap() = voice::VoiceAnalyzer::new(16_000);
    *state.current_history_id.lock().unwrap() = None;
    *state.session_meta.lock().unwrap() = meta;
    *state.last_audio.lock().unwrap() = None;
    // 上一会话的声调分析结果清空（新会话从「分析中」重新开始）
    *state.tone_flags.lock().unwrap_or_else(|p| p.into_inner()) = Vec::new();

    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let generation = state.session_generation.fetch_add(1, Ordering::SeqCst) + 1;
    *stop_guard = Some(StopSignal { generation, tx });
    let engine = Arc::clone(&state.engine);
    let voice = Arc::clone(&state.voice);
    let stop_arc = Arc::clone(&state.stop);
    let last_audio = Arc::clone(&state.last_audio);
    let app_clone = app.clone();
    let handle = std::thread::spawn(move || {
        let result =
            session::run_session(app_clone.clone(), rx, source, engine, voice, models_dir, options);
        // 线程退出前（无论是正常结束还是出错）必须释放 stop 标志，否则无法重新启动会话。
        // 只清理属于自己的代：若停止收尾期间新会话已启动（stop 槽已被新代占用），
        // 绝不能清掉新会话的停止信号——那会让新会话永远无法停止（stop_session join 挂死）。
        let still_current = session_exit_cleanup(&stop_arc, generation);
        // 录音落盘路径带回 AppState（前端 get_last_audio / 历史落盘读取）。
        // 无条件写入：本会话的 stop_session 正在 join 等待，join 返回前必须能看到本值。
        match result {
            Ok(audio_path) => {
                *last_audio.lock().unwrap() = audio_path;
            }
            Err(e) => {
                // 只有 stop 槽仍属本会话才广播错误：旧会话迟到的失败
                // 不该作为 session_error 打断已开始的新会话
                if still_current {
                    let _ = app_clone.emit("session_error", e);
                }
            }
        }
    });
    *state.handle.lock().unwrap() = Some(handle);
    Ok(())
}

#[tauri::command]
fn start_session(
    app: AppHandle,
    state: State<AppState>,
    planned_sec: Option<u64>,
) -> Result<(), String> {
    launch_session(&app, &state, planned_sec, session::AudioSource::Mic, SessionMeta::default())
}

/// 只探测音频文件时长（选择文件后展示用；不解码全部样本）
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioFileMeta {
    duration_ms: u64,
}

#[tauri::command]
fn probe_audio_file(path: String) -> Result<AudioFileMeta, String> {
    Ok(AudioFileMeta { duration_ms: decode::probe_duration_ms(std::path::Path::new(&path))? })
}

/// 从本地音频文件开始练习：先完整解码校验（失败直接报错，不启动任何线程），
/// 然后按选定倍速（1.0/1.5/2.0/极速 99，默认 1.0）喂入与麦克风完全相同的
/// 会话管线。时间戳与统计走内容时钟（素材内位置），倍速只影响喂入的墙钟节奏
/// （极速 = 不限速：跳过 partial 字幕，只出终稿句）。录音增强（自动增益）
/// 开启时在会话线程内对整段 16k 样本先做增益。文件播完由后端发 `session_finished`。
#[tauri::command]
fn start_session_from_file(
    app: AppHandle,
    state: State<AppState>,
    path: String,
    planned_sec: Option<u64>,
    speed: Option<f64>,
) -> Result<serde_json::Value, String> {
    let speed = validate_speed(speed)?;
    let p = std::path::Path::new(&path);
    let decoded = decode::decode_audio_file(p)?;
    let duration_ms = decoded.duration_ms();
    let file_name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    launch_session(
        &app,
        &state,
        planned_sec,
        session::AudioSource::File {
            samples: decoded.samples,
            sample_rate: decoded.sample_rate,
            channels: decoded.channels,
            speed,
        },
        SessionMeta { source: "file".into(), file_name: Some(file_name.clone()) },
    )?;
    Ok(json!({ "fileName": file_name, "durationMs": duration_ms, "speed": speed }))
}

#[tauri::command]
fn stop_session(app: AppHandle, state: State<AppState>) -> SessionSnapshot {
    // 先取出 stop 发送端并立刻释放锁：若跨 join 持有 stop 锁，会话线程退出前
    // 清理标志时也要锁它，会互相等待（死锁）。会话已自然结束（文件播完）时
    // 标志已被会话线程清空，这里直接跳过，照常返回快照。
    let sig = state.stop.lock().unwrap().take();
    if let Some(sig) = sig {
        let _ = sig.tx.send(());
    }
    // Take the handle out and drop the guard before joining so the join
    // does not hold the handle mutex for the worker's lifetime.
    let handle = state.handle.lock().unwrap().take();
    if let Some(handle) = handle {
        let _ = handle.join();
    }
    // 会话线程已 join：并入声音层终值
    let mut snap = state.engine.lock().unwrap().snapshot();
    snap.voice = Some(state.voice.lock().unwrap().metrics());
    // 声调偏差检查（v0）：停止后的后处理路径——不碰实时链路。
    // 开关开 + 有录音 wav + 有终稿句时，后台线程离线分析并 emit tone_update；
    // 此处并入的是「此刻已就绪」的结果（通常为空：分析在后台刚起步）
    snap.tone_flags = spawn_tone_analysis(&app, &state);
    snap
}

#[tauri::command]
fn get_snapshot(state: State<AppState>) -> SessionSnapshot {
    let mut snap = state.engine.lock().unwrap().snapshot();
    snap.voice = Some(state.voice.lock().unwrap().metrics());
    snap.tone_flags = state
        .tone_flags
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    snap
}

// ---------------------------------------------------------------------------
// 声调偏差检查（v1）：会话停止后的异步后处理，不碰实时链路
// ---------------------------------------------------------------------------

/// 停止后触发声调分析（若条件满足）：读录音 wav → 逐句（startMs/endMs 切片）
/// 跑 tone::check_sentence（含变调规则豁免与句内音域归一）→ 结果写入
/// AppState.tone_flags 并 emit `tone_update`
/// `{ flags }`（空数组 = 已分析无发现；不发射 = 未开启/无录音）。
/// 返回值恒为空（分析在后台异步完成，stop_session 的快照不带新结果）；
/// 世代校验：分析完成时新会话已启动则丢弃结果，不打扰新会话的界面。
fn spawn_tone_analysis(app: &AppHandle, state: &AppState) -> Vec<tone::ToneFlag> {
    let settings = settings::load(app).normalized();
    let audio = state
        .last_audio
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    if !settings.tone_check || audio.is_none() {
        return Vec::new();
    }
    let (sentences, _) = state.transcript();
    if sentences.is_empty() {
        // 有录音但没有任何终稿句：无可分析文本。仍发一次空结果事件，
        // 前端「分析中…」空态才能落定（不会永远转下去）
        *state.tone_flags.lock().unwrap_or_else(|p| p.into_inner()) = Vec::new();
        let _ = app.emit("tone_update", json!({ "flags": [] }));
        return Vec::new();
    }
    let generation = state.session_generation.load(Ordering::SeqCst);
    let flags_store = Arc::clone(&state.tone_flags);
    let generation_counter = Arc::clone(&state.session_generation);
    let wav = audio.unwrap();
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("tone-analysis".into())
        .spawn(move || {
            // 解码 + 重采样到 16k 单声道（会话录音本就是 16k 单声道，直通）
            let analysis = |sentences: Vec<Sentence>| -> Vec<tone::ToneFlag> {
                let decoded = match decode::decode_audio_file(std::path::Path::new(&wav)) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("声调分析：读取录音失败（{e}），本次跳过");
                        return Vec::new();
                    }
                };
                let samples = audio::resample_to_16k_mono(
                    &decoded.samples,
                    decoded.sample_rate,
                    decoded.channels,
                );
                tone::analyze_session(&sentences, &samples, 16_000)
            };
            // 分析线程 panic 不许带崩整个应用（与主流程隔离）
            let flags = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                analysis(sentences)
            }))
            .unwrap_or_else(|_| {
                eprintln!("声调分析线程异常退出，本次跳过");
                Vec::new()
            });
            // 世代校验：分析期间新会话已启动 → 结果作废（不写不发）
            if generation_counter.load(Ordering::SeqCst) != generation {
                return;
            }
            *flags_store.lock().unwrap_or_else(|p| p.into_inner()) = flags.clone();
            let _ = app.emit("tone_update", json!({ "flags": flags }));
        });
    if let Err(e) = spawned {
        eprintln!("声调分析线程启动失败（{e}），本次跳过");
    }
    Vec::new()
}

/// 字幕红色标注用的口头禅词表（词库分级 + 用户词库 + 自定义）。
/// 高频与自定义词优先展示；中频词误报率高，仅在右栏统计里出现。
#[tauri::command]
fn get_filler_words(app: AppHandle) -> serde_json::Value {
    // 先应用用户词库（user-lexicon.json 里的自定义 filler 并入 high 档）
    growth::apply_user_lexicon_from_disk(&app);
    let s = settings::load(&app).normalized();
    let lex = rules::lexicon::lexicon();
    json!({
        "high": lex.fillers.high,
        "medium": lex.fillers.medium,
        "custom": settings::parse_custom_fillers(&s.custom_fillers),
    })
}

/// 本次会话录音的落盘路径（无录音 = null；会话进行中不提供回放，结束后才查）
#[tauri::command]
fn get_last_audio(state: State<AppState>) -> Option<String> {
    state.last_audio.lock().unwrap().clone()
}

// ---------------------------------------------------------------------------
// API Key 安全存储（系统凭据管理器；见 secrets.rs）
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SetApiKeyResult {
    /// true = 已安全写入系统凭据（store 里不再留明文）
    secure: bool,
    /// secure=false 时的中文提示（降级为明文保存）
    message: Option<String>,
}

/// 设置保存时由前端调用：把 API Key 写进系统凭据管理器。
/// 空串 = 清除。写失败返回 secure:false + 提示，前端降级为明文保存并展示提示。
#[tauri::command]
fn set_api_key(key: String) -> SetApiKeyResult {
    let store = secrets::SystemKeyring::ai_api_key();
    let key = key.trim();
    if key.is_empty() {
        return match store.delete() {
            Ok(()) => SetApiKeyResult { secure: true, message: None },
            Err(e) => SetApiKeyResult {
                secure: false,
                message: Some(format!("{e}；已保留原有明文存储")),
            },
        };
    }
    match store.set(key) {
        Ok(()) => SetApiKeyResult { secure: true, message: None },
        Err(e) => SetApiKeyResult {
            secure: false,
            message: Some(format!("{e}；API Key 将以明文保存在本机设置文件中")),
        },
    }
}

/// 前端启动时取生效 Key（凭据管理器优先，自动迁移旧明文；无 Key = null）
#[tauri::command]
fn get_api_key(app: AppHandle) -> Option<String> {
    let key = settings::load(&app).api_key;
    if key.trim().is_empty() {
        None
    } else {
        Some(key)
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            // panic 日志（技术债 C1）：启动最早期装 hook，写入 appdata/logs/app.log
            if let Ok(appdata) = app.path().app_data_dir() {
                paniclog::install(appdata);
            }
            // 孤儿音频清理（技术债 C2）：删掉不被任何历史引用且超 7 天的
            // sessions/audio/*.wav（模拟面试逐题会话的录音靠这里回收）；失败静默
            history::cleanup_orphan_audio_on_startup(&history::sessions_dir(app.handle()));
            // 启动即合并用户词库（词库自生长），全局生效
            growth::apply_user_lexicon_from_disk(app.handle());
            Ok(())
        })
        .manage(AppState {
            engine: Arc::new(Mutex::new(RuleEngine::new())),
            stop: Arc::new(Mutex::new(None)),
            handle: Mutex::new(None),
            session_generation: Arc::new(AtomicU64::new(0)),
            checkin: Mutex::new(checkin::CheckinRuntime::default()),
            voice: Arc::new(Mutex::new(voice::VoiceAnalyzer::new(16_000))),
            current_history_id: Mutex::new(None),
            session_meta: Mutex::new(SessionMeta::default()),
            last_audio: Arc::new(Mutex::new(None)),
            tone_flags: Arc::new(Mutex::new(Vec::new())),
            downloading: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            start_session,
            start_session_from_file,
            probe_audio_file,
            stop_session,
            get_snapshot,
            get_filler_words,
            get_last_audio,
            get_api_key,
            set_api_key,
            downloader::check_models,
            downloader::download_models,
            checkin::check_in,
            checkin::reset_checkin,
            interview::generate_interview_questions,
            report::get_transcript,
            report::generate_report,
            report::test_connection,
            export::save_text_file,
            export::export_obsidian,
            history::list_sessions,
            history::get_session,
            history::delete_session,
            growth::list_lexicon_candidates,
            growth::add_lexicon_entry,
            growth::dismiss_lexicon_candidate,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 复现「停止与新启动交叠」的时序：stop_session 取走旧信号后 join 等
    /// 旧线程收尾，期间 launch_session 已注册新会话的停止信号——旧线程
    /// 退出时绝不能清掉新会话的信号（否则新会话永远无法停止）。
    #[test]
    fn session_exit_never_clears_newer_generation_stop_signal() {
        let (tx2, _rx2) = std::sync::mpsc::channel::<()>();
        let stop = Mutex::new(Some(StopSignal { generation: 2, tx: tx2 }));

        // 旧会话（gen 1）线程退出：槽已属新会话 → 不清理，报告「非当前」
        assert!(!session_exit_cleanup(&stop, 1));
        assert_eq!(
            stop.lock().unwrap().as_ref().map(|s| s.generation),
            Some(2),
            "旧线程退出不得清掉新会话的停止信号"
        );

        // 新会话（gen 2）自然结束：清空自己的槽，报告「当前」
        assert!(session_exit_cleanup(&stop, 2));
        assert!(stop.lock().unwrap().is_none());

        // 槽已空（stop_session 先 take 走了）：任何退出都是「非当前」
        assert!(!session_exit_cleanup(&stop, 2));
    }

    /// 世代递增：launch 两次得到不同代（保证交叠判定有区分度）
    #[test]
    fn session_generation_increments_per_launch() {
        let state = AppState {
            engine: Arc::new(Mutex::new(RuleEngine::new())),
            stop: Arc::new(Mutex::new(None)),
            handle: Mutex::new(None),
            session_generation: Arc::new(AtomicU64::new(0)),
            checkin: Mutex::new(checkin::CheckinRuntime::default()),
            voice: Arc::new(Mutex::new(voice::VoiceAnalyzer::new(16_000))),
            current_history_id: Mutex::new(None),
            session_meta: Mutex::new(SessionMeta::default()),
            last_audio: Arc::new(Mutex::new(None)),
            tone_flags: Arc::new(Mutex::new(Vec::new())),
            downloading: AtomicBool::new(false),
        };
        let g1 = state.session_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let g2 = state.session_generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert_ne!(g1, g2);
    }

    /// transcript 并入已完成的声调分析结果（报告与历史落盘走这条路；
    /// 分析未完成时为空数组，不影响旧流程）
    #[test]
    fn transcript_merges_tone_flags_into_snapshot() {
        let state = AppState {
            engine: Arc::new(Mutex::new(RuleEngine::new())),
            stop: Arc::new(Mutex::new(None)),
            handle: Mutex::new(None),
            session_generation: Arc::new(AtomicU64::new(0)),
            checkin: Mutex::new(checkin::CheckinRuntime::default()),
            voice: Arc::new(Mutex::new(voice::VoiceAnalyzer::new(16_000))),
            current_history_id: Mutex::new(None),
            session_meta: Mutex::new(SessionMeta::default()),
            last_audio: Arc::new(Mutex::new(None)),
            tone_flags: Arc::new(Mutex::new(vec![tone::ToneFlag {
                sentence_id: 2,
                char_index: 1,
                char: "妈".into(),
                expected_tone: 1,
                detected_shape: 4,
                note: None,
            }])),
            downloading: AtomicBool::new(false),
        };
        let (sentences, snap) = state.transcript();
        assert!(sentences.is_empty()); // 无句会话也能并快照
        assert_eq!(snap.tone_flags.len(), 1);
        assert_eq!(snap.tone_flags[0].sentence_id, 2);
        // 序列化 camelCase（tone_update / 历史落盘同口径）
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(v["toneFlags"][0]["sentenceId"], 2);
        assert_eq!(v["toneFlags"][0]["detectedShape"], 4);
    }

    /// generate_report 入口捕获的落盘快照必须是值拷贝：捕获后 AppState
    /// 被新会话重置（launch_session 会清空这三个字段）不影响已捕获值，
    /// 否则旧报告会错记到新会话名下（来源/录音错配、重复建档）。
    #[test]
    fn captured_persist_info_is_immutable_to_later_state_resets() {
        let state = AppState {
            engine: Arc::new(Mutex::new(RuleEngine::new())),
            stop: Arc::new(Mutex::new(None)),
            handle: Mutex::new(None),
            session_generation: Arc::new(AtomicU64::new(0)),
            checkin: Mutex::new(checkin::CheckinRuntime::default()),
            voice: Arc::new(Mutex::new(voice::VoiceAnalyzer::new(16_000))),
            current_history_id: Mutex::new(Some("2026-08-31-100000".into())),
            session_meta: Mutex::new(SessionMeta {
                source: "file".into(),
                file_name: Some("talk.wav".into()),
            }),
            last_audio: Arc::new(Mutex::new(Some("C:/rec.wav".into()))),
            tone_flags: Arc::new(Mutex::new(Vec::new())),
            downloading: AtomicBool::new(false),
        };
        let info = crate::history::capture_session_persist_info(&state);
        assert_eq!(info.existing_id.as_deref(), Some("2026-08-31-100000"));
        assert_eq!(info.meta.source, "file");
        assert_eq!(info.audio_file.as_deref(), Some("C:/rec.wav"));

        // 模拟报告流式生成期间新会话启动（launch_session 的重置动作）
        *state.current_history_id.lock().unwrap() = None;
        *state.session_meta.lock().unwrap() = SessionMeta::default();
        *state.last_audio.lock().unwrap() = None;

        // 捕获值不受影响
        assert_eq!(info.existing_id.as_deref(), Some("2026-08-31-100000"));
        assert_eq!(info.meta.source, "file");
        assert_eq!(info.meta.file_name.as_deref(), Some("talk.wav"));
        assert_eq!(info.audio_file.as_deref(), Some("C:/rec.wav"));
    }

    #[test]
    fn engine_config_respects_switches_and_custom_fillers() {
        let mut s = Settings::default();
        s.rule_enabled.insert("hedge".into(), false);
        s.rule_enabled.insert("time_vague".into(), false);
        s.custom_fillers = "老铁，绝绝子".into();
        s.filler_high_threshold = 5.0;
        let cfg = engine_config_from_settings(&s);
        assert!(!cfg.is_enabled("hedge"));
        assert!(!cfg.is_enabled("time_vague"));
        assert!(cfg.is_enabled("filler_words"));
        assert!(cfg.is_enabled("imagery"));
        assert_eq!(cfg.custom_fillers, vec!["老铁".to_string(), "绝绝子".to_string()]);
        assert_eq!(cfg.filler_medium_threshold_per_min, 5.0);
    }

    #[test]
    fn validate_speed_accepts_whitelist_and_defaults_to_1x() {
        assert_eq!(validate_speed(None).unwrap(), 1.0);
        assert_eq!(validate_speed(Some(1.0)).unwrap(), 1.0);
        assert_eq!(validate_speed(Some(1.5)).unwrap(), 1.5);
        assert_eq!(validate_speed(Some(2.0)).unwrap(), 2.0);
        // 极速档哨兵：仅 99.0 精确命中（98/100 都不行）
        assert_eq!(validate_speed(Some(FILE_SPEED_MAX)).unwrap(), FILE_SPEED_MAX);
        assert!(validate_speed(Some(98.0)).is_err());
        assert!(validate_speed(Some(100.0)).is_err());
        // 白名单外的值：中文报错
        for bad in [Some(1.2), Some(0.5), Some(3.0), Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
            let err = validate_speed(bad).unwrap_err();
            assert!(err.contains("倍速仅支持 1.0 / 1.5 / 2.0 / 极速"), "{err}");
        }
    }

    #[test]
    fn is_max_speed_matches_sentinel_only() {
        assert!(is_max_speed(FILE_SPEED_MAX));
        assert!(is_max_speed(99.0));
        // 实时三档与其它值都不是极速
        for s in [1.0, 1.5, 2.0, 0.0, -99.0, f64::INFINITY] {
            assert!(!is_max_speed(s), "{s}");
        }
        // 白名单 = 三档实时 + 极速，与前端 SPEED_OPTIONS 对应
        assert_eq!(FILE_SPEEDS, &[1.0, 1.5, 2.0, FILE_SPEED_MAX]);
    }

    #[test]
    fn hotwords_temp_file_written_one_word_per_line() {
        let mut s = Settings::default();
        s.hotwords = "DeepSeek, 米糕\n米糕".into();
        s.hotwords_score = None; // 默认权重
        let hw = build_hotwords_from_settings(&s).unwrap();
        assert_eq!(hw.score as f64, settings::DEFAULT_HOTWORDS_SCORE);
        let body = std::fs::read_to_string(&hw.file).unwrap();
        assert_eq!(body, "DeepSeek\n米糕");
        let _ = std::fs::remove_file(&hw.file);

        // 自定义权重
        let s2 = Settings { hotwords: "大模型".into(), hotwords_score: Some(2.5), ..Default::default() };
        let hw2 = build_hotwords_from_settings(&s2).unwrap();
        assert_eq!(hw2.score, 2.5);
        let _ = std::fs::remove_file(&hw2.file);

        // 未配置 → None
        assert!(build_hotwords_from_settings(&Settings::default()).is_none());
        assert!(build_hotwords_from_settings(&Settings {
            hotwords: "  ，, ".into(),
            ..Default::default()
        })
        .is_none());
    }
}
