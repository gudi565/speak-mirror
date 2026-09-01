//! 音频文件解码（「从文件练习」：无麦克风走查 + 口播创作者分析已录素材）。
//!
//! - wav：直接解析 RIFF 字节（PCM 8/16/24/32 位与 IEEE float 32/64 位、常见采样率、多声道），
//!   不引入解码依赖；采样率/声道保持原样，由会话线程按块复用 `resample_to_16k_mono`
//!   （与麦克风完全相同的重采样路径）。
//! - mp3 / flac / m4a / aac：symphonia（纯 Rust）；aac/m4a 尽力而为，失败给中文可操作错误。
//! - 其他扩展名：明确报「暂不支持该格式，请转为 wav/mp3」。
//!
//! 所有错误信息中文且包含格式与原因。

// as_chunks 建议（clippy::chunks_exact_to_as_chunks）在按位宽切循环里可读性更差，保持 chunks_exact
#![allow(clippy::chunks_exact_to_as_chunks)]

use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::TimeBase;

/// 解码结果：交错 f32 样本 + 原生采样率/声道（16k 单声道由会话循环负责重采样）
#[derive(Debug)]
pub struct DecodedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl DecodedAudio {
    /// 音频内容时长（毫秒）：帧数 × 1000 / 采样率
    pub fn duration_ms(&self) -> u64 {
        let channels = self.channels.max(1) as u64;
        let frames = self.samples.len() as u64 / channels;
        frames * 1000 / self.sample_rate.max(1) as u64
    }
}

/// wav 走自研解析；其余支持的扩展名交给 symphonia
const SYMPHONIA_EXTS: &[&str] = &["mp3", "flac", "m4a", "aac", "mp4"];

/// 读取并解码一个本地音频文件（存在性/格式/解码错误均为中文提示）
pub fn decode_audio_file(path: &Path) -> Result<DecodedAudio, String> {
    if !path.exists() {
        return Err(format!(
            "找不到音频文件：{}（文件可能已被移动、删除或重命名）",
            path.display()
        ));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "wav" | "wave" => decode_wav_file(path),
        e if SYMPHONIA_EXTS.contains(&e) => decode_symphonia(path, &ext),
        "" => Err("暂不支持该格式（文件没有扩展名），请将音频转为 wav 或 mp3 后再试".to_string()),
        other => Err(format!(
            "暂不支持该格式（.{other}），请将音频转为 wav 或 mp3 后再试"
        )),
    }
}

/// 只探测时长（毫秒），不解码全部样本：wav 读头即可，其余格式扫包不解码
pub fn probe_duration_ms(path: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Err(format!(
            "找不到音频文件：{}（文件可能已被移动、删除或重命名）",
            path.display()
        ));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "wav" | "wave" => {
            let bytes = std::fs::read(path).map_err(|e| format!("读取文件失败：{e}"))?;
            let header = parse_wav_header(&bytes)?;
            Ok(header.duration_ms())
        }
        e if SYMPHONIA_EXTS.contains(&e) => probe_symphonia_duration(path, &ext),
        "" => Err("暂不支持该格式（文件没有扩展名），请将音频转为 wav 或 mp3 后再试".to_string()),
        other => Err(format!(
            "暂不支持该格式（.{other}），请将音频转为 wav 或 mp3 后再试"
        )),
    }
}

// ---------------------------------------------------------------------------
// wav（RIFF）解析
// ---------------------------------------------------------------------------

/// fmt 块要点（data 的定位与长度一并带出）
#[derive(Debug)]
struct WavHeader {
    audio_format: u16, // 1 = PCM，3 = IEEE float，0xFFFE = EXTENSIBLE（看子格式）
    channels: u16,
    sample_rate: u32,
    bits: u16,
    bytes_per_sample: usize,
    data_offset: usize,
    data_len: usize,
}

impl WavHeader {
    /// data 块时长（毫秒）
    fn duration_ms(&self) -> u64 {
        let block = self.channels.max(1) as u64 * self.bytes_per_sample as u64;
        if block == 0 || self.sample_rate == 0 {
            return 0;
        }
        let frames = self.data_len as u64 / block;
        frames * 1000 / self.sample_rate as u64
    }
}

