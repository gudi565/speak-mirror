use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

// ---------------------------------------------------------------------------
// 录音自动增益（AGC）：文件模式「录音增强」的核心
// ---------------------------------------------------------------------------

/// AGC 帧长：100ms @ 16k（与 examples/transcribe_any 的音量诊断同口径）
const AGC_FRAME_SAMPLES: usize = 1_600;
/// 过静判定阈值：帧 RMS p95 低于该值视为「手机远距离录音过静」，需要放大
pub const AGC_QUIET_P95: f32 = 0.08;
/// 放大目标：把有效语音电平（帧 RMS p95）拉到该值附近
pub const AGC_TARGET_P95: f32 = 0.15;
/// 增益上限：过高增益会同时放大底噪，8 倍封顶
pub const AGC_MAX_GAIN: f32 = 8.0;

/// 100ms 帧 RMS 的 p95（有效语音电平估计；静音占比再高也有 5% 帧是说话）
pub fn frame_rms_p95(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
    let mut frame_rms: Vec<f32> = samples.chunks(AGC_FRAME_SAMPLES).map(rms).collect();
    frame_rms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    frame_rms[(frame_rms.len() as f64 * 0.95) as usize % frame_rms.len()]
}

/// 录音自动增益（纯函数）：先减全段均值消除直流偏置，再按 100ms 帧 RMS
/// 的 p95 估计有效语音电平——p95 < 0.08（过静）时整段线性放大至目标
/// p95 ≈ 0.15（增益上限 8×，放大后钳位 ±1.0 防溢出）；达标则不放大
/// （增益 1.0）。返回 (处理后样本, 实际增益)。
///
/// 典型场景：手机远距离录音电平过低 → VAD 漏切轻尾音、识别引擎喂入
/// 电平不足 → 识别率差。只用于文件模式整段处理（离线分析），麦克风
/// 实时路径不启用（实时增益有突变/回声风险）。
pub fn auto_gain(samples: &[f32]) -> (Vec<f32>, f32) {
    if samples.is_empty() {
        return (Vec::new(), 1.0);
    }
    // 直流偏置消除：减去全段均值（过静录音常有直流分量，白白吃掉动态范围）
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    let centered: Vec<f32> = samples.iter().map(|&s| s - mean).collect();
    let p95 = frame_rms_p95(&centered);
    // 达标或全静音（无参考电平，防除 0）→ 不放大
    if p95 >= AGC_QUIET_P95 || p95 <= 0.0 {
        return (centered, 1.0);
    }
    let gain = (AGC_TARGET_P95 / p95).min(AGC_MAX_GAIN);
    let amplified: Vec<f32> = centered.iter().map(|&s| (s * gain).clamp(-1.0, 1.0)).collect();
    (amplified, gain)
}

