//! 用应用的完整识别管线转写任意受支持的音频文件：
//! 解码（wav/mp3/flac/m4a/aac）→ 重采样 16k 单声道 → Silero VAD 断句 → 终稿。
//! 与会话内识别行为一致（含收尾 flush），但不走规则引擎/UI，用于命令行快速检测素材。
//!
//! 用法：
//! ```text
//! cargo run --release --example transcribe_any -- [--compare] [--agc] [--max-seconds N] [--vad-threshold T] <音频文件路径>
//! ```
//!
//! - `--compare` 双引擎对比：每个 VAD 段并排打印「流式终稿」（Paraformer）
//!   与「SenseVoice 终稿」（离线高精引擎），末尾统计两版总字数与耗时——
//!   量化双引擎改进的核心工具
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
use sherpa_onnx::{OfflineRecognizer, OnlineRecognizer, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::path::PathBuf;
use std::time::Instant;

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
}

/// 解析参数：`[--compare] [--agc] [--max-seconds N] [--vad-threshold T] <path>`；标志与路径顺序不限
pub fn parse_cli(args: &[String]) -> Result<Cli, String> {
    let mut path: Option<PathBuf> = None;
    let mut compare = false;
    let mut agc = false;
    let mut max_seconds: Option<f64> = None;
    let mut vad_threshold = 0.5f32;
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
            eprintln!("用法: transcribe_any [--compare] [--agc] [--max-seconds N] [--vad-threshold T] <音频文件路径>");
            std::process::exit(1);
        }
    };

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
    let recognizer =
        OnlineRecognizer::create(&build_asr_config(&models_dir, None)).expect("ASR 初始化失败");

    // --compare 需要 SenseVoice 模型（可选组件）
    let offline = if cli.compare {
        if !sense_voice_ready(&models_dir) {
            eprintln!(
                "--compare 需要 SenseVoice 模型（models/{}/model.int8.onnx + tokens.txt），请先下载",
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

    // 每段两版终稿 + 各自解码耗时（本块 = 会话「极速」档的等价消费路径：
    // 不等任何时刻表，VAD/识别跑多快算多快，计时即极速会话的处理耗时）
    let mut rows: Vec<(String, u128, Option<(String, u128)>)> = Vec::new();
    let t_process = Instant::now();
    {
        let drain = |vad: &mut VoiceActivityDetector, out: &mut Vec<(String, u128, Option<(String, u128)>)>| {
            while !vad.is_empty() {
                if let Some(seg) = vad.front() {
                    let t0 = Instant::now();
                    let text = transcribe_buffer(&recognizer, seg.samples());
                    let stream_ms = t0.elapsed().as_millis();
                    let sense = offline.as_ref().map(|r| {
                        let t0 = Instant::now();
                        let text = transcribe_offline(r, seg.samples());
                        (text, t0.elapsed().as_millis())
                    });
                    out.push((text.trim().to_string(), stream_ms, sense));
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

    if !cli.compare {
        println!("识别到 {} 句:", rows.len());
        for (i, (s, _, _)) in rows.iter().enumerate() {
            if !s.is_empty() {
                println!("[{}] {}", i + 1, s);
            }
        }
        // 对照：整段一次性转写（不经 VAD 断句），用于判断是否 VAD 漏切
        let whole = transcribe_buffer(&recognizer, samples);
        println!("—— 整段对照 ——");
        println!("{}", whole.trim());
        return;
    }

    // 双引擎对比：每段两列并排 + 末尾统计
    println!("\n—— 双引擎对比（每段：流式终稿 vs SenseVoice 终稿）——");
    let mut stream_chars = 0usize;
    let mut sense_chars = 0usize;
    let mut stream_ms_total = 0u128;
    let mut sense_ms_total = 0u128;
    for (i, (s, sms, sense)) in rows.iter().enumerate() {
        let sv = sense.as_ref().map(|(t, _)| t.as_str()).unwrap_or("");
        println!("[{}] 流式({}ms): {}", i + 1, sms, s);
        if let Some((_, ms)) = sense {
            println!("    高精({}ms): {}", ms, sv);
        }
        stream_chars += char_count(s);
        sense_chars += char_count(sv);
        stream_ms_total += *sms;
        if let Some((_, ms)) = sense {
            sense_ms_total += ms;
        }
    }
    println!("\n—— 统计 ——");
    println!("段数: {}", rows.len());
    println!("流式终稿总字数: {}", stream_chars);
    println!("SenseVoice 终稿总字数: {}", sense_chars);
    println!(
        "流式解码总耗时: {} ms（平均 {}/段）",
        stream_ms_total,
        if rows.is_empty() { 0 } else { stream_ms_total / rows.len() as u128 }
    );
    println!(
        "SenseVoice 解码总耗时: {} ms（平均 {}/段）",
        sense_ms_total,
        if rows.is_empty() { 0 } else { sense_ms_total / rows.len() as u128 }
    );
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
