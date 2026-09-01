//! 应用内模型下载器（M4 首启引导 · 产品方案 §6 M4）。
//!
//! 职责：
//! - 检测模型完整性（必需：Silero VAD + 流式 Paraformer 中英双语 int8；
//!   推荐必装：SenseVoice 离线高精引擎 int8——0.2.2 起默认随必需组件一起
//!   下载，仅当用户在引导页显式确认「仅下载必需组件」时才跳过；缺失不影响
//!   可用性，终稿回退流式引擎）
//! - 从镜像下载缺失模型（github.com 直连在目标网络不可达，镜像顺序：ghfast.top
//!   反代优先 → HuggingFace 官方/hf-mirror 镜像兜底；与 scripts/download-models.ps1 同源）
//! - tar.bz2 解压并删除 fp32 权重（与 ps1 逻辑一致，只留 int8）
//! - 可中断重试：下载写入 `<file>.part`，重试带 HTTP Range 断点续传；
//!   服务端不支持 Range 时自动整档重下；失败一律返回 Err，绝不 panic
//!
//! 事件契约（前端 OnboardingView / SettingsView 监听）：
//! - `download_progress` `{ file, received, total }`（total=0 表示未知长度，前端显示不确定进度）
//! - `download_done`   `{ modelsDir, missing }`（成功时 missing 恒为空）

use crate::AppState;
use bzip2::read::BzDecoder;
use futures_util::StreamExt;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// 识别模型目录名（与 session.rs 的 PARA_DIR 一致）
pub const PARA_DIR_NAME: &str = "sherpa-onnx-streaming-paraformer-bilingual-zh-en";
/// 可选离线高精引擎目录名（与 session.rs 的 SENSE_VOICE_DIR 一致）
pub const SENSE_DIR_NAME: &str = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17";
const VAD_FILE: &str = "silero_vad.onnx";
/// Paraformer 必需文件（int8 + 词表；fp32 权重不下载/下载后删除）
const PARA_FILES: &[&str] = &["tokens.txt", "encoder.int8.onnx", "decoder.int8.onnx"];
const PARA_ARCHIVE: &str = "sherpa-onnx-streaming-paraformer-bilingual-zh-en.tar.bz2";
/// SenseVoice 必需文件（int8 + 词表；同样是 fp32 不保留）
const SENSE_FILES: &[&str] = &["tokens.txt", "model.int8.onnx"];
const SENSE_ARCHIVE: &str = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2";

const GH_RELEASE_BASE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";
const GHFAST_PROXY_BASE: &str =
    "https://ghfast.top/https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";
/// HuggingFace 官方站与国内镜像（同一文件两处都有）
const HF_BASES: &[&str] = &["https://huggingface.co", "https://hf-mirror.com"];
const HF_VAD_REPO: &str = "csukuangfj/vad";
const HF_PARA_REPO: &str = "csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en";
const HF_SENSE_REPO: &str = "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17";

/// 进度事件最小步长（字节）：1GB 文件约 2000 条事件，避免刷屏
pub const PROGRESS_EMIT_STEP: u64 = 512 * 1024;

// ---------------------------------------------------------------------------
// URL 构造（纯函数，供单测）
// ---------------------------------------------------------------------------

/// GitHub release 资产候选源：ghfast.top 反代优先（github.com 直连常不可达），
/// 直连兜底（海外网络可用）。
pub fn gh_asset_urls(relative: &str) -> Vec<String> {
    vec![
        format!("{GHFAST_PROXY_BASE}/{relative}"),
        format!("{GH_RELEASE_BASE}/{relative}"),
    ]
}

/// HuggingFace 散文件候选源：官方站优先，hf-mirror.com 镜像兜底。
pub fn hf_file_urls(repo: &str, file: &str) -> Vec<String> {
    HF_BASES
        .iter()
        .map(|b| format!("{b}/{repo}/resolve/main/{file}"))
        .collect()
}

// ---------------------------------------------------------------------------
// 进度与断点（纯函数，供单测）
// ---------------------------------------------------------------------------