/// 混成立体声为单声道 + 线性插值重采样到 16kHz
pub fn resample_to_16k_mono(samples: &[f32], src_rate: u32, channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let mono: Vec<f32> = samples
        .chunks(ch)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    if src_rate == 16_000 {
        return mono;
    }
    let ratio = src_rate as f64 / 16_000.0;
    let out_len = (mono.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = mono[idx.min(mono.len() - 1)];
            let b = mono[(idx + 1).min(mono.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// f32 [-1,1] → i16 PCM（钳位 + 四舍五入；与会话录音回放路径配对使用）
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16)
        .collect()
}

/// 单声道 PCM16 wav 完整字节（44 字节头 + 数据）。
/// 供会话录音落盘 `appdata/sessions/audio/<时间戳>.wav` 使用；头字段
/// 与 decode.rs 的 RIFF 解析器互补（round-trip 测试互相验证）。
pub fn wav_pcm16_bytes(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let byte_rate = sample_rate * 2; // 单声道 × 16 位
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes()); // RIFF 块总长 - 8
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt 块长
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // 位深
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    out.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
    out
}

/// 低延迟采集缓冲的目标时长：100ms。取舍说明——cpal 在未显式设置
/// buffer_size 时由设备/驱动决定（WASAPI 共享模式常见 10ms 事件周期，
/// 但部分驱动会给出大得多的默认缓冲，叠加主循环 100ms 的 recv 节拍后
/// 显著放大字幕跟手延迟）；显式 Fixed 到 100ms 时长后回调最多滞后
/// 100ms，与主循环（recv_timeout 100ms）消费粒度对齐，再小无收益、
/// 只会推高回调频率与 CPU。帧数按设备原生采样率折算等价 100ms 时长，
/// 并夹到设备支持的 [min, max] 内（越界会被部分驱动直接拒绝建流）。
pub const CAPTURE_BUFFER_MS: u32 = 100;

/// 采集缓冲帧数（纯函数可测）：100ms 时长的帧数，夹在设备支持范围内。
/// 注意 cpal 的 buffer_size 单位是**帧**（每帧含全部声道），与采样率同纲。
pub fn low_latency_buffer_frames(sample_rate: u32, min_frames: u32, max_frames: u32) -> u32 {
    (sample_rate * CAPTURE_BUFFER_MS / 1000).clamp(min_frames, max_frames)
}

/// 打开默认输入设备，回调把原始 f32 帧推入 channel。
/// 显式配置低延迟缓冲（见 CAPTURE_BUFFER_MS）——实时字幕跟手的关键一环。
pub fn start_capture() -> Result<AudioCapture, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or("找不到可用的麦克风设备")?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("读取麦克风配置失败: {e}"))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let sample_format = supported.sample_format();
    // 先取出设备支持的缓冲范围再 move（SupportedStreamConfig 无 Copy）
    let buffer_range = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
        cpal::SupportedBufferSize::Unknown => None,
    };
    let mut config: cpal::StreamConfig = supported.into();
    // 显式低延迟缓冲：缺省 buffer_size 由驱动决定，可能远大于 100ms；
    // 夹到设备支持范围后设为 Fixed（见 CAPTURE_BUFFER_MS 注释的取舍）
    if let Some((min, max)) = buffer_range {
        config.buffer_size =
            cpal::BufferSize::Fixed(low_latency_buffer_frames(sample_rate, min, max));
    }

    let (tx, rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let err_fn = |e| eprintln!("audio capture error: {e}");

    let stream = match sample_format {
        SampleFormat::F32 => device
            .build_input_stream(
                &config,
                move |data: &[f32], _| {
                    let _ = tx.send(data.to_vec());
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("打开麦克风失败（请检查系统录音权限）: {e}"))?,
        SampleFormat::I16 => device
            .build_input_stream(
                &config,
                move |data: &[i16], _| {
                    let converted: Vec<f32> = data
                        .iter()
                        .map(|&s| s as f32 / i16::MAX as f32)
                        .collect();
                    let _ = tx.send(converted);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("打开麦克风失败（请检查系统录音权限）: {e}"))?,
        SampleFormat::U16 => device
            .build_input_stream(
                &config,
                move |data: &[u16], _| {
                    let converted: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 - 32768.0) / 32768.0)
                        .collect();
                    let _ = tx.send(converted);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("打开麦克风失败（请检查系统录音权限）: {e}"))?,
        _ => {
            return Err(format!(
                "不支持的麦克风采样格式: {:?}",
                sample_format
            ))
        }
    };

    stream.play().map_err(|e| format!("启动音频流失败: {e}"))?;
    Ok(AudioCapture {
        stream,
        rx,
        sample_rate,
        channels,
    })
}

pub struct AudioCapture {
    pub stream: cpal::Stream,
    pub rx: std::sync::mpsc::Receiver<Vec<f32>>,
    pub sample_rate: u32,
    pub channels: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_latency_buffer_frames_is_100ms_equivalent() {
        // 16k → 1600 帧；48k → 4800 帧：均为 100ms 时长（帧数随原生采样率折算）
        assert_eq!(low_latency_buffer_frames(16_000, 1, u32::MAX), 1_600);
        assert_eq!(low_latency_buffer_frames(48_000, 1, u32::MAX), 4_800);
        assert_eq!(low_latency_buffer_frames(44_100, 1, u32::MAX), 4_410);
    }

    #[test]
    fn low_latency_buffer_frames_clamps_to_device_range() {
        // 设备最小缓冲更大：取设备下限（越界的 Fixed 会被部分驱动拒绝建流）
        assert_eq!(low_latency_buffer_frames(16_000, 2_048, u32::MAX), 2_048);
        // 设备最大缓冲更小：取设备上限
        assert_eq!(low_latency_buffer_frames(48_000, 1, 1_000), 1_000);
        // 极端采样率也不为 0
        assert_eq!(low_latency_buffer_frames(8_000, 1, u32::MAX), 800);
    }

    #[test]
    fn stereo_to_mono_averages_channels() {
        // 左右声道各一个采样对
        let mono = resample_to_16k_mono(&[1.0, 0.0, 0.5, 0.5], 16_000, 2);
        assert_eq!(mono, vec![0.5, 0.5]);
    }

    #[test]
    fn downsamples_48k_to_16k_by_third() {
        let input: Vec<f32> = (0..480).map(|i| i as f32).collect();
        let out = resample_to_16k_mono(&input, 48_000, 1);
        assert_eq!(out.len(), 160);
        assert_eq!(out[0], 0.0);
    }

    #[test]
    fn passthrough_when_already_16k_mono() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_to_16k_mono(&input, 16_000, 1), input);
    }

    #[test]
    fn f32_to_i16_clamps_and_rounds() {
        assert_eq!(f32_to_i16(&[0.0]), vec![0]);
        // 0.5 × 32767 = 16383.5 → 四舍五入 16384
        assert_eq!(f32_to_i16(&[0.5]), vec![16384]);
        // 超界钳位到 ±32767
        assert_eq!(f32_to_i16(&[2.0, -2.0]), vec![32767, -32767]);
        assert_eq!(f32_to_i16(&[-0.5]), vec![-16384]);
    }

    #[test]
    fn auto_gain_amplifies_quiet_recording_to_target() {
        // 方波幅度 0.05 → 每帧 RMS 恰为 0.05（< 0.08 过静）→ 增益 0.15/0.05 = 3.0
        let quiet: Vec<f32> = (0..16_000).map(|i| if i % 2 == 0 { 0.05 } else { -0.05 }).collect();
        let (out, gain) = auto_gain(&quiet);
        assert!((gain - 3.0).abs() < 1e-3, "gain {gain}"); // f32 帧内求和有微小舍入
        // 放大后幅度 0.05 × 3 ≈ 0.15（帧 RMS p95 达标 ≈ 0.15）
        for s in &out {
            assert!((s.abs() - 0.15).abs() < 1e-4, "{s}");
        }
        assert!((frame_rms_p95(&out) - AGC_TARGET_P95).abs() < 1e-3);
    }

    #[test]
    fn auto_gain_leaves_loud_recording_untouched() {
        // 达标电平（p95 ≈ 0.2 ≥ 0.08，零均值）：增益 1.0 且样本原样返回
        let loud: Vec<f32> = (0..16_000).map(|i| if i % 2 == 0 { 0.2 } else { -0.2 }).collect();
        let (out, gain) = auto_gain(&loud);
        assert_eq!(gain, 1.0);
        assert_eq!(out, loud);
    }

    #[test]
    fn auto_gain_caps_gain_at_8x_and_clamps_samples() {
        // 过静到需要 150× → 增益封顶 8.0，不再追求达标（防把底噪放大到失控）
        let ultra_quiet: Vec<f32> = (0..16_000).map(|i| if i % 2 == 0 { 0.001 } else { -0.001 }).collect();
        let (out, gain) = auto_gain(&ultra_quiet);
        assert_eq!(gain, AGC_MAX_GAIN);
        for s in &out {
            assert!((s.abs() - 0.008).abs() < 1e-6, "{s}");
        }
        // 防溢出钳位：安静的整段 + 个别高峰样本，放大后高峰被夹到 ±1.0。
        // 用 100 帧（10 秒）：p95 取第 95 位帧，高峰所在帧只占少数不影响判定
        let mut spiky: Vec<f32> = vec![0.0; 100 * AGC_FRAME_SAMPLES];
        for (i, s) in spiky.iter_mut().enumerate() {
            *s = if i % 2 == 0 { 0.05 } else { -0.05 };
        }
        spiky[100] = 0.9; // 高峰在安静段里：p95 仍由 0.05 的帧决定 → 增益 3.0
        let (out, gain) = auto_gain(&spiky);
        assert!((gain - 3.0).abs() < 1e-3, "gain {gain}");
        assert_eq!(out[100], 1.0); // 0.9 × 3 = 2.7 → 钳位 1.0
        assert!((out[101] + 0.15).abs() < 1e-3); // 其余样本正常放大（±直流偏置消除的微小残差）
    }

    #[test]
    fn auto_gain_removes_dc_offset() {
        // 带直流偏置的达标录音：增益 1.0（不放大），但偏置被减掉（均值 ≈ 0；
        // f32 顺序求和有微小残差，容忍 1e-4——原偏置 0.1 的千分之一以下）
        let biased: Vec<f32> = (0..16_000).map(|i| if i % 2 == 0 { 0.3 } else { -0.1 }).collect();
        let (out, gain) = auto_gain(&biased);
        assert_eq!(gain, 1.0);
        let mean = out.iter().sum::<f32>() / out.len() as f32;
        assert!(mean.abs() < 1e-4, "mean {mean}");
        // 交流波形保留：峰 0.3 → 0.2、谷 -0.1 → -0.2（围绕新均值 0）
        assert!((out[0] - 0.2).abs() < 1e-4);
        assert!((out[1] + 0.2).abs() < 1e-4);
        // 过静 + 偏置：增益路径同样先消除偏置（不会把直流一起放大）
        let quiet_biased: Vec<f32> = (0..16_000).map(|i| if i % 2 == 0 { 0.1 } else { -0.02 }).collect();
        let (out, gain) = auto_gain(&quiet_biased);
        assert!(gain > 1.0);
        let mean = out.iter().sum::<f32>() / out.len() as f32;
        assert!(mean.abs() < 1e-4, "mean {mean}");
    }

    #[test]
    fn auto_gain_reports_gain_value_and_handles_edges() {
        // p95 = 0.06 → 增益 = 0.15/0.06 = 2.5（返回值可直接用于日志/UI 展示）
        let quiet: Vec<f32> = (0..16_000).map(|i| if i % 2 == 0 { 0.06 } else { -0.06 }).collect();
        let (_, gain) = auto_gain(&quiet);
        assert!((gain - 2.5).abs() < 1e-3, "gain {gain}");
        // 空段：不 panic，返回空 + 增益 1.0
        let (out, gain) = auto_gain(&[]);
        assert!(out.is_empty());
        assert_eq!(gain, 1.0);
        // 全零段：无参考电平 → 不放大（防除 0）
        let (out, gain) = auto_gain(&[0.0; 16_000]);
        assert_eq!(gain, 1.0);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn frame_rms_p95_matches_diagnostic_definition() {
        // 与 transcribe_any 音量诊断同口径：10 帧（1 秒）里只有最后 1 帧有信号
        // → 排序后 p95 取第 9 位（下标 9）= 信号帧的 RMS
        let mut samples = vec![0.0f32; 9 * AGC_FRAME_SAMPLES];
        samples.extend(std::iter::repeat(0.2).take(AGC_FRAME_SAMPLES / 2));
        samples.extend(std::iter::repeat(-0.2).take(AGC_FRAME_SAMPLES / 2));
        assert!((frame_rms_p95(&samples) - 0.2).abs() < 1e-4);
        assert_eq!(frame_rms_p95(&[]), 0.0);
    }

    #[test]
    fn wav_pcm16_header_fields() {
        let bytes = wav_pcm16_bytes(&[0, 1, -1], 16_000);
        assert_eq!(bytes.len(), 44 + 6);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        // fmt 块 16 字节：PCM / 单声道 / 16k / byte_rate 32000 / align 2 / 16 位
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(bytes[28..32].try_into().unwrap()), 32_000);
        assert_eq!(u16::from_le_bytes(bytes[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
        // RIFF 总长 = 文件长 - 8
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len() - 8);
    }

    #[test]
    fn wav_pcm16_roundtrip_through_decoder() {
        // 写出 → decode::decode_audio_file 读回：采样率/声道/样本值一致
        let i16_samples: Vec<i16> = vec![0, 16384, -16384, 32767, -32767];
        let bytes = wav_pcm16_bytes(&i16_samples, 16_000);
        let dir = std::env::temp_dir().join(format!("sm-audio-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.wav");
        std::fs::write(&path, &bytes).unwrap();
        let decoded = crate::decode::decode_audio_file(&path).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples.len(), i16_samples.len());
        for (a, b) in decoded.samples.iter().zip(f32_to_i16(&decoded.samples)) {
            // f32 往返（/32767 再 ×32767）允许 1 LSB 误差
            assert!((a - b as f32 / 32767.0).abs() <= 2.0 / 32767.0);
        }
        assert_eq!(f32_to_i16(&decoded.samples), i16_samples);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
