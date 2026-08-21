pub mod audio;
pub mod rules;
pub mod session;

use rules::engine::{RuleEngine, SessionSnapshot};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tauri::{AppHandle, Emitter, Manager, State};

pub struct AppState {
    engine: Arc<Mutex<RuleEngine>>,
    stop: Arc<Mutex<Option<Sender<()>>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

#[tauri::command]
fn start_session(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let mut stop_guard = state.stop.lock().unwrap();
    if stop_guard.is_some() {
        return Err("会话已在进行中".into());
    }

    let models_dir: PathBuf = app
        .path()
        .resource_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("..")
        .join("models");
    // 开发模式下 resource_dir 指向 target/debug，models 在项目根：
    // 找不到时回退到 crate 根的上级
    let models_dir = if models_dir.join("silero_vad.onnx").exists() {
        models_dir
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("models")
    };
    if !models_dir.join("silero_vad.onnx").exists() {
        return Err("模型文件缺失，请先运行 scripts/download-models.ps1".into());
    }

    // 重置引擎：仅在所有校验通过之后、派生线程之前执行
    *state.engine.lock().unwrap() = RuleEngine::new();

    let (tx, rx) = std::sync::mpsc::channel::<()>();
    *stop_guard = Some(tx);
    let engine = Arc::clone(&state.engine);
    let stop_arc = Arc::clone(&state.stop);
    let app_clone = app.clone();
    let handle = std::thread::spawn(move || {
        let result = session::run_session(app_clone.clone(), rx, engine, models_dir);
        // 线程退出前（无论是正常结束还是出错）必须释放 stop 标志，否则无法重新启动会话
        *stop_arc.lock().unwrap() = None;
        if let Err(e) = result {
            let _ = app_clone.emit("session_error", e);
        }
    });
    *state.handle.lock().unwrap() = Some(handle);
    Ok(())
}

#[tauri::command]
fn stop_session(state: State<AppState>) -> SessionSnapshot {
    if let Some(tx) = state.stop.lock().unwrap().take() {
        let _ = tx.send(());
        // Take the handle out and drop the guard before joining so the join
        // does not hold the handle mutex for the worker's lifetime.
        let handle = state.handle.lock().unwrap().take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
    state.engine.lock().unwrap().snapshot()
}

#[tauri::command]
fn get_snapshot(state: State<AppState>) -> SessionSnapshot {
    state.engine.lock().unwrap().snapshot()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            engine: Arc::new(Mutex::new(RuleEngine::new())),
            stop: Arc::new(Mutex::new(None)),
            handle: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![start_session, stop_session, get_snapshot])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