/// 下载百分比（一位小数，封顶 100）；total=0（未知长度）→ None
pub fn progress_percent(received: u64, total: u64) -> Option<f64> {
    if total == 0 {
        None
    } else {
        let pct = ((received as f64 / total as f64) * 1000.0).round() / 10.0;
        Some(pct.clamp(0.0, 100.0))
    }
}

/// 是否到达发进度事件的粒度
pub fn progress_due(received: u64, last_emitted: u64) -> bool {
    received >= last_emitted + PROGRESS_EMIT_STEP
}

/// 解析 `Content-Range: bytes 100-999/2000` 中的总长度（`*` / 非法 → None）
pub fn parse_content_range_total(value: &str) -> Option<u64> {
    let total = value.trim().rsplit('/').next()?.trim();
    if total == "*" {
        return None;
    }
    total.parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDecision {
    /// 服务端返回 206：从 offset 追加写入
    Append { offset: u64 },
    /// 服务端不支持 Range（200 全量）或响应异常：整档重下
    Restart,
}

/// 已带 `Range: bytes=part_len-` 请求后，依据响应决定续传或重下
pub fn resume_decision(part_len: u64, status: u16, range_total: Option<u64>) -> ResumeDecision {
    match (status, range_total) {
        (206, Some(total)) if part_len > 0 && total >= part_len => {
            ResumeDecision::Append { offset: part_len }
        }
        _ => ResumeDecision::Restart,
    }
}

/// 断点文件路径：`<dest>.part`
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

// ---------------------------------------------------------------------------
// 完整性检测与目录解析
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsStatus {
    pub models_dir: String,
    pub missing: Vec<String>,
    /// SenseVoice 缺失文件；非空不影响可用性（终稿回退流式引擎）。
    /// 旧字段名，内容与 recommended_missing 一致，保留兼容
    pub optional_missing: Vec<String>,
    /// 推荐安装（0.2.2 起默认必装）但缺失的组件文件：目前即 SenseVoice。
    /// 前端据此提示「补装高精引擎」；跳过安装必须经用户显式确认
    pub recommended_missing: Vec<String>,
}

/// 缺失的必需模型文件清单（相对 models 目录）；全部就绪 → 空。
/// SenseVoice 是可选组件，不进本清单（缺失不算模型不完整）。
pub fn missing_models(models_dir: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    if !models_dir.join(VAD_FILE).is_file() {
        missing.push(VAD_FILE.to_string());
    }
    let para = models_dir.join(PARA_DIR_NAME);
    for f in PARA_FILES {
        if !para.join(f).is_file() {
            missing.push(format!("{PARA_DIR_NAME}/{f}"));
        }
    }
    missing
}

/// 可选高精引擎（SenseVoice）缺失文件清单；就绪 → 空
pub fn missing_sense_voice(models_dir: &Path) -> Vec<String> {
    let dir = models_dir.join(SENSE_DIR_NAME);
    SENSE_FILES
        .iter()
        .filter(|f| !dir.join(f).is_file())
        .map(|f| format!("{SENSE_DIR_NAME}/{f}"))
        .collect()
}

/// 模型目录搜索顺序：
/// 1. 应用数据目录（安装版首启引导的下载位置）
/// 2. exe 同级 models/（便携部署手动放置）
/// 3. resource_dir/../models（Tauri 开发模式）
/// 4. 仓库根 models/（开发模式：crate 根的上一级）
fn models_search_paths(app: &AppHandle) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(data) = app.path().app_data_dir() {
        paths.push(data.join("models"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("models"));
        }
    }
    if let Ok(res) = app.path().resource_dir() {
        paths.push(res.join("..").join("models"));
    }
    paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("models"));
    paths
}

/// 已齐备的模型目录（任一搜索路径完整即可）；都不齐 → None
pub fn find_models_dir(app: &AppHandle) -> Option<PathBuf> {
    models_search_paths(app)
        .into_iter()
        .find(|d| missing_models(d).is_empty())
}

/// 下载目标目录：应用数据目录下的 models（不存在则创建）
fn download_target_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法定位应用数据目录: {e}"))?
        .join("models");
    fs::create_dir_all(&dir).map_err(|e| format!("创建模型目录失败: {e}"))?;
    Ok(dir)
}