fn wav_err(reason: &str) -> String {
    format!("wav 文件解析失败：{reason}；文件可能损坏，可尝试用其他工具重新导出为 wav 或 mp3")
}

fn read_u16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn read_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// 解析 RIFF 头：RIFF/WAVE 魔数 → 遍历块找 fmt 与 data（跳过 LIST 等未知块，奇长补齐）
fn parse_wav_header(bytes: &[u8]) -> Result<WavHeader, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(wav_err("不是有效的 wav（RIFF/WAVE）文件"));
    }
    let mut fmt: Option<WavHeader> = None;
    let mut data: Option<(usize, usize)> = None;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = read_u32(bytes, pos + 4).ok_or_else(|| wav_err("块长度越界"))? as usize;
        let body = pos + 8;
        if body + size > bytes.len() {
            // 尾块被截断：data 取到文件末尾为止，其余块直接报错
            if id == b"data" {
                data = Some((body, bytes.len() - body));
                break;
            }
            return Err(wav_err("文件在块中间被截断"));
        }
        match id {
            b"fmt " => {
                if size < 16 {
                    return Err(wav_err("fmt 块过短"));
                }
                let mut audio_format =
                    read_u16(bytes, body).ok_or_else(|| wav_err("fmt 块损坏"))?;
                let channels = read_u16(bytes, body + 2).ok_or_else(|| wav_err("fmt 块损坏"))?;
                let sample_rate =
                    read_u32(bytes, body + 4).ok_or_else(|| wav_err("fmt 块损坏"))?;
                let bits = read_u16(bytes, body + 14).ok_or_else(|| wav_err("fmt 块损坏"))?;
                // WAVE_FORMAT_EXTENSIBLE：真正格式码在 SubFormat GUID 前 4 字节（小端 u32）
                if audio_format == 0xFFFE && size >= 40 {
                    audio_format = read_u32(bytes, body + 24)
                        .map(|v| v as u16)
                        .ok_or_else(|| wav_err("扩展 fmt 块损坏"))?;
                }
                if channels == 0 || sample_rate == 0 || bits == 0 || bits % 8 != 0 {
                    return Err(wav_err(&format!(
                        "无效参数（声道 {channels}、采样率 {sample_rate}、位深 {bits}）"
                    )));
                }
                fmt = Some(WavHeader {
                    audio_format,
                    channels,
                    sample_rate,
                    bits,
                    bytes_per_sample: bits as usize / 8,
                    data_offset: 0,
                    data_len: 0,
                });
            }
            b"data" => data = Some((body, size)),
            _ => {}
        }
        // RIFF 块按 2 字节对齐：奇数长度补一个填充字节
        pos = body + size + (size & 1);
    }
    let mut header = fmt.ok_or_else(|| wav_err("缺少 fmt 块"))?;
    let (data_offset, data_len) = data.ok_or_else(|| wav_err("缺少 data 块"))?;
    header.data_offset = data_offset;
    header.data_len = data_len;
    Ok(header)
}

/// 把 data 原始字节按位深/编码转成交错 f32
fn wav_samples_to_f32(header: &WavHeader, data: &[u8]) -> Result<Vec<f32>, String> {
    let bps = header.bytes_per_sample;
    let ch = header.channels.max(1) as usize;
    // 丢弃尾部不完整帧
    let usable = data.len() / (bps * ch) * (bps * ch);
    let data = &data[..usable];
    let samples: Vec<f32> = match (header.audio_format, header.bits) {
        (1, 8) => data
            .iter()
            .map(|&s| (s as f32 - 128.0) / 128.0)
            .collect(),
        (1, 16) => data
            .chunks_exact(2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / i16::MAX as f32)
            .collect(),
        (1, 24) => data
            .chunks_exact(3)
            .map(|s| {
                let v = ((s[2] as i32) << 24 | (s[1] as i32) << 16 | (s[0] as i32) << 8) >> 8;
                v as f32 / 8_388_608.0
            })
            .collect(),
        (1, 32) => data
            .chunks_exact(4)
            .map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / i32::MAX as f32)
            .collect(),
        (3, 32) => data
            .chunks_exact(4)
            .map(|s| f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
            .collect(),
        (3, 64) => data
            .chunks_exact(8)
            .map(|s| f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) as f32)
            .collect(),
        (f, bits) => {
            return Err(format!(
                "wav 文件使用了暂不支持的编码（格式 {f}、{bits} 位），请转存为 16 位 PCM wav 或 mp3"
            ))
        }
    };
    Ok(samples)
}

