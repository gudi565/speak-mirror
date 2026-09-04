//! 用应用的完整识别管线转写任意受支持的音频文件：
//! 解码（wav/mp3/flac/m4a/aac）→ 重采样 16k 单声道 → Silero VAD 断句 → 终稿。
//! 与会话内识别行为一致（含收尾 flush），但不走规则引擎/UI，用于命令行快速检测素材。
//!
//! 用法：
//! ```text
//! cargo run --release --example transcribe_any -- [--compare] [--agc] [--max-seconds N] [--vad-threshold T] [--engines 逗号列表] [--threads N] <音频文件路径>
//! ```
//!
//! - `--compare` 双引擎对比：每个 VAD 段并排打印「流式终稿」（Paraformer）
//!   与「SenseVoice 终稿」（离线高精引擎），末尾统计两版总字数与耗时——
//!   量化双引擎改进的核心工具
//! - `--engines a,b,c` 显式选择参与对比的引擎集合（覆盖 --compare 的默认两引擎）：
//!   - `stream` 流式 Paraformer（会话主引擎）
//!   - `sense` SenseVoice（现役离线终稿引擎）
//!   - `fire-red-ctc` FireRedASR2-CTC int8（候选终稿引擎，评估 spike 用）
//!   例：`--engines stream,sense,fire-red-ctc` 三引擎并排；至少一个，不可重复
//! - `--threads N` 离线引擎（sense / fire-red-ctc）的推理线程数（默认 4，
//!   与 SenseVoice 会话配置一致；`--threads 1` 用于对标论文单线程 RTF）
//! - `--agc` 录音增强：先应用与会话「录音增强」相同的 auto_gain（过静整段
//!   线性放大）再跑识别——对比有无增益的终稿差异（过静手机录音的验收工具）
//! - `--max-seconds N` 只处理前 N 秒（长素材快速抽样）
//! - `--vad-threshold T` Silero VAD 阈值（默认 0.5 = 会话「标准」灵敏度；
//!   0.35 = 「高灵敏度」，静音占比高/尾音轻的录音用它对比断句效果）

use express_trainer_lib::audio::{auto_gain, frame_rms_p95, resample_to_16k_mono};
use express_trainer_lib::decode::decode_audio_file;
use express_trainer_lib::session::{
    build_asr_config, build_sense_voice_config, sense_voice_ready, transcribe_buffer,
    transcribe_offline, SENSE_VOICE_DIR,
};
use sherpa_onnx::{
    OfflineFireRedAsrCtcModelConfig, OfflineModelConfig, OfflineRecognizer,
    OfflineRecognizerConfig, OnlineRecognizer, SileroVadModelConfig, VadModelConfig,
    VoiceActivityDetector,
};
use std::path::PathBuf;
use std::time::Instant;

/// FireRedASR2-CTC int8 模型目录名（评估 spike 专用，不入 downloader）
const FIRE_RED_CTC_DIR: &str = "sherpa-onnx-fire-red-asr2-ctc-zh_en-int8-2026-02-25";

/// 参与对比的识别引擎（--engines 的取值）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// 流式 Paraformer（会话主引擎，partial + 终稿）
    Stream,
    /// SenseVoice（现役离线终稿引擎）
    SenseVoice,
    /// FireRedASR2-CTC int8（候选终稿引擎）
    FireRedCtc,
}

impl Engine {
    pub fn id(self) -> &'static str {
        match self {
            Engine::Stream => "stream",
            Engine::SenseVoice => "sense",
            Engine::FireRedCtc => "fire-red-ctc",
        }
    }

    /// 输出标签（每段并排打印时的行首）
    pub fn label(self) -> &'static str {
        match self {
            Engine::Stream => "流式",
            Engine::SenseVoice => "高精(SenseVoice)",
            Engine::FireRedCtc => "高精(FireRedCTC)",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "stream" => Some(Engine::Stream),
            "sense" | "sense-voice" => Some(Engine::SenseVoice),
            "fire-red-ctc" | "firered" => Some(Engine::FireRedCtc),
            _ => None,
        }
    }
}