fn status_of(dir: &Path) -> ModelsStatus {
    ModelsStatus {
        models_dir: dir.to_string_lossy().into_owned(),
        missing: missing_models(dir),
        optional_missing: missing_sense_voice(dir),
        recommended_missing: missing_sense_voice(dir),
    }
}

// ---------------------------------------------------------------------------
// 下载范围（download_models 的 scope 参数）
// ---------------------------------------------------------------------------

/// 下载范围：
/// - Full：必需组件 + SenseVoice 高精引擎（0.2.2 起默认，推荐）
/// - RequiredOnly：仅必需组件（Paraformer + VAD）——只有用户在引导页
///   显式确认「仅下载必需组件」后才允许（定稿精度明显下降且不含标点）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadScope {
    Full,
    RequiredOnly,
}

impl DownloadScope {
    /// 是否包含 SenseVoice 高精引擎
    pub fn includes_sense_voice(self) -> bool {
        matches!(self, DownloadScope::Full)
    }
}

/// scope 参数解析（白名单）：缺省与 "full" → 全量；"requiredOnly" → 仅必需；
/// 其余取值一律拒绝——前端拼错时宁可报错，也不能静默跳过高精引擎
pub fn parse_download_scope(raw: Option<&str>) -> Result<DownloadScope, String> {
    match raw {
        None | Some("full") => Ok(DownloadScope::Full),
        Some("requiredOnly") => Ok(DownloadScope::RequiredOnly),
        Some(other) => Err(format!("未知的下载范围「{other}」（仅支持 full / requiredOnly）")),
    }
}

// ---------------------------------------------------------------------------
// 下载
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    file: String,
    received: u64,
    total: u64,
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .user_agent(format!("SpeakMirror/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("构建 HTTP 客户端失败")
}

fn emit_progress(app: &AppHandle, file: &str, received: u64, total: u64) {
    let _ = app.emit(
        "download_progress",
        DownloadProgress {
            file: file.to_string(),
            received,
            total,
        },
    );
}

/// 依次尝试镜像列表，任一成功即返回；全部失败时汇总各源错误
async fn download_first_mirror(
    client: &reqwest::Client,
    app: &AppHandle,
    urls: &[String],
    display: &str,
    dest: &Path,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for url in urls {
        match download_one(client, app, url, display, dest).await {
            Ok(()) => return Ok(()),
            Err(e) => errors.push(format!("{url} → {e}")),
        }
    }
    Err(errors.join("；"))
}

/// 单源流式下载（写 .part，支持 Range 断点续传，完成校验长度后落位）
async fn download_one(
    client: &reqwest::Client,
    app: &AppHandle,
    url: &str,
    display: &str,
    dest: &Path,
) -> Result<(), String> {
    let part = part_path(dest);
    let part_len = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let mut req = client.get(url);
    if part_len > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={part_len}-"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    let range_total = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_range_total);
    // 206 时 Content-Length 是「剩余长度」而非总长，总长以 Content-Range 为准
    let total = range_total.or_else(|| resp.content_length()).unwrap_or(0);
    let (mut file, offset) = match resume_decision(part_len, status, range_total) {
        ResumeDecision::Append { offset } => (
            OpenOptions::new()
                .append(true)
                .open(&part)
                .map_err(|e| format!("打开断点文件失败: {e}"))?,
            offset,
        ),
        ResumeDecision::Restart => (
            fs::File::create(&part).map_err(|e| format!("创建临时文件失败: {e}"))?,
            0,
        ),
    };

    let mut received = offset;
    let mut last_emit = 0;
    emit_progress(app, display, received, total);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("传输中断: {e}"))?;
        file.write_all(&chunk).map_err(|e| format!("写盘失败: {e}"))?;
        received += chunk.len() as u64;
        if progress_due(received, last_emit) {
            emit_progress(app, display, received, total);
            last_emit = received;
        }
    }
    file.flush().map_err(|e| format!("写盘失败: {e}"))?;
    if total > 0 && received != total {
        return Err(format!("下载不完整（{received}/{total} 字节），已保留断点，可重试续传"));
    }
    // Windows 上 rename 不覆盖已存在文件，先删旧目标
    let _ = fs::remove_file(dest);
    fs::rename(&part, dest).map_err(|e| format!("落盘失败: {e}"))?;
    emit_progress(app, display, received, total.max(received));
    Ok(())
}

