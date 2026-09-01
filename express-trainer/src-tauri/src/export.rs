//! 导出：保存对话框写文件 + Obsidian vault 直写。
//!
//! - `save_text_file`：tauri-plugin-dialog 弹保存框，写入所选路径
//! - `export_obsidian`：vaultPath 已配置时直接写 `vault/表达训练/YYYY-MM-DD-主题.md`
//!   （报告与逐字稿各一个入口，kind = "report" | "transcript"）；未配置则退回保存框

use crate::settings;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

pub const OBSIDIAN_DIR: &str = "表达训练";
pub const MAX_TITLE_CHARS: usize = 50;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOutcome {
    /// true = 已写入文件；false = 用户取消或回退后取消
    pub saved: bool,
    pub path: Option<String>,
}

/// 文件名非法字符替换为全角或下划线，去掉首尾空白，超长截断
pub fn sanitize_title(title: &str) -> String {
    let cleaned: String = title
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect();
    let cleaned = cleaned.trim_matches('_').to_string();
    cleaned.chars().take(MAX_TITLE_CHARS).collect::<String>().trim().to_string()
}

/// kind + 主题 + 日期 → 文件名（不含目录）
pub fn obsidian_file_name(kind: &str, title: &str, date: &str) -> String {
    let safe = sanitize_title(title);
    let stem = if safe.is_empty() { date.to_string() } else { format!("{date}-{safe}") };
    if kind == "transcript" {
        format!("{stem}-逐字稿.md")
    } else {
        format!("{stem}.md")
    }
}

/// 已存在时追加 -2 / -3 … 序号（before 扩展名）
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let first = dir.join(file_name);
    if !first.exists() {
        return first;
    }
    let stem = Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or(file_name);
    let ext = Path::new(file_name).extension().and_then(|s| s.to_str()).unwrap_or("");
    for n in 2..1000 {
        let candidate = if ext.is_empty() {
            dir.join(format!("{stem}-{n}"))
        } else {
            dir.join(format!("{stem}-{n}.{ext}"))
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

fn default_file_name(kind: &str, title: &str, date: &str) -> String {
    obsidian_file_name(kind, title, date)
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 弹保存框并写入。default_dir 为空时尝试桌面目录。
/// 返回 saved=false 表示用户取消。
#[tauri::command]
pub async fn save_text_file(
    app: AppHandle,
    content: String,
    suggested_name: String,
    default_dir: Option<String>,
) -> Result<SaveOutcome, String> {
    // 保存对话框必须阻塞在工作线程上跑，不能占用异步运行时
    let app_for_dialog = app.clone();
    let name = suggested_name.clone();
    let dir = match default_dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => Some(PathBuf::from(d)),
        None => app.path().desktop_dir().ok(),
    };
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut builder = app_for_dialog
            .dialog()
            .file()
            .add_filter("Markdown / 文本", &["md", "txt"]);
        if !name.is_empty() {
            builder = builder.set_file_name(&name);
        }
        if let Some(d) = dir {
            builder = builder.set_directory(d);
        }
        builder.blocking_save_file()
    })
    .await
    .map_err(|e| format!("保存对话框异常：{e}"))?;

    let Some(file_path) = picked else {
        return Ok(SaveOutcome { saved: false, path: None });
    };
    let path: PathBuf = file_path.into_path().map_err(|e| format!("无效的保存路径：{e}"))?;
    std::fs::write(&path, content.as_bytes()).map_err(|e| format!("写入文件失败：{e}"))?;
    Ok(SaveOutcome { saved: true, path: Some(path.to_string_lossy().into_owned()) })
}

/// Obsidian 直写：vaultPath/表达训练/YYYY-MM-DD-主题.md；未配置 vault 时退回保存框
#[tauri::command]
pub async fn export_obsidian(
    app: AppHandle,
    content: String,
    title: Option<String>,
    kind: String,
) -> Result<SaveOutcome, String> {
    let settings = settings::load(&app);
    let vault = settings.obsidian_vault_path.trim().to_string();
    let title = title.as_deref().unwrap_or("").trim().to_string();

    if vault.is_empty() {
        // 未配置 vault：退回系统保存框
        return save_text_file(
            app,
            content,
            default_file_name(&kind, &title, &today()),
            None,
        )
        .await;
    }

    let dir = PathBuf::from(&vault).join(OBSIDIAN_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败（检查 vault 路径是否有效）：{e}"))?;
    let file_name = obsidian_file_name(&kind, &title, &today());
    let path = unique_path(&dir, &file_name);
    std::fs::write(&path, content.as_bytes()).map_err(|e| format!("写入文件失败：{e}"))?;
    Ok(SaveOutcome { saved: true, path: Some(path.to_string_lossy().into_owned()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_illegal_chars_and_whitespace() {
        assert_eq!(sanitize_title("  自律/练习:第一课*?  "), "自律_练习_第一课");
        assert_eq!(sanitize_title(""), "");
        assert_eq!(sanitize_title("///"), "");
    }

    #[test]
    fn sanitize_truncates_long_titles() {
        let long = "长".repeat(80);
        assert_eq!(sanitize_title(&long).chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn obsidian_names_for_kinds_and_empty_title() {
        assert_eq!(obsidian_file_name("report", "自律", "2026-08-30"), "2026-08-30-自律.md");
        assert_eq!(
            obsidian_file_name("transcript", "自律", "2026-08-30"),
            "2026-08-30-自律-逐字稿.md"
        );
        assert_eq!(obsidian_file_name("report", "", "2026-08-30"), "2026-08-30.md");
    }

    #[test]
    fn unique_path_appends_counter_on_collision() {
        let dir = std::env::temp_dir().join(format!("sm-export-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 第一次与第二次同名 → 第二次应得 -2
        let p1 = unique_path(&dir, "2026-08-30-自律.md");
        assert_eq!(p1, dir.join("2026-08-30-自律.md"));
        std::fs::write(&p1, b"x").unwrap();
        let p2 = unique_path(&dir, "2026-08-30-自律.md");
        assert_eq!(p2, dir.join("2026-08-30-自律-2.md"));
        std::fs::write(&p2, b"x").unwrap();
        let p3 = unique_path(&dir, "2026-08-30-自律.md");
        assert_eq!(p3, dir.join("2026-08-30-自律-3.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_outcome_serializes_camel_case() {
        let o = SaveOutcome { saved: true, path: Some("x.md".into()) };
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(v["saved"], true);
        assert_eq!(v["path"], "x.md");
    }
}