/// 解析 `--engines` 逗号列表：至少一个、不可重复、名字必须合法
pub fn parse_engines(spec: &str) -> Result<Vec<Engine>, String> {
    let mut out: Vec<Engine> = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let e = Engine::from_id(part).ok_or_else(|| {
            format!("未知引擎: {part}（可选 stream / sense / fire-red-ctc）")
        })?;
        if out.contains(&e) {
            return Err(format!("引擎重复: {part}"));
        }
        out.push(e);
    }
    if out.is_empty() {
        return Err("--engines 至少需要一个引擎（stream / sense / fire-red-ctc）".to_string());
    }
    Ok(out)
}

/// 生效引擎集合（纯函数可测）：`--engines` 显式指定 > `--compare` 双引擎 > 默认仅流式
pub fn resolve_engines(compare: bool, engines: &Option<Vec<Engine>>) -> Vec<Engine> {
    if let Some(v) = engines {
        return v.clone();
    }
    if compare {
        vec![Engine::Stream, Engine::SenseVoice]
    } else {
        vec![Engine::Stream]
    }
}

/// FireRedASR2-CTC 模型就绪检查（与 sense_voice_ready 同款）
pub fn fire_red_ctc_ready(models_dir: &std::path::Path) -> bool {
    let dir = models_dir.join(FIRE_RED_CTC_DIR);
    dir.join("model.int8.onnx").is_file() && dir.join("tokens.txt").is_file()
}