// ---------------------------------------------------------------------------
// 解压（tar.bz2）
// ---------------------------------------------------------------------------

/// 解压 release 归档（顶层目录 dir_name）到 models_dir 并删除非 int8 权重
/// （与 ps1 同款清理）。归档用完即删（删除失败不影响模型可用性）。
pub fn extract_archive(archive: &Path, dir_name: &str, models_dir: &Path) -> Result<(), String> {
    // 清掉半解压残留，避免新旧混杂
    let _ = fs::remove_dir_all(models_dir.join(dir_name));
    let f = fs::File::open(archive).map_err(|e| format!("打开归档失败: {e}"))?;
    tar::Archive::new(BzDecoder::new(f))
        .unpack(models_dir)
        .map_err(|e| format!("解压失败: {e}"))?;
    strip_non_int8_onnx(&models_dir.join(dir_name))?;
    let _ = fs::remove_file(archive);
    Ok(())
}

/// 解压 Paraformer release 归档（顶层目录即 PARA_DIR_NAME）
pub fn extract_paraformer_archive(archive: &Path, models_dir: &Path) -> Result<(), String> {
    extract_archive(archive, PARA_DIR_NAME, models_dir)
}

/// 解压 SenseVoice release 归档（顶层目录即 SENSE_DIR_NAME）
pub fn extract_sense_voice_archive(archive: &Path, models_dir: &Path) -> Result<(), String> {
    extract_archive(archive, SENSE_DIR_NAME, models_dir)
}

/// 删除指定模型目录下非 int8 的 .onnx 权重（应用只用 int8），返回删除数量
pub fn strip_non_int8_onnx(dir: &Path) -> Result<usize, String> {
    let mut removed = 0;
    let entries = fs::read_dir(dir).map_err(|e| format!("读取模型目录失败: {e}"))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("读取模型目录失败: {e}"))?.path();
        let name_is_int8 = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.contains("int8"))
            .unwrap_or(true);
        let is_onnx = path.extension().and_then(|e| e.to_str()) == Some("onnx");
        if is_onnx && !name_is_int8 {
            fs::remove_file(&path).map_err(|e| format!("删除 fp32 权重失败: {e}"))?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// 删除 Paraformer 目录下非 int8 的 .onnx 权重，返回删除数量
pub fn strip_fp32_weights(models_dir: &Path) -> Result<usize, String> {
    strip_non_int8_onnx(&models_dir.join(PARA_DIR_NAME))
}

// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

/// 检查模型完整性：找到已齐备的目录则 missing 为空，否则给出目标目录与缺失清单
#[tauri::command]
pub fn check_models(app: AppHandle) -> ModelsStatus {
    match find_models_dir(&app) {
        Some(dir) => status_of(&dir),
        // 无齐备目录：报告下载目标目录及其缺失清单
        None => {
            let dir = app
                .path()
                .app_data_dir()
                .map(|d| d.join("models"))
                .unwrap_or_else(|_| PathBuf::from("models"));
            status_of(&dir)
        }
    }
}

/// 下载缺失模型（首启引导 / 设置页补装调用）。scope 白名单见
/// [`parse_download_scope`]：缺省 "full"（必需 + SenseVoice 全量，0.2.2 起默认）；
/// "requiredOnly" 仅必需组件，只允许用户显式确认后由前端传入。
/// 并发保护：同时只允许一个下载任务。
#[tauri::command]
pub async fn download_models(
    app: AppHandle,
    state: State<'_, AppState>,
    scope: Option<String>,
) -> Result<ModelsStatus, String> {
    let scope = parse_download_scope(scope.as_deref())?;
    if state
        .downloading
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("下载已在进行中，请稍候".into());
    }
    let result = run_download(&app, scope).await;
    state.downloading.store(false, Ordering::SeqCst);
    let status = result?;
    let _ = app.emit("download_done", &status);
    Ok(status)
}