fn decode_wav_file(path: &Path) -> Result<DecodedAudio, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取 wav 文件失败：{e}"))?;
    let header = parse_wav_header(&bytes)?;
    let data = &bytes[header.data_offset..header.data_offset + header.data_len];
    let samples = wav_samples_to_f32(&header, data)?;
    Ok(DecodedAudio {
        samples,
        sample_rate: header.sample_rate,
        channels: header.channels,
    })
}

// ---------------------------------------------------------------------------
// symphonia（mp3 / flac / m4a / aac 尽力而为）
// ---------------------------------------------------------------------------

fn symphonia_unsupported(ext: &str, reason: &str) -> String {
    format!("暂不支持该音频（.{ext}：{reason}），请转为 wav 或 mp3 后再试")
}

/// symphonia 打开结果：format reader + decoder + 音轨 id + 采样率兜底值
type SymphoniaOpen = (
    Box<dyn symphonia::core::formats::FormatReader>,
    Box<dyn symphonia::core::codecs::Decoder>,
    u32,
    u32,
);

/// 打开容器并定位默认音轨（探测失败区分「不支持」与「损坏」）
fn open_symphonia(path: &Path, ext: &str) -> Result<SymphoniaOpen, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开文件失败：{e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(ext);
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| match e {
            SymError::Unsupported(reason) => symphonia_unsupported(ext, reason),
            other => format!(
                "音频读取失败（.{ext}）：{other}；文件可能损坏，请重试或转为 wav/mp3"
            ),
        })?;
    let format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| format!("文件里没有音频轨道（.{ext}），请选择一个音频文件"))?;
    let track_id = track.id;
    let fallback_rate = track.codec_params.sample_rate.unwrap_or(0);
    let decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| match e {
            SymError::Unsupported(reason) => symphonia_unsupported(ext, reason),
            other => format!(
                "音频解码器初始化失败（.{ext}）：{other}；请转为 wav 或 mp3 后再试"
            ),
        })?;
    Ok((format, decoder, track_id, fallback_rate))
}

fn decode_symphonia(path: &Path, ext: &str) -> Result<DecodedAudio, String> {
    let (mut format, mut decoder, track_id, fallback_rate) = open_symphonia(path, ext)?;
    let mut samples: Vec<f32> = Vec::new();
    let mut spec: Option<(u32, u16)> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // 容器读尽 = 正常到头
            Err(SymError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                return Err(format!(
                    "音频解码失败（.{ext}）：{e}；文件可能损坏，请重试或转为 wav/mp3"
                ))
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let frames = decoded.frames();
                if frames == 0 {
                    continue;
                }
                let s = decoded.spec();
                spec = Some((s.rate, s.channels.count() as u16));
                let mut buf = SampleBuffer::<f32>::new(frames as u64, *s);
                buf.copy_interleaved_ref(decoded);
                samples.extend_from_slice(buf.samples());
            }
            // 单包解码失败跳过（损坏帧不终止整次解码）
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => {
                return Err(format!(
                    "音频解码失败（.{ext}）：{e}；文件可能损坏，请重试或转为 wav/mp3"
                ))
            }
        }
    }
    let (sample_rate, channels) = spec.ok_or_else(|| {
        format!("音频解码失败（.{ext}）：没有解出任何音频帧；请重试或转为 wav/mp3")
    })?;
    let sample_rate = if sample_rate > 0 { sample_rate } else { fallback_rate };
    Ok(DecodedAudio {
        samples,
        sample_rate,
        channels,
    })
}