/// FireRedASR2-CTC 配置构建（纯函数可测）：单模型 CTC 架构（encoder-decoder 融合
/// 在一个 onnx 里），线程数与 SenseVoice 对齐（默认 4，--threads 可改），
/// 建模单元留空走默认（cjkchar，仅影响热词，本工具不用热词）。
pub fn build_fire_red_ctc_config(models_dir: &std::path::Path, num_threads: i32) -> OfflineRecognizerConfig {
    let dir = models_dir.join(FIRE_RED_CTC_DIR);
    OfflineRecognizerConfig {
        model_config: OfflineModelConfig {
            fire_red_asr_ctc: OfflineFireRedAsrCtcModelConfig {
                model: Some(dir.join("model.int8.onnx").to_string_lossy().into_owned()),
            },
            tokens: Some(dir.join("tokens.txt").to_string_lossy().into_owned()),
            num_threads,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 命令行参数（解析逻辑纯函数化，供单测）
#[derive(Debug, PartialEq)]
pub struct Cli {
    pub path: PathBuf,
    /// 双引擎对比模式
    pub compare: bool,
    /// 只处理前 N 秒
    pub max_seconds: Option<f64>,
    /// Silero VAD 阈值（默认 0.5；高灵敏度用 0.35，对应设置里的「断句灵敏度」）
    pub vad_threshold: f32,
    /// 录音增强（自动增益）：先 auto_gain 再识别
    pub agc: bool,
    /// 显式引擎集合（None = 按 compare/默认规则解析）
    pub engines: Option<Vec<Engine>>,
    /// 离线引擎推理线程数（默认 4，与 SenseVoice 会话配置一致）
    pub threads: i32,
}

/// 解析参数：`[--compare] [--agc] [--max-seconds N] [--vad-threshold T] [--engines 列表] [--threads N] <path>`；标志与路径顺序不限
pub fn parse_cli(args: &[String]) -> Result<Cli, String> {
    let mut path: Option<PathBuf> = None;
    let mut compare = false;
    let mut agc = false;
    let mut max_seconds: Option<f64> = None;
    let mut vad_threshold = 0.5f32;
    let mut engines: Option<Vec<Engine>> = None;
    let mut threads = 4i32;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--compare" => compare = true,
            "--agc" => agc = true,
            "--max-seconds" => {
                let raw = args.get(i + 1).ok_or_else(|| "--max-seconds 需要一个数值参数".to_string())?;
                let v: f64 = raw.parse().map_err(|_| format!("--max-seconds 不是合法数字: {raw}"))?;
                if !v.is_finite() || v <= 0.0 {
                    return Err(format!("--max-seconds 必须为正数（收到 {raw}）"));
                }
                max_seconds = Some(v);
                i += 1; // 跳过数值参数
            }
            "--vad-threshold" => {
                let raw = args.get(i + 1).ok_or_else(|| "--vad-threshold 需要一个数值参数".to_string())?;
                let v: f32 = raw.parse().map_err(|_| format!("--vad-threshold 不是合法数字: {raw}"))?;
                if !v.is_finite() || !(0.01..=0.99).contains(&v) {
                    return Err(format!("--vad-threshold 需在 (0, 1) 内（收到 {raw}）"));
                }
                vad_threshold = v;
                i += 1;
            }
            "--engines" => {
                let raw = args.get(i + 1).ok_or_else(|| "--engines 需要一个逗号列表参数".to_string())?;
                engines = Some(parse_engines(raw)?);
                i += 1;
            }
            "--threads" => {
                let raw = args.get(i + 1).ok_or_else(|| "--threads 需要一个数值参数".to_string())?;
                let v: i32 = raw.parse().map_err(|_| format!("--threads 不是合法整数: {raw}"))?;
                if !(1..=16).contains(&v) {
                    return Err(format!("--threads 需在 1..=16（收到 {raw}）"));
                }
                threads = v;
                i += 1;
            }
            flag if flag.starts_with("--") => return Err(format!("未知参数: {flag}")),
            _ => {
                if path.is_some() {
                    return Err(format!("多余的位置参数: {a}"));
                }
                path = Some(PathBuf::from(a));
            }
        }
        i += 1;
    }
    Ok(Cli {
        path: path.ok_or("缺少音频文件路径")?,
        compare,
        max_seconds,
        vad_threshold,
        agc,
        engines,
        threads,
    })
}

/// 截取前 max_seconds 秒的 16k 样本（None 或超长 → 原样返回）
pub fn clamp_samples(samples: &[f32], sample_rate: usize, max_seconds: Option<f64>) -> &[f32] {
    match max_seconds {
        Some(sec) => {
            let n = ((sec * sample_rate as f64) as usize).min(samples.len());
            &samples[..n]
        }
        None => samples,
    }
}

/// 音量诊断（p50 = 100ms 帧 RMS 中位数，p95 见 audio::frame_rms_p95）
fn frame_rms_p50(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
    let mut frame_rms: Vec<f32> = samples.chunks(1600).map(rms).collect();
    frame_rms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    frame_rms[frame_rms.len() / 2]
}

/// 字数统计：非空白字符数（标点计入，标点密度本身是双引擎差异之一）
pub fn char_count(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse_cli(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("参数错误: {e}");
            eprintln!("用法: transcribe_any [--compare] [--agc] [--max-seconds N] [--vad-threshold T] [--engines stream,sense,fire-red-ctc] [--threads N] <音频文件路径>");
            std::process::exit(1);
        }
    };
    let engines = resolve_engines(cli.compare, &cli.engines);
    println!("参与引擎: {}", engines.iter().map(|e| e.id()).collect::<Vec<_>>().join(", "));
    if cli.threads != 4 {
        println!("离线引擎线程数: {}（默认 4）", cli.threads);
    }

    let decoded = match decode_audio_file(&cli.path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("解码失败: {e}");
            std::process::exit(2);
        }
    };
    println!(
        "解码成功: {} Hz / {} 声道 / {:.1} 秒",
        decoded.sample_rate,
        decoded.channels,
        decoded.duration_ms() as f64 / 1000.0
    );

    let all_samples = resample_to_16k_mono(&decoded.samples, decoded.sample_rate, decoded.channels);
    let clamped = clamp_samples(&all_samples, 16_000, cli.max_seconds);
    if cli.max_seconds.is_some() {
        println!("截取前 {:.1} 秒（{} 样本）", clamped.len() as f64 / 16.0 / 1000.0, clamped.len());
    }

    // 诊断：音量水平（VAD 对过静音频会漏切）
    println!(
        "音量诊断(前): 帧RMS p50={:.4} p95={:.4}（<0.005 偏静，VAD 可能漏切；0.02+ 正常）",
        frame_rms_p50(clamped),
        frame_rms_p95(clamped)
    );

    // --agc：与会话「录音增强」相同的 auto_gain（整段线性放大，达标不动）
    let agc_buf: Vec<f32>;
    let samples: &[f32] = if cli.agc {
        let (gained, gain) = auto_gain(clamped);
        if gain > 1.0 {
            println!("AGC: 电平过低，整段放大 {gain:.2}×（目标 p95≈0.15，上限 8×）");
        } else {
            println!("AGC: 电平达标，未放大（增益 1.0×，仅直流偏置消除）");
        }
        agc_buf = gained;
        println!(
            "音量诊断(后): 帧RMS p50={:.4} p95={:.4}",
            frame_rms_p50(&agc_buf),
            frame_rms_p95(&agc_buf)
        );
        &agc_buf
    } else {
        clamped
    };

    let models_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("models");

    let vad_cfg = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(models_dir.join("silero_vad.onnx").to_string_lossy().into_owned()),
            threshold: cli.vad_threshold,
            min_silence_duration: 0.5,
            min_speech_duration: 0.25,
            window_size: 512,
            max_speech_duration: 20.0,
        },
        ten_vad: Default::default(),
        sample_rate: 16_000,
        num_threads: 1,
        provider: Some("cpu".into()),
        debug: false,
    };
    let mut vad = VoiceActivityDetector::create(&vad_cfg, 30.0).expect("VAD 初始化失败");

    // 引擎按需加载：流式默认加载（主引擎）；离线引擎仅在参与对比时加载（含加载计时）
    let use_stream = engines.contains(&Engine::Stream);
    let recognizer = if use_stream {
        Some(OnlineRecognizer::create(&build_asr_config(&models_dir, None)).expect("ASR 初始化失败"))
    } else {
        None
    };
    let sense_voice = if engines.contains(&Engine::SenseVoice) {
        if !sense_voice_ready(&models_dir) {
            eprintln!(
                "--engines 含 sense：需要 SenseVoice 模型（models/{}/model.int8.onnx + tokens.txt），请先下载",
                SENSE_VOICE_DIR
            );
            std::process::exit(3);
        }
        let t0 = Instant::now();
        let r = OfflineRecognizer::create(&build_sense_voice_config(&models_dir))
            .expect("SenseVoice 初始化失败");
        println!("SenseVoice 引擎加载耗时: {:.0} ms", t0.elapsed().as_millis());
        Some(r)
    } else {
        None
    };
    let fire_red = if engines.contains(&Engine::FireRedCtc) {
        if !fire_red_ctc_ready(&models_dir) {
            eprintln!(
                "--engines 含 fire-red-ctc：需要 FireRedASR2-CTC 模型（models/{}/model.int8.onnx + tokens.txt），请先下载",
                FIRE_RED_CTC_DIR
            );
            std::process::exit(3);
        }
        let t0 = Instant::now();
        let r = OfflineRecognizer::create(&build_fire_red_ctc_config(&models_dir, cli.threads))
            .expect("FireRedASR2-CTC 初始化失败");
        println!(
            "FireRedASR2-CTC 引擎加载耗时（{} 线程）: {:.0} ms",
            cli.threads,
            t0.elapsed().as_millis()
        );
        Some(r)
    } else {
        None
    };

    // 每段各引擎终稿 + 各自解码耗时（与 engines 顺序对齐；本块 = 会话「极速」档的
    // 等价消费路径：不等任何时刻表，VAD/识别跑多快算多快，计时即极速会话的处理耗时）
    let mut rows: Vec<Vec<(String, u128)>> = Vec::new();
    let t_process = Instant::now();
    {
        let drain = |vad: &mut VoiceActivityDetector, out: &mut Vec<Vec<(String, u128)>>| {
            while !vad.is_empty() {
                if let Some(seg) = vad.front() {
                    let mut row: Vec<(String, u128)> = Vec::with_capacity(engines.len());
                    for &e in &engines {
                        match e {
                            Engine::Stream => {
                                let r = recognizer.as_ref().expect("流式引擎未加载");
                                let t0 = Instant::now();
                                let text = transcribe_buffer(r, seg.samples());
                                row.push((text.trim().to_string(), t0.elapsed().as_millis()));
                            }
                            Engine::SenseVoice => {
                                let r = sense_voice.as_ref().expect("SenseVoice 未加载");
                                let t0 = Instant::now();
                                let text = transcribe_offline(r, seg.samples());
                                row.push((text.trim().to_string(), t0.elapsed().as_millis()));
                            }
                            Engine::FireRedCtc => {
                                let r = fire_red.as_ref().expect("FireRedASR2-CTC 未加载");
                                let t0 = Instant::now();
                                let text = transcribe_offline(r, seg.samples());
                                row.push((text.trim().to_string(), t0.elapsed().as_millis()));
                            }
                        }
                    }
                    out.push(row);
                }
                vad.pop();
            }
        };

        // 与会话循环同款：100ms 块喂 VAD，静音断句即出终稿
        for chunk in samples.chunks(1600) {
            vad.accept_waveform(chunk);
            if vad.detected() {
                drain(&mut vad, &mut rows);
            }
        }
        vad.flush();
        drain(&mut vad, &mut rows);
    }
    let content_ms = samples.len() as f64 / 16.0;
    let process_ms = t_process.elapsed().as_millis();
    println!(
        "处理耗时: {process_ms} ms（内容 {:.1}s，等效 {:.1}× 实时）",
        content_ms / 1000.0,
        if process_ms == 0 { f64::INFINITY } else { content_ms / process_ms as f64 }
    );

    if engines.len() == 1 && use_stream {
        println!("识别到 {} 句:", rows.len());
        for (i, row) in rows.iter().enumerate() {
            if let Some((s, _)) = row.first() {
                if !s.is_empty() {
                    println!("[{}] {}", i + 1, s);
                }
            }
        }
        // 对照：整段一次性转写（不经 VAD 断句），用于判断是否 VAD 漏切
        let whole = transcribe_buffer(recognizer.as_ref().expect("流式引擎未加载"), samples);
        println!("—— 整段对照 ——");
        println!("{}", whole.trim());
        return;
    }

    // 多引擎对比：每段各引擎一列并排 + 末尾统计
    println!("\n—— 多引擎对比（每段并排各引擎终稿）——");
    let mut chars_total = vec![0usize; engines.len()];
    let mut ms_total = vec![0u128; engines.len()];
    for (i, row) in rows.iter().enumerate() {
        for (j, (text, ms)) in row.iter().enumerate() {
            let prefix = if j == 0 { format!("[{}]", i + 1) } else { " ".repeat(4) };
            println!("{} {}({}ms): {}", prefix, engines[j].label(), ms, text);
        }
        for (j, (text, ms)) in row.iter().enumerate() {
            chars_total[j] += char_count(text);
            ms_total[j] += *ms;
        }
    }
    println!("\n—— 统计 ——");
    println!("段数: {}", rows.len());
    println!("内容时长: {:.1} s", content_ms / 1000.0);
    for (j, &e) in engines.iter().enumerate() {
        let rtf = ms_total[j] as f64 / content_ms;
        let speedup = if ms_total[j] == 0 { f64::INFINITY } else { content_ms / ms_total[j] as f64 };
        println!(
            "{}: 总字数 {}，解码总耗时 {} ms（平均 {}/段），RTF {:.3}（{:.1}× 实时）",
            e.label(),
            chars_total[j],
            ms_total[j],
            if rows.is_empty() { 0 } else { ms_total[j] / rows.len() as u128 },
            rtf,
            speedup,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_cli_requires_path() {
        assert!(parse_cli(&args(&[])).is_err());
        let e = parse_cli(&args(&["--compare"])).unwrap_err();
        assert!(e.contains("缺少音频文件路径"));
    }

    #[test]
    fn parse_cli_plain_path() {
        let cli = parse_cli(&args(&["a.wav"])).unwrap();
        assert_eq!(cli.path, PathBuf::from("a.wav"));
        assert!(!cli.compare);
        assert!(!cli.agc);
        assert_eq!(cli.max_seconds, None);
        assert_eq!(cli.engines, None);
        assert_eq!(cli.threads, 4);
    }

    #[test]
    fn parse_cli_agc_flag_combines_with_others() {
        // 单独 --agc
        let cli = parse_cli(&args(&["--agc", "a.wav"])).unwrap();
        assert!(cli.agc);
        assert!(!cli.compare);
        // 与 --compare / --vad-threshold / --max-seconds 任意组合、顺序不限
        let cli = parse_cli(&args(&["--compare", "--vad-threshold", "0.35", "--agc", "a.wav"])).unwrap();
        assert!(cli.agc && cli.compare);
        assert_eq!(cli.vad_threshold, 0.35);
        let cli = parse_cli(&args(&["a.wav", "--agc", "--max-seconds", "60", "--compare"])).unwrap();
        assert!(cli.agc && cli.compare);
        assert_eq!(cli.max_seconds, Some(60.0));
    }

    #[test]
    fn parse_cli_flags_in_any_order() {
        let cli = parse_cli(&args(&["--compare", "--max-seconds", "90", "a.wav"])).unwrap();
        assert!(cli.compare);
        assert_eq!(cli.max_seconds, Some(90.0));
        assert_eq!(cli.path, PathBuf::from("a.wav"));
        // 顺序不限
        let cli = parse_cli(&args(&["a.wav", "--max-seconds", "12.5", "--compare"])).unwrap();
        assert!(cli.compare);
        assert_eq!(cli.max_seconds, Some(12.5));
    }

    #[test]
    fn parse_cli_max_seconds_validation() {
        // 缺数值
        assert!(parse_cli(&args(&["--max-seconds"])).is_err());
        assert!(parse_cli(&args(&["--max-seconds", "a.wav"])).is_err());
        // 非数字 / 非正数 / NaN
        assert!(parse_cli(&args(&["--max-seconds", "abc", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--max-seconds", "0", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--max-seconds", "-3", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--max-seconds", "NaN", "a.wav"])).is_err());
        // 未知标志 / 多余位置参数
        assert!(parse_cli(&args(&["--wat", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["a.wav", "b.wav"])).is_err());
    }

    #[test]
    fn parse_cli_vad_threshold_validation() {
        // 默认 0.5（与会话「标准」灵敏度一致）
        assert_eq!(parse_cli(&args(&["a.wav"])).unwrap().vad_threshold, 0.5);
        // 高灵敏度 0.35（与会话设置「高灵敏度」一致）
        let cli = parse_cli(&args(&["--vad-threshold", "0.35", "a.wav"])).unwrap();
        assert_eq!(cli.vad_threshold, 0.35);
        // 缺数值 / 非数字 / 越界
        assert!(parse_cli(&args(&["--vad-threshold"])).is_err());
        assert!(parse_cli(&args(&["--vad-threshold", "abc", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--vad-threshold", "0", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--vad-threshold", "1", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--vad-threshold", "-0.1", "a.wav"])).is_err());
    }

    #[test]
    fn parse_cli_engines_selection() {
        // 三引擎全量（含空格容错）
        let cli = parse_cli(&args(&["--engines", "stream,sense,fire-red-ctc", "a.wav"])).unwrap();
        assert_eq!(
            cli.engines,
            Some(vec![Engine::Stream, Engine::SenseVoice, Engine::FireRedCtc])
        );
        // 只跑离线两引擎（不要流式）
        let cli = parse_cli(&args(&["--engines", "sense,fire-red-ctc", "a.wav"])).unwrap();
        assert_eq!(cli.engines, Some(vec![Engine::SenseVoice, Engine::FireRedCtc]));
        // 别名
        let cli = parse_cli(&args(&["--engines", "sense-voice, firered", "a.wav"])).unwrap();
        assert_eq!(cli.engines, Some(vec![Engine::SenseVoice, Engine::FireRedCtc]));
        // 未指定 → None（走 compare/默认解析）
        assert_eq!(parse_cli(&args(&["a.wav"])).unwrap().engines, None);
        // 校验：未知引擎 / 重复 / 空列表 / 缺参数
        assert!(parse_cli(&args(&["--engines", "wat", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--engines", "stream,stream", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--engines", "", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--engines"])).is_err());
    }

    #[test]
    fn parse_cli_threads_validation() {
        // 默认 4；显式 1（对标论文单线程 RTF）
        assert_eq!(parse_cli(&args(&["a.wav"])).unwrap().threads, 4);
        assert_eq!(parse_cli(&args(&["--threads", "1", "a.wav"])).unwrap().threads, 1);
        // 缺数值 / 非整数 / 越界
        assert!(parse_cli(&args(&["--threads"])).is_err());
        assert!(parse_cli(&args(&["--threads", "abc", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--threads", "0", "a.wav"])).is_err());
        assert!(parse_cli(&args(&["--threads", "17", "a.wav"])).is_err());
    }

    #[test]
    fn resolve_engines_default_compare_and_explicit() {
        // 默认：仅流式（现有行为不变）
        assert_eq!(resolve_engines(false, &None), vec![Engine::Stream]);
        // --compare：流式 + SenseVoice（现有行为不变）
        assert_eq!(
            resolve_engines(true, &None),
            vec![Engine::Stream, Engine::SenseVoice]
        );
        // --engines 显式覆盖（含 --compare 同给时）
        let explicit = Some(vec![Engine::Stream, Engine::SenseVoice, Engine::FireRedCtc]);
        assert_eq!(resolve_engines(false, &explicit), explicit.clone().unwrap());
        assert_eq!(resolve_engines(true, &explicit), explicit.unwrap());
    }

    #[test]
    fn parse_engines_edge_cases() {
        // 单引擎
        assert_eq!(parse_engines("fire-red-ctc").unwrap(), vec![Engine::FireRedCtc]);
        // 未知名 / 重复 / 空
        assert!(parse_engines("gpt").is_err());
        assert!(parse_engines("sense,sense").is_err());
        assert!(parse_engines("").is_err());
        assert!(parse_engines(" , ").is_err());
    }

    #[test]
    fn engine_ids_round_trip() {
        for e in [Engine::Stream, Engine::SenseVoice, Engine::FireRedCtc] {
            assert_eq!(Engine::from_id(e.id()), Some(e));
        }
        assert_eq!(Engine::from_id("unknown"), None);
    }

    #[test]
    fn clamp_samples_truncates_to_limit() {
        let samples: Vec<f32> = (0..16_000 * 5).map(|i| i as f32).collect();
        // 90 秒上限对 5 秒素材：原样
        assert_eq!(clamp_samples(&samples, 16_000, Some(90.0)).len(), 80_000);
        // 截前 2 秒
        assert_eq!(clamp_samples(&samples, 16_000, Some(2.0)).len(), 32_000);
        assert_eq!(clamp_samples(&samples, 16_000, Some(2.0))[0], 0.0);
        // None：全量
        assert_eq!(clamp_samples(&samples, 16_000, None).len(), 80_000);
    }

    #[test]
    fn char_count_ignores_whitespace_only() {
        assert_eq!(char_count("你好，世界。"), 6);
        assert_eq!(char_count("hello world"), 10);
        assert_eq!(char_count("  \n\t "), 0);
        assert_eq!(char_count(""), 0);
    }
}