async fn run_download(app: &AppHandle, scope: DownloadScope) -> Result<ModelsStatus, String> {
    // 必需模型：任一搜索路径已齐备 → 直接复用该目录（开发机复用仓库 models 即此路径），
    // 否则下载到应用数据目录
    let (dir, required_done) = match find_models_dir(app) {
        Some(dir) => (dir, true),
        None => (download_target_dir(app)?, false),
    };
    let client = build_client();

    if !required_done {
        // 1) Silero VAD（单文件）：ghfast → GitHub 直连 → HuggingFace 两站
        if !dir.join(VAD_FILE).is_file() {
            let mut urls = gh_asset_urls(VAD_FILE);
            urls.extend(hf_file_urls(HF_VAD_REPO, VAD_FILE));
            download_first_mirror(&client, app, &urls, VAD_FILE, &dir.join(VAD_FILE))
                .await
                .map_err(|e| format!("Silero VAD 下载失败：{e}"))?;
        }

        // 2) Paraformer
        if missing_models(&dir)
            .iter()
            .any(|m| m.starts_with(PARA_DIR_NAME))
        {
            // 计划 A：GitHub release 归档（ghfast 优先 → 直连兜底），解压后删 fp32
            let archive = dir.join("paraformer.tar.bz2");
            let plan_a = download_first_mirror(
                &client,
                app,
                &gh_asset_urls(PARA_ARCHIVE),
                "paraformer.tar.bz2",
                &archive,
            )
            .await
            .and_then(|()| extract_paraformer_archive(&archive, &dir));
            if let Err(gh_err) = plan_a {
                // 计划 B：HuggingFace 散文件（只下必需的 int8 + 词表，约 230MB）
                let para = dir.join(PARA_DIR_NAME);
                fs::create_dir_all(&para).map_err(|e| format!("创建模型目录失败: {e}"))?;
                for f in PARA_FILES {
                    download_first_mirror(&client, app, &hf_file_urls(HF_PARA_REPO, f), f, &para.join(f))
                        .await
                        .map_err(|hf_err| {
                            format!("GitHub 归档与 HuggingFace 镜像均失败。\n归档: {gh_err}\n镜像: {hf_err}")
                        })?;
                }
            }
        }

        let missing = missing_models(&dir);
        if !missing.is_empty() {
            return Err(format!("下载完成但模型仍不完整：{}", missing.join("，")));
        }
    }

    // 3) SenseVoice 高精引擎（0.2.2 起默认必装）：仅当用户显式选择
    //    「仅下载必需组件」时跳过（scope=RequiredOnly）。全量下载失败要如实
    //    报错——此时必需组件已就绪，用户可重试完整下载（断点续传）或退回
    //    仅必需组件；缺失时终稿仍回退流式引擎兜底（session.rs 不受影响）
    if scope.includes_sense_voice() && !missing_sense_voice(&dir).is_empty() {
        if let Err(e) = download_sense_voice(&client, app, &dir).await {
            return Err(format!(
                "高精引擎（SenseVoice）下载失败：{e}。必需组件已就绪，可重试完整下载（已下载部分支持断点续传）；网络受限时也可在引导页选择「仅下载必需组件」继续"
            ));
        }
    }

    Ok(status_of(&dir))
}

