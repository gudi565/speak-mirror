//! panic 日志（技术债 C1）：把 panic 信息与时间追加写 `appdata/logs/app.log`。
//!
//! - 超过 1MB 轮转为 `app.log.1`（最多保留一份：先删旧 .1 再改名）；
//! - 不影响默认崩溃行为：记录后仍调用先前的默认 hook（打印 stderr + 退出）；
//! - 任何写失败静默（日志本身绝不能引起新的崩溃）。
//!
//! install() 在应用启动早期（run() 的 setup）调用，目录由 AppHandle 解析；
//! 纯 IO 逻辑（追加 + 轮转）以目录为参数，可单测。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// 单文件上限（字节）：超过则轮转
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;
pub const LOG_FILE: &str = "app.log";
pub const ROTATED_FILE: &str = "app.log.1";

/// hook 里解析出的日志目录（install 时写入一次）
static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 安装 panic hook：记录到 appdata/logs/app.log 后转交默认 hook。
/// 幂等：重复调用以后一次为准（先前的 hook 链不会被丢掉——本函数只装一次，run() setup 调用）。
pub fn install(appdata: PathBuf) {
    let dir = appdata.join("logs");
    let prev = Arc::new(std::panic::take_hook());
    let hook_prev = Arc::clone(&prev);
    std::panic::set_hook(Box::new(move |info| {
        // 先落盘（失败静默），再保持默认崩溃行为
        if let Some(dir) = LOG_DIR.get() {
            let line = format_panic_line(info);
            let _ = append_panic_line(dir, &line);
        }
        hook_prev(info);
    }));
    let _ = LOG_DIR.set(dir);
}

/// 组装一行 panic 记录（本地时间 + 位置 + 消息）
fn format_panic_line(info: &std::panic::PanicHookInfo<'_>) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "未知位置".into());
    let payload = payload_text(info.payload());
    compose_line(&now, &location, &payload)
}

/// panic payload 转文本（&str / String 之外的类型给占位说明）
fn payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "非文本 panic payload".into()
    }
}

/// 纯拼装（可单测）：[时间] panic at 位置: 消息
pub fn compose_line(now: &str, location: &str, payload: &str) -> String {
    format!("[{now}] panic at {location}: {payload}\n")
}

/// 追加一行日志（超过上限先轮转）；目录不存在则创建
pub fn append_panic_line(dir: &Path, line: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建日志目录失败：{e}"))?;
    let path = dir.join(LOG_FILE);
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() >= MAX_LOG_BYTES {
            rotate(dir);
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开日志文件失败：{e}"))?;
    f.write_all(line.as_bytes()).map_err(|e| format!("写入日志失败：{e}"))
}

/// 轮转：app.log → app.log.1（旧的 .1 先删，最多保留一份；失败静默）
fn rotate(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(ROTATED_FILE));
    let _ = std::fs::rename(dir.join(LOG_FILE), dir.join(ROTATED_FILE));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("sm-paniclog-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn append_creates_dir_and_file_and_accumulates() {
        let dir = test_dir().join("logs");
        append_panic_line(&dir, "第一行\n").unwrap();
        append_panic_line(&dir, "第二行\n").unwrap();
        let body = std::fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        assert_eq!(body, "第一行\n第二行\n");
        assert!(!dir.join(ROTATED_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotates_when_exceeding_cap_and_keeps_only_one_backup() {
        let dir = test_dir();
        // 预置一个超上限的 app.log（1MB 零字节）
        std::fs::write(dir.join(LOG_FILE), vec![b'x'; MAX_LOG_BYTES as usize + 1]).unwrap();
        // 预置一份旧备份（应在二次轮转时被删，最多保留一份）
        std::fs::write(dir.join(ROTATED_FILE), "旧备份").unwrap();
        append_panic_line(&dir, "新记录\n").unwrap();
        let rotated = std::fs::read(dir.join(ROTATED_FILE)).unwrap();
        assert_eq!(rotated.len(), MAX_LOG_BYTES as usize + 1); // 旧 app.log 整体挪走
        let body = std::fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        assert_eq!(body, "新记录\n"); // 新文件只含新记录
        // 再写一条不轮转（未超限）
        append_panic_line(&dir, "又一条\n").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(LOG_FILE)).unwrap(), "新记录\n又一条\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_line_format_is_stable() {
        assert_eq!(
            compose_line("2026-08-30 10:00:00", "src/main.rs:12:5", "测试爆炸"),
            "[2026-08-30 10:00:00] panic at src/main.rs:12:5: 测试爆炸\n"
        );
        assert_eq!(LOG_FILE, "app.log");
        assert_eq!(ROTATED_FILE, "app.log.1");
        assert_eq!(MAX_LOG_BYTES, 1024 * 1024);
    }

    #[test]
    fn payload_text_covers_str_string_and_other() {
        assert_eq!(payload_text(&"字符串切片"), "字符串切片");
        assert_eq!(payload_text(&String::from("堆字符串")), "堆字符串");
        assert_eq!(payload_text(&42usize), "非文本 panic payload");
    }
}