/// 扫包计时长（只解复用不解码）：Σ packet.dur() 按 timebase 折算毫秒
fn probe_symphonia_duration(path: &Path, ext: &str) -> Result<u64, String> {
    let (mut format, _decoder, _track_id, _rate) = open_symphonia(path, ext)?;
    let track = format
        .default_track()
        .ok_or_else(|| format!("文件里没有音频轨道（.{ext}），请选择一个音频文件"))?;
    // timebase 缺失时按采样率折算（1/rate 秒每 tick）
    let tb = track
        .codec_params
        .time_base
        .unwrap_or_else(|| TimeBase::new(1, track.codec_params.sample_rate.unwrap_or(1).max(1)));
    let mut total: u128 = 0;
    loop {
        match format.next_packet() {
            Ok(p) => total += p.dur() as u128,
            Err(SymError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymError::ResetRequired) => break,
            Err(_) => break,
        }
    }
    Ok((total * tb.numer as u128 * 1000 / tb.denom.max(1) as u128) as u64)
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str) -> std::path::PathBuf {
        // 每次调用独立编号：并行测试即使生成等长 wav 也不会互相覆盖
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "sm-decode-test-{}-{}-{}",
            std::process::id(),
            n,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// 构造一个最小 wav：fmt + data（可选在中间插一个 LIST 块模拟真实文件）
    fn build_wav(
        audio_format: u16,
        channels: u16,
        sample_rate: u32,
        bits: u16,
        data: &[u8],
        with_list: bool,
    ) -> Vec<u8> {
        let block_align = channels * (bits / 8);
        let byte_rate = sample_rate * block_align as u32;
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&audio_format.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        // Extensible 需要扩展区（cbSize=22 + valid_bits + mask + subformat GUID）
        if audio_format == 0xFFFE {
            fmt.extend_from_slice(&22u16.to_le_bytes());
            fmt.extend_from_slice(&bits.to_le_bytes());
            fmt.extend_from_slice(&0u32.to_le_bytes());
            let sub: u32 = 1; // KSDATAFORMAT_SUBTYPE_PCM
            fmt.extend_from_slice(&sub.to_le_bytes());
            fmt.extend_from_slice(&[0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71]);
        }
        let mut body = Vec::new();
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt);
        if fmt.len() % 2 == 1 {
            body.push(0);
        }
        if with_list {
            let info = b"INFOISFT\x0d\x00\x00\x00Lavf60.3\x00\x00"; // 22 字节（偶数，无需填充）
            body.extend_from_slice(b"LIST");
            body.extend_from_slice(&(info.len() as u32).to_le_bytes());
            body.extend_from_slice(info);
        }
        body.extend_from_slice(b"data");
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn decodes_pcm16_mono_16k() {
        let data: Vec<u8> = [0i16, 16384, -16384]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let bytes = build_wav(1, 1, 16_000, 16, &data, false);
        let decoded = decode_wav_bytes(&bytes).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples.len(), 3);
        assert_eq!(decoded.samples[0], 0.0);
        // 换算口径与 cpal 采集路径一致：/ i16::MAX（32767）
        assert!((decoded.samples[1] - 16384.0 / 32767.0).abs() < 1e-9);
        assert!((decoded.samples[2] + 16384.0 / 32767.0).abs() < 1e-9);
    }

    #[test]
    fn keeps_stereo_native_rate_for_session_resampler() {
        // 解码层保持原生采样率/声道（16k 单声道由会话循环与麦克风同路径重采样）
        let frames: Vec<i16> = vec![100, -100, 200, -200, 300, -300, 400, -400];
        let data: Vec<u8> = frames.iter().flat_map(|s| s.to_le_bytes()).collect();
        let bytes = build_wav(1, 2, 44_100, 16, &data, false);
        let decoded = decode_wav_bytes(&bytes).unwrap();
        assert_eq!(decoded.sample_rate, 44_100);
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.samples.len(), 8); // 4 帧 × 2 声道，保持交错
        assert_eq!(decoded.duration_ms(), 0); // 4 帧 → 不足 1ms
    }

    #[test]
    fn decodes_ieee_float32_wav() {
        let data: Vec<u8> = [0.25f32, -0.75]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let bytes = build_wav(3, 1, 48_000, 32, &data, false);
        let decoded = decode_wav_bytes(&bytes).unwrap();
        assert_eq!(decoded.sample_rate, 48_000);
        assert_eq!(decoded.samples, vec![0.25, -0.75]);
    }

    #[test]
    fn decodes_pcm24_samples() {
        // 24 位小端补码：0x010000 = 65536 → 65536/8388608 = 1/128
        let data = [0x00u8, 0x00, 0x01, 0x00, 0x00, 0x00];
        let bytes = build_wav(1, 1, 16_000, 24, &data, false);
        let decoded = decode_wav_bytes(&bytes).unwrap();
        assert_eq!(decoded.samples.len(), 2);
        assert!((decoded.samples[0] - 1.0 / 128.0).abs() < 1e-9);
        assert_eq!(decoded.samples[1], 0.0);
    }

    #[test]
    fn decodes_extensible_wav_looking_at_subformat() {
        let data: Vec<u8> = [0i16, 16384].iter().flat_map(|s| s.to_le_bytes()).collect();
        let bytes = build_wav(0xFFFE, 1, 16_000, 16, &data, false);
        let decoded = decode_wav_bytes(&bytes).unwrap();
        assert_eq!(decoded.samples.len(), 2);
        assert!((decoded.samples[1] - 16384.0 / 32767.0).abs() < 1e-9);
    }

    #[test]
    fn skips_list_chunk_between_fmt_and_data() {
        // 真实文件（如 lei-jun-test.wav）fmt 与 data 之间常带 LIST/INFO 元数据块
        let data: Vec<u8> = [0i16, 0].iter().flat_map(|s| s.to_le_bytes()).collect();
        let bytes = build_wav(1, 1, 16_000, 16, &data, true);
        let decoded = decode_wav_bytes(&bytes).unwrap();
        assert_eq!(decoded.samples.len(), 2);
    }

    #[test]
    fn odd_sized_unknown_chunk_is_padded() {
        // RIFF 块 2 字节对齐：奇数长度块后应有 1 字节填充，遍历不能错位
        let mut body = Vec::new();
        let fmt = {
            let mut f = Vec::new();
            f.extend_from_slice(&1u16.to_le_bytes());
            f.extend_from_slice(&1u16.to_le_bytes());
            f.extend_from_slice(&16_000u32.to_le_bytes());
            f.extend_from_slice(&32_000u32.to_le_bytes());
            f.extend_from_slice(&2u16.to_le_bytes());
            f.extend_from_slice(&16u16.to_le_bytes());
            f
        };
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt);
        let junk = b"junk!"; // 5 字节（奇数）
        body.extend_from_slice(b"JUNK");
        body.extend_from_slice(&(junk.len() as u32).to_le_bytes());
        body.extend_from_slice(junk);
        body.push(0); // 填充
        let data: Vec<u8> = [0i16, 0, 0].iter().flat_map(|s| s.to_le_bytes()).collect();
        body.extend_from_slice(b"data");
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(&data);
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);
        let decoded = decode_wav_bytes(&out).unwrap();
        assert_eq!(decoded.samples.len(), 3);
    }

    #[test]
    fn unsupported_wav_encoding_gives_actionable_error() {
        // 格式码 6 = a-law：明确不支持而不是崩溃
        let data = [0u8, 0];
        let bytes = build_wav(6, 1, 16_000, 8, &data, false);
        let err = decode_wav_bytes(&bytes).unwrap_err();
        assert!(err.contains("暂不支持的编码"), "{err}");
        assert!(err.contains("wav 或 mp3"), "{err}");
    }

    #[test]
    fn corrupt_wav_header_errors_in_chinese() {
        let err = parse_wav_header(b"RIFFxxxxgarbage!").unwrap_err();
        assert!(err.contains("wav 文件解析失败"), "{err}");
        // 有 RIFF 魔数但没有 fmt 块
        let bytes = {
            let mut out = Vec::new();
            out.extend_from_slice(b"RIFF");
            out.extend_from_slice(&4u32.to_le_bytes());
            out.extend_from_slice(b"WAVE");
            out
        };
        let err = parse_wav_header(&bytes).unwrap_err();
        assert!(err.contains("缺少 fmt 块"), "{err}");
    }

    #[test]
    fn duration_from_wav_header() {
        // 16k 单声道 16 位：32000 字节 data = 16000 帧 = 1000ms
        let data = vec![0u8; 32_000];
        let bytes = build_wav(1, 1, 16_000, 16, &data, false);
        let header = parse_wav_header(&bytes).unwrap();
        assert_eq!(header.duration_ms(), 1000);
    }

    #[test]
    fn truncated_data_chunk_reads_available_bytes() {
        // 声明的 data 长度超出文件末尾：取到实际可用部分（按整帧截断）
        let data = [0i16, 0, 0].iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<_>>();
        let mut bytes = build_wav(1, 1, 16_000, 16, &data, false);
        // 砍掉最后一个字节 → 2.5 帧 → 取 2 帧
        bytes.pop();
        let decoded = decode_wav_bytes(&bytes).unwrap();
        assert_eq!(decoded.samples.len(), 2);
    }

    #[test]
    fn missing_file_reports_chinese_error() {
        let path = temp_file("ghost.wav");
        let err = decode_audio_file(&path).unwrap_err();
        assert!(err.contains("找不到音频文件"), "{err}");
        let err = probe_duration_ms(&path).unwrap_err();
        assert!(err.contains("找不到音频文件"), "{err}");
    }

    #[test]
    fn unsupported_extension_rejected_before_any_decoding() {
        let path = temp_file("note.txt");
        std::fs::write(&path, b"hello").unwrap();
        let err = decode_audio_file(&path).unwrap_err();
        assert!(err.contains("暂不支持该格式"), "{err}");
        assert!(err.contains("wav 或 mp3"), "{err}");
        let err = probe_duration_ms(&path).unwrap_err();
        assert!(err.contains("暂不支持该格式"), "{err}");
        // 无扩展名同样拒绝
        let noext = path.with_extension("");
        std::fs::copy(&path, &noext).unwrap();
        assert!(decode_audio_file(&noext).unwrap_err().contains("暂不支持"));
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        let path = temp_file("clip.WAV");
        let data: Vec<u8> = [0i16, 16384].iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(&path, build_wav(1, 1, 16_000, 16, &data, false)).unwrap();
        let decoded = decode_audio_file(&path).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
    }

    #[test]
    fn decodes_real_repo_test_wav_when_present() {
        // 仓库自带的真实中文演讲（272 秒、16k 单声道 PCM16、fmt 与 data 之间有 LIST 块）
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("models")
            .join("test-wavs")
            .join("lei-jun-test.wav");
        if !path.exists() {
            return; // 仓库外环境（无测试素材）跳过
        }
        let decoded = decode_audio_file(&path).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        let ms = decoded.duration_ms();
        assert!((265_000..280_000).contains(&ms), "duration {ms}");
        // 时长探测（只读头）与完整解码一致
        assert_eq!(probe_duration_ms(&path).unwrap(), ms);
        // 开头 100ms 有实际波形（非全静音）
        assert!(decoded.samples.iter().take(1_600).any(|s| s.abs() > 1e-4));
    }

    /// 写临时 wav 并整链路解码（decode_audio_file 含存在性/扩展名分发）
    fn decode_wav_bytes(bytes: &[u8]) -> Result<DecodedAudio, String> {
        let path = temp_file(&format!("t{}.wav", bytes.len()));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        drop(f);
        decode_audio_file(&path)
    }
}