/// 下载 SenseVoice 高精引擎：镜像顺序与 Paraformer 一致
/// （release 归档 ghfast → 直连，失败转 HuggingFace 散文件）
async fn download_sense_voice(
    client: &reqwest::Client,
    app: &AppHandle,
    dir: &Path,
) -> Result<(), String> {
    // 计划 A：release 归档，解压后删 fp32
    let archive = dir.join("sense-voice.tar.bz2");
    let plan_a = download_first_mirror(
        client,
        app,
        &gh_asset_urls(SENSE_ARCHIVE),
        "sense-voice.tar.bz2",
        &archive,
    )
    .await
    .and_then(|()| extract_sense_voice_archive(&archive, dir));
    if let Err(gh_err) = plan_a {
        // 计划 B：HuggingFace 散文件（只下 int8 + 词表，约 230MB）
        let sense = dir.join(SENSE_DIR_NAME);
        fs::create_dir_all(&sense).map_err(|e| format!("创建模型目录失败: {e}"))?;
        for f in SENSE_FILES {
            download_first_mirror(client, app, &hf_file_urls(HF_SENSE_REPO, f), f, &sense.join(f))
                .await
                .map_err(|hf_err| {
                    format!("GitHub 归档与 HuggingFace 镜像均失败。\n归档: {gh_err}\n镜像: {hf_err}")
                })?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 测试（全部离线：URL 构造 / 进度计算 / 断点决策 / 完整性 / 解压）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_asset_urls_prefers_proxy_then_direct() {
        let urls = gh_asset_urls("silero_vad.onnx");
        assert_eq!(urls.len(), 2);
        assert_eq!(
            urls[0],
            "https://ghfast.top/https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"
        );
        assert_eq!(
            urls[1],
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"
        );
    }

    #[test]
    fn hf_file_urls_covers_official_and_mirror() {
        let urls = hf_file_urls("csukuangfj/vad", "silero_vad.onnx");
        assert_eq!(
            urls,
            vec![
                "https://huggingface.co/csukuangfj/vad/resolve/main/silero_vad.onnx".to_string(),
                "https://hf-mirror.com/csukuangfj/vad/resolve/main/silero_vad.onnx".to_string(),
            ]
        );
    }

    #[test]
    fn sense_voice_urls_follow_same_mirror_order() {
        // 归档：ghfast 反代优先 → GitHub 直连兜底
        let urls = gh_asset_urls(SENSE_ARCHIVE);
        assert_eq!(
            urls[0],
            "https://ghfast.top/https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2"
        );
        assert_eq!(
            urls[1],
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2"
        );
        // 散文件：HF 官方 → hf-mirror
        let urls = hf_file_urls(HF_SENSE_REPO, "model.int8.onnx");
        assert_eq!(
            urls,
            vec![
                "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/model.int8.onnx".to_string(),
                "https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/model.int8.onnx".to_string(),
            ]
        );
    }

    #[test]
    fn progress_percent_handles_unknown_and_rounds() {
        assert_eq!(progress_percent(123, 0), None);
        assert_eq!(progress_percent(0, 200), Some(0.0));
        assert_eq!(progress_percent(100, 200), Some(50.0));
        // 一位小数四舍五入：1/3 ≈ 33.3
        assert_eq!(progress_percent(1, 3), Some(33.3));
        // 超出按 100 计（续传时 received 可能短暂超过未知 total 的估计）
        assert_eq!(progress_percent(300, 200), Some(100.0));
    }

    #[test]
    fn progress_due_requires_half_mib_step() {
        assert!(!progress_due(0, 0));
        assert!(!progress_due(PROGRESS_EMIT_STEP - 1, 0));
        assert!(progress_due(PROGRESS_EMIT_STEP, 0));
        // 续传场景：上次已发 offset，再累积半 MiB 才发
        assert!(!progress_due(PROGRESS_EMIT_STEP + 10, PROGRESS_EMIT_STEP));
        assert!(progress_due(2 * PROGRESS_EMIT_STEP, PROGRESS_EMIT_STEP));
    }

    #[test]
    fn parse_content_range_total_variants() {
        assert_eq!(parse_content_range_total("bytes 100-999/2000"), Some(2000));
        assert_eq!(parse_content_range_total("bytes 0-0/123"), Some(123));
        assert_eq!(parse_content_range_total("bytes */123"), Some(123));
        assert_eq!(parse_content_range_total("bytes 100-999/*"), None);
        assert_eq!(parse_content_range_total("garbage"), None);
        assert_eq!(parse_content_range_total("bytes 1-2/abc"), None);
    }

    #[test]
    fn resume_decision_append_vs_restart() {
        use ResumeDecision::*;
        // 206 且总长 ≥ 断点长度 → 追加
        assert_eq!(resume_decision(100, 206, Some(2000)), Append { offset: 100 });
        assert_eq!(resume_decision(2000, 206, Some(2000)), Append { offset: 2000 });
        // 200（服务端忽略 Range）→ 重下
        assert_eq!(resume_decision(100, 200, Some(2000)), Restart);
        // 无 Content-Range 的 206 / 断点比总长还长（陈旧文件）→ 重下
        assert_eq!(resume_decision(100, 206, None), Restart);
        assert_eq!(resume_decision(3000, 206, Some(2000)), Restart);
        // 空 .part（part_len=0）按普通全量下载处理
        assert_eq!(resume_decision(0, 206, Some(2000)), Restart);
        assert_eq!(resume_decision(0, 200, Some(2000)), Restart);
    }

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(
            part_path(Path::new("models/silero_vad.onnx")),
            PathBuf::from("models/silero_vad.onnx.part")
        );
        assert_eq!(
            part_path(Path::new("models/paraformer.tar.bz2")),
            PathBuf::from("models/paraformer.tar.bz2.part")
        );
    }

    #[test]
    fn missing_models_detects_each_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // 空目录：VAD + 3 个 Paraformer 文件全缺
        assert_eq!(missing_models(dir).len(), 4);
        assert!(missing_models(dir).contains(&"silero_vad.onnx".to_string()));
        assert!(missing_models(dir)
            .contains(&format!("{PARA_DIR_NAME}/tokens.txt")));

        // 只有 VAD：Paraformer 3 个仍缺
        fs::write(dir.join(VAD_FILE), b"x").unwrap();
        assert_eq!(missing_models(dir).len(), 3);
        assert!(missing_models(dir)
            .iter()
            .all(|m| m.starts_with(PARA_DIR_NAME)));

        // 补齐 int8 + 词表（fp32 权重不算必需）→ 完整
        let para = dir.join(PARA_DIR_NAME);
        fs::create_dir_all(&para).unwrap();
        for f in PARA_FILES {
            fs::write(para.join(f), b"x").unwrap();
        }
        assert!(missing_models(dir).is_empty());
    }

    #[test]
    fn extract_strips_fp32_and_keeps_int8() {
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path();
        // VAD 先行就位（解压只负责 Paraformer 部分）
        fs::write(models.join(VAD_FILE), b"vad").unwrap();
        let archive = write_test_archive(
            tmp.path(),
            &[
                (format!("{PARA_DIR_NAME}/tokens.txt"), "a b".to_string()),
                (format!("{PARA_DIR_NAME}/encoder.onnx"), "fp32".to_string()),
                (format!("{PARA_DIR_NAME}/decoder.onnx"), "fp32".to_string()),
                (
                    format!("{PARA_DIR_NAME}/encoder.int8.onnx"),
                    "int8".to_string(),
                ),
                (
                    format!("{PARA_DIR_NAME}/decoder.int8.onnx"),
                    "int8".to_string(),
                ),
            ],
        );
        extract_paraformer_archive(&archive, models).unwrap();

        let para = models.join(PARA_DIR_NAME);
        assert!(para.join("tokens.txt").is_file());
        assert!(para.join("encoder.int8.onnx").is_file());
        assert!(para.join("decoder.int8.onnx").is_file());
        // fp32 权重被删除（与 ps1 的 notmatch "int8" 一致）
        assert!(!para.join("encoder.onnx").exists());
        assert!(!para.join("decoder.onnx").exists());
        // 归档删除
        assert!(!archive.exists());
        assert!(missing_models(models).is_empty());
    }

    #[test]
    fn strip_fp32_is_idempotent_on_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // 目录不存在时不 panic（extract 前置流程不会走到，但函数本身要稳）
        assert!(strip_fp32_weights(tmp.path()).is_err());
        let para = tmp.path().join(PARA_DIR_NAME);
        fs::create_dir_all(&para).unwrap();
        assert_eq!(strip_fp32_weights(tmp.path()).unwrap(), 0);
    }

    #[test]
    fn optional_sense_voice_missing_does_not_block_readiness() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // 必需模型齐备但 SenseVoice 缺失：missing 仍为空（不影响可用性）
        fs::write(dir.join(VAD_FILE), b"x").unwrap();
        let para = dir.join(PARA_DIR_NAME);
        fs::create_dir_all(&para).unwrap();
        for f in PARA_FILES {
            fs::write(para.join(f), b"x").unwrap();
        }
        assert!(missing_models(dir).is_empty());
        assert_eq!(missing_sense_voice(dir).len(), 2);
        // 状态里单独呈现：旧字段 optional_missing 与新字段 recommended_missing 一致
        let status = status_of(dir);
        assert!(status.missing.is_empty());
        assert_eq!(status.optional_missing.len(), 2);
        assert_eq!(status.recommended_missing.len(), 2);
        assert!(status
            .optional_missing
            .contains(&format!("{SENSE_DIR_NAME}/model.int8.onnx")));
        // 补齐后归零
        let sense = dir.join(SENSE_DIR_NAME);
        fs::create_dir_all(&sense).unwrap();
        for f in SENSE_FILES {
            fs::write(sense.join(f), b"x").unwrap();
        }
        assert!(missing_sense_voice(dir).is_empty());
        assert!(status_of(dir).optional_missing.is_empty());
        assert!(status_of(dir).recommended_missing.is_empty());
    }

    #[test]
    fn status_recommended_missing_kept_out_of_required_list_and_camel_case() {
        let tmp = tempfile::tempdir().unwrap();
        // 空目录：必需缺失只进 missing；SenseVoice 缺失只进推荐清单，两者不混
        let status = status_of(tmp.path());
        assert_eq!(status.missing.len(), 4);
        assert!(!status.missing.iter().any(|m| m.contains("sense-voice")));
        assert_eq!(status.recommended_missing.len(), 2);
        assert!(status
            .recommended_missing
            .contains(&format!("{SENSE_DIR_NAME}/model.int8.onnx")));
        // 序列化键名 camelCase（前端契约），旧字段与新字段并存
        let v = serde_json::to_value(&status).unwrap();
        assert!(v.get("recommendedMissing").is_some());
        assert!(v.get("optionalMissing").is_some());
        assert!(v.get("missing").is_some());
    }

    #[test]
    fn parse_download_scope_defaults_full_and_rejects_unknown_values() {
        use DownloadScope::*;
        // 缺省 = 全量（必需 + SenseVoice）：download_models 默认下载全部三组
        assert_eq!(parse_download_scope(None).unwrap(), Full);
        assert_eq!(parse_download_scope(Some("full")).unwrap(), Full);
        // 跳过高精引擎必须显式传 requiredOnly
        assert_eq!(parse_download_scope(Some("requiredOnly")).unwrap(), RequiredOnly);
        // 白名单外的取值一律拒绝（大小写敏感）：拼错宁可报错，不可静默跳过高精引擎
        for bad in ["", "FULL", "RequiredOnly", "required-only", "skip", "sense"] {
            assert!(
                parse_download_scope(Some(bad)).is_err(),
                "scope 白名单应拒绝：{bad}"
            );
        }
        // 全量含高精引擎；仅必需不含——run_download 据此决定是否跳过 SenseVoice
        assert!(Full.includes_sense_voice());
        assert!(!RequiredOnly.includes_sense_voice());
    }

    #[test]
    fn extract_sense_voice_archive_strips_fp32() {
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path();
        let archive = write_test_archive(
            tmp.path(),
            &[
                (format!("{SENSE_DIR_NAME}/tokens.txt"), "a b".to_string()),
                (format!("{SENSE_DIR_NAME}/model.onnx"), "fp32".to_string()),
                (format!("{SENSE_DIR_NAME}/model.int8.onnx"), "int8".to_string()),
            ],
        );
        extract_sense_voice_archive(&archive, models).unwrap();
        let sense = models.join(SENSE_DIR_NAME);
        assert!(sense.join("tokens.txt").is_file());
        assert!(sense.join("model.int8.onnx").is_file());
        assert!(!sense.join("model.onnx").exists());
        assert!(!archive.exists());
        assert!(missing_sense_voice(models).is_empty());
    }

    /// 构造测试用 tar.bz2：含指定条目（`目录/文件名` → 内容）
    fn write_test_archive(dir: &Path, entries: &[(String, String)]) -> PathBuf {
        let archive = dir.join("test.tar.bz2");
        let f = fs::File::create(&archive).unwrap();
        let enc = bzip2::write::BzEncoder::new(f, bzip2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        for (name, content) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(content.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, name, content.as_bytes()).unwrap();
        }
        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap().sync_all().unwrap();
        archive
    }
}
