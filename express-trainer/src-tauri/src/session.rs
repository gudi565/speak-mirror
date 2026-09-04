use crate::audio::{resample_to_16k_mono, start_capture, AudioCapture};
use crate::rules::engine::RuleEngine;
use crate::rules::Sentence;
use crate::settings::Correction;
use crate::voice::{VoiceAnalyzer, VOICE_UPDATE_INTERVAL_MS};
use sherpa_onnx::{
    OfflineModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
    OnlineModelConfig, OnlineParaformerModelConfig, OnlineRecognizer, OnlineRecognizerConfig,
    OnlineStream, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Emitter;

const SAMPLE_RATE: usize = 16_000;
const MAX_SEGMENT_SECONDS: usize = 30;
const MAX_SEGMENT_SAMPLES: usize = SAMPLE_RATE * MAX_SEGMENT_SECONDS;
/// partial 字幕节流间隔。300ms：段级持久流（SegmentStream）做的是增量
/// 解码，单次刷新成本只与本间隔内新增的几百 ms 音频相关（与段长无关，
/// 见 ignored 集成测试的耗时证据），因此可以压到 300ms 换更跟手的字幕；
/// 再往下收益递减且会与主循环 100ms 消费节拍/渲染刷新打架。
const PARTIAL_TRANSCRIPT_INTERVAL_MS: u64 = 300;
const MIN_TRAILING_SEGMENT_SECONDS: f32 = 0.3;
const PARA_DIR: &str = "sherpa-onnx-streaming-paraformer-bilingual-zh-en";
/// 可选离线高精引擎（SenseVoice）目录名（与 downloader::SENSE_DIR_NAME 一致）
pub const SENSE_VOICE_DIR: &str = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17";
/// 会话录音累积上限（30 分钟 16k PCM16 ≈ 57.6MB，防长会话内存失控）
pub const MAX_RECORD_SECONDS: usize = 30 * 60;

// ---------------------------------------------------------------------------
// 段级持久流（延迟修复）：partial 从 O(段长) 降为 O(增量)
// ---------------------------------------------------------------------------

/// 语音起始前的预滚样本时长：建流时先喂最近这段音频，补上 VAD 段头静音，
/// 防止首音节被流截掉（VAD 判定「语音开始」前已有少量样本进段）。
pub const STREAM_PRE_ROLL_SECONDS: f32 = 0.6;
/// 预滚缓冲容量（16k 采样）
pub fn pre_roll_samples() -> usize {
    (SAMPLE_RATE as f32 * STREAM_PRE_ROLL_SECONDS) as usize
}

/// 每个音频块对持久流的处置决策（纯函数可测）：
/// - 静音且无流：滚动预滚缓冲（覆盖最近 0.6s，等语音开始时作为段头喂入）
/// - 语音首块且无流：建流（先喂预滚，再喂当前块）
/// - 语音且流在：直喂当前块
/// - 语音停但流还在（切段瞬态，同轮循环内即会被 VAD 定稿取走）：不喂
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStep {
    BufferPreRoll,
    CreateStream,
    FeedStream,
    Hold,
}

pub fn stream_step(vad_detected: bool, stream_alive: bool) -> StreamStep {
    match (vad_detected, stream_alive) {
        (false, false) => StreamStep::BufferPreRoll,
        (true, false) => StreamStep::CreateStream,
        (true, true) => StreamStep::FeedStream,
        (false, true) => StreamStep::Hold,
    }
}

/// 预滚缓冲：追加样本并只保留最近 cap 个（超出从头部丢弃）
pub fn push_pre_roll(buf: &mut Vec<f32>, samples: &[f32], cap: usize) {
    buf.extend_from_slice(samples);
    if buf.len() > cap {
        let drain = buf.len() - cap;
        buf.drain(0..drain);
    }
}

/// 段级持久流解码器（OnlineRecognizer 的标准用法）：
/// - VAD 语音段开始时建流，块到达即 `accept_waveform`（增量喂入）
/// - partial 节流时对该流做增量 decode（`is_ready` 循环内 decode，
///   **不 input_finished、不重建流**）后 `get_result`——单次成本 O(增量)
/// - VAD 切段时对同一流 `input_finished` + decode 到底取终稿，随后弃流
///
/// 流的生命周期与 VAD 段对齐（≤ max_speech_duration 20s + 预滚 0.6s），
/// 不会随段间静音无限增长（静音期不喂流，只滚动预滚缓冲）。
pub struct SegmentStream {
    stream: Option<OnlineStream>,
    pre_roll: Vec<f32>,
}

impl SegmentStream {
    pub fn new() -> Self {
        Self { stream: None, pre_roll: Vec::new() }
    }

    pub fn is_alive(&self) -> bool {
        self.stream.is_some()
    }

    /// 每个音频块调用：按 VAD 状态决定 预滚 / 建流 / 喂流
    pub fn push(&mut self, recognizer: &OnlineRecognizer, vad_detected: bool, samples: &[f32]) {
        match stream_step(vad_detected, self.stream.is_some()) {
            StreamStep::BufferPreRoll => {
                push_pre_roll(&mut self.pre_roll, samples, pre_roll_samples())
            }
            StreamStep::CreateStream => {
                let stream = recognizer.create_stream();
                if !self.pre_roll.is_empty() {
                    stream.accept_waveform(SAMPLE_RATE as i32, &self.pre_roll);
                }
                stream.accept_waveform(SAMPLE_RATE as i32, samples);
                self.stream = Some(stream);
            }
            StreamStep::FeedStream => {
                if let Some(stream) = &self.stream {
                    stream.accept_waveform(SAMPLE_RATE as i32, samples);
                }
            }
            StreamStep::Hold => {}
        }
    }

    /// partial：增量 decode（不 input_finished、不重建流）后取当前已解码文本。
    /// 自上次调用以来新增的音频越多，本次要补的 decode 越多——节流间隔内
    /// 增量约为几百 ms，成本与段长无关。
    pub fn partial(&self, recognizer: &OnlineRecognizer) -> Option<String> {
        let stream = self.stream.as_ref()?;
        while recognizer.is_ready(stream) {
            recognizer.decode(stream);
        }
        recognizer.get_result(stream).map(|r| r.text)
    }

    /// 终稿：对同一流 `input_finished` + decode 到底取终稿，随后弃流。
    /// 返回 None 表示本就没有活流（语音从未被 VAD 认领），调用方回退
    /// `transcribe_buffer`。预滚缓冲一并清空（下一段从零开始）。
    pub fn finalize(&mut self, recognizer: &OnlineRecognizer) -> Option<String> {
        let stream = self.stream.take()?;
        stream.input_finished();
        while recognizer.is_ready(&stream) {
            recognizer.decode(&stream);
        }
        let text = recognizer.get_result(&stream).map(|r| r.text).unwrap_or_default();
        self.pre_roll.clear();
        Some(text)
    }

    /// 弃流不取稿：离线引擎已承担终稿时省去 decode 到底的 CPU
    pub fn discard(&mut self) {
        self.stream = None;
        self.pre_roll.clear();
    }
}

/// 终稿文本的引擎选择（纯函数可测）：优先离线高精（SenseVoice）；
/// 离线空稿（段内无可识别语音）回退流式终稿；流终稿缺失（多段同批切出、
/// 未被认领的尾巴等）回退一次性 `transcribe_buffer`。
pub fn pick_final_text(
    offline_text: Option<String>,
    stream_text: Option<String>,
    fallback_text: String,
) -> String {
    match offline_text {
        Some(t) if !t.trim().is_empty() => t,
        _ => stream_text.unwrap_or(fallback_text),
    }
}

/// Keep the live buffer from growing without bound during long silences.
pub fn trim_segment_to_cap(segment: &mut Vec<f32>, cap_samples: usize) {
    if segment.len() > cap_samples {
        let drain = segment.len() - cap_samples;
        segment.drain(0..drain);
    }
}

/// Whether the trailing live buffer is worth one last transcription after the
/// VAD drain: only non-trivial audio the VAD never claimed (e.g. sub-min-duration speech).
pub fn should_finalize_tail(remaining_samples: usize, sample_rate: usize, min_seconds: f32) -> bool {
    remaining_samples as f32 > sample_rate as f32 * min_seconds
}

/// One-shot transcription of a complete buffer with the streaming recognizer:
/// fresh stream, feed everything, mark input finished, decode to completion.
/// (Equivalent of sherpa-rs `TransducerRecognizer::transcribe`.)
pub fn transcribe_buffer(recognizer: &OnlineRecognizer, samples: &[f32]) -> String {
    let stream = recognizer.create_stream();
    stream.accept_waveform(SAMPLE_RATE as i32, samples);
    stream.input_finished();
    while recognizer.is_ready(&stream) {
        recognizer.decode(&stream);
    }
    recognizer
        .get_result(&stream)
        .map(|r| r.text)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 会话录音（搭便车）：在喂管线的 16k 单声道流上累积 i16 样本，
// 会话结束写 wav 到 appdata/sessions/audio/（音频只存本机，供逐句回放）
// ---------------------------------------------------------------------------

/// 录音累积器：超过 30 分钟上限后停止累积并标记截断（截断 = 不落盘不关联）
pub struct SessionRecorder {
    samples: Vec<i16>,
    capped: bool,
}

impl SessionRecorder {
    pub fn new() -> Self {
        Self { samples: Vec::new(), capped: false }
    }

    /// 搭便车累积一批 16k 单声道样本（不改变管线行为，纯只读旁路）
    pub fn push(&mut self, samples: &[f32]) {
        self.push_with_cap(samples, SAMPLE_RATE * MAX_RECORD_SECONDS);
    }

    /// 带上限的累积（cap 参数化仅为可测；生产路径用 30 分钟常量）
    pub fn push_with_cap(&mut self, samples: &[f32], cap: usize) {
        if self.capped {
            return;
        }
        if self.samples.len() + samples.len() > cap {
            self.capped = true;
            return;
        }
        self.samples.extend(crate::audio::f32_to_i16(samples));
    }

    /// 会话结束取样本：未截断且非空 → Some；截断/空 → None（不写文件）
    pub fn finish(self) -> Option<Vec<i16>> {
        if self.capped || self.samples.is_empty() {
            None
        } else {
            Some(self.samples)
        }
    }
}

/// 写会话录音 wav（失败静默降级 → None，绝不影响主流程）
pub fn write_session_wav(app: &tauri::AppHandle, samples: Option<Vec<i16>>) -> Option<String> {
    let samples = samples?;
    let dir = crate::history::sessions_dir(app).join("audio");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}.wav", crate::history::timestamp_id()));
    let bytes = crate::audio::wav_pcm16_bytes(&samples, SAMPLE_RATE as u32);
    std::fs::write(&path, bytes).ok()?;
    Some(path.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// 识别热词（sherpa-onnx crate 1.13.6 的 OnlineRecognizerConfig 暴露
// hotwords_file / hotwords_score 字段 → 走真实热词路线传入解码器；
// 注意运行时仅 transducer 系模型 + modified_beam_search 消费热词，
// 当前流式 Paraformer 不感知——兜底纠错见 settings::apply_corrections）
// ---------------------------------------------------------------------------

/// 会话启动时构建的热词参数（临时文件 + 权重）
pub struct AsrHotwords {
    pub file: PathBuf,
    pub score: f32,
}

/// 热词临时文件的守卫：run_session 结束（任何路径——包括 VAD/ASR 初始化
/// 失败的提前返回）时删除文件，防止失败路径把含用户词表的临时文件
/// 残留在系统临时目录。
pub struct HotwordsTempFile(PathBuf);

impl Drop for HotwordsTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// ASR 配置构建（纯函数可测）：无热词时与原配置逐字段一致；有热词时
/// 带上 hotwords_file/score。
///
/// 解码方法在两条路径下都保持 greedy_search：热词只在 transducer 系模型 +
/// modified_beam_search 下参与解码，而当前部署的流式 Paraformer 对其它
/// 解码方法会 `SHERPA_ONNX_EXIT(-1)` 直接杀进程（C++ 源码
/// online-recognizer-paraformer-impl.h 构造函数里硬校验）。hotwords_file
/// 对 Paraformer 是无害的惰性字段，将来换用 transducer 模型即自动生效
/// （届时需同步把 decoding_method 切到 modified_beam_search）。
pub fn build_asr_config(models_dir: &Path, hotwords: Option<&AsrHotwords>) -> OnlineRecognizerConfig {
    let para = models_dir.join(PARA_DIR);
    let mut cfg = OnlineRecognizerConfig {
        model_config: OnlineModelConfig {
            paraformer: OnlineParaformerModelConfig {
                encoder: Some(para.join("encoder.int8.onnx").to_string_lossy().into_owned()),
                decoder: Some(para.join("decoder.int8.onnx").to_string_lossy().into_owned()),
            },
            tokens: Some(para.join("tokens.txt").to_string_lossy().into_owned()),
            num_threads: 2,
            model_type: Some("paraformer".into()),
            ..Default::default()
        },
        decoding_method: Some("greedy_search".into()),
        ..Default::default()
    };
    if let Some(h) = hotwords {
        cfg.hotwords_file = Some(h.file.to_string_lossy().into_owned());
        cfg.hotwords_score = h.score;
    }
    cfg
}

/// 会话级选项（launch_session 从设置构建后传入 run_session）
pub struct SessionOptions {
    /// 会话录音开关（默认开）
    pub record_audio: bool,
    /// 热词（None = 未配置）
    pub hotwords: Option<AsrHotwords>,
    /// 终稿纠错映射（进规则引擎前替换）
    pub corrections: Vec<Correction>,
    /// 高精度终稿（双引擎，默认开）：SenseVoice 模型就绪时，句子定稿
    /// 优先用离线引擎重新识别该段；缺失/关闭则沿用流式终稿
    pub precision_finals: bool,
    /// VAD 灵敏度："standard"（阈值 0.5）/ "high"（0.35，远距离/小声录音）
    pub vad_sensitivity: String,
    /// 录音增强（自动增益，默认开）：文件模式整段重采样到 16k 后检测
    /// 电平（帧 RMS p95 < 0.08 视为过静）则线性放大（上限 8×），
    /// 改善手机远距离录音的 VAD 断句与识别；麦克风路径不启用
    pub enhance_audio: bool,
}

// ---------------------------------------------------------------------------
// 离线高精引擎（SenseVoice，可选组件）：终稿双引擎的「高精」一侧
// ---------------------------------------------------------------------------

/// SenseVoice 模型文件是否齐备（可选组件：缺失不算「模型不完整」，
/// 会话照常运行，终稿回退流式引擎；与 downloader::missing_sense_voice 对应）
pub fn sense_voice_ready(models_dir: &Path) -> bool {
    let dir = models_dir.join(SENSE_VOICE_DIR);
    dir.join("model.int8.onnx").is_file() && dir.join("tokens.txt").is_file()
}

/// SenseVoice 配置构建（纯函数可测）：language="zh"（产品面向中文为主、
/// 中英混合的练习场景，锁定中文可避免语种误判；英文品牌词仍可识别）、
/// use_itn=true（数字/单位正规化，如「二零二四」→「2024」）。
/// num_threads=4：终稿解码发生在句尾停顿期（流式引擎此刻空闲），偏高配。
pub fn build_sense_voice_config(models_dir: &Path) -> OfflineRecognizerConfig {
    let dir = models_dir.join(SENSE_VOICE_DIR);
    OfflineRecognizerConfig {
        model_config: OfflineModelConfig {
            sense_voice: OfflineSenseVoiceModelConfig {
                model: Some(dir.join("model.int8.onnx").to_string_lossy().into_owned()),
                language: Some("zh".into()),
                use_itn: true,
            },
            tokens: Some(dir.join("tokens.txt").to_string_lossy().into_owned()),
            num_threads: 4,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 离线一次性转写（SenseVoice）：输出自带标点与 ITN
pub fn transcribe_offline(recognizer: &OfflineRecognizer, samples: &[f32]) -> String {
    if samples.is_empty() {
        return String::new();
    }
    let stream = recognizer.create_stream();
    stream.accept_waveform(SAMPLE_RATE as i32, samples);
    recognizer.decode(&stream);
    stream.get_result().map(|r| r.text).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 音频源抽象：麦克风（cpal 实时）与音频文件（1 倍速喂入同一管线）
// ---------------------------------------------------------------------------

/// 会话音频源。`Mic` 走原有 cpal 路径（行为不变）；`File` 把解码好的原生
/// 交错样本交给喂入线程按指定倍速推送，之后与麦克风共用同一主循环。
pub enum AudioSource {
    Mic,
    File {
        /// 原生采样率下的交错样本（重采样到 16k 与麦克风同路径、同代码）
        samples: Vec<f32>,
        sample_rate: u32,
        channels: u16,
        /// 喂入倍速（1.0 / 1.5 / 2.0 / 99=极速不限速，lib.rs 校验）：
        /// 内容时间戳不受影响；极速档跳过 partial（见 `should_emit_partial`）
        speed: f64,
    },
}

/// 主循环实际读取的块流：麦克风直接来自 cpal；文件来自 1 倍速喂入线程。
/// 文件喂完后发送端 drop → `Disconnected`，主循环据此做终稿 flush 收尾。
enum Feed {
    Mic(AudioCapture),
    File {
        rx: Receiver<Vec<f32>>,
        sample_rate: u32,
        channels: u16,
    },
}

impl Feed {
    fn recv_timeout(&self, d: Duration) -> Result<Vec<f32>, RecvTimeoutError> {
        match self {
            Feed::Mic(c) => c.rx.recv_timeout(d),
            Feed::File { rx, .. } => rx.recv_timeout(d),
        }
    }
    fn sample_rate(&self) -> u32 {
        match self {
            Feed::Mic(c) => c.sample_rate,
            Feed::File { sample_rate, .. } => *sample_rate,
        }
    }
    fn channels(&self) -> u16 {
        match self {
            Feed::Mic(c) => c.channels,
            Feed::File { channels, .. } => *channels,
        }
    }
    fn is_file(&self) -> bool {
        matches!(self, Feed::File { .. })
    }
}

/// 句子时间戳时钟：
/// - `Wall`：麦克风模式沿用墙钟（与会话启动点比较，与原实现逐字一致）
/// - `Content`：文件模式按已喂入的 16k 采样数推出**音频内容时间**，
///   不受解码/调度耗时的墙钟抖动影响（start_ms/end_ms 与素材内位置对齐）
pub enum SessionClock {
    Wall { started: Instant },
    Content { fed_samples: u64 },
}

impl SessionClock {
    pub fn now_ms(&self) -> u64 {
        match self {
            SessionClock::Wall { started } => started.elapsed().as_millis() as u64,
            SessionClock::Content { fed_samples } => fed_samples * 1000 / SAMPLE_RATE as u64,
        }
    }
    /// 记录又喂入了多少 16k 采样（仅 Content 模式推进）
    pub fn advance(&mut self, samples_16k: usize) {
        if let SessionClock::Content { fed_samples } = self {
            *fed_samples += samples_16k as u64;
        }
    }
}

/// 终稿句 start_ms：end_ms 往前推该段样本的时长（16k 下每毫秒 16 个样本）。
/// 麦克风与文件模式共用同一公式。
pub fn sentence_start_ms(end_ms: u64, sample_count: usize) -> u64 {
    end_ms.saturating_sub(sample_count as u64 / 16)
}

/// 文件喂入的块时长：与 cpal 块的节奏量级一致（100ms），
/// 保证 partial 节流（300ms）/voice_update（2s）/停顿计数等实时行为与真麦一致
pub const FILE_FEED_CHUNK_MS: u64 = 100;

/// pacing 块大小（源采样率下每块的帧数）
pub fn feed_frames_per_chunk(sample_rate: u32, chunk_ms: u64) -> usize {
    (sample_rate as u64 * chunk_ms / 1000).max(1) as usize
}

/// 按绝对时刻表喂入的线程：第 i 块（内容时长 100ms）应在 i×(100ms/倍速)
/// 的墙钟时刻送达（不累积漂移），全部发完即 drop 发送端，主循环收到
/// `Disconnected` 走「播放到头」收尾。会话停止时接收端被 drop，
/// `send` 失败即退出，不泄漏。时间戳/统计走内容时钟（样本位置），与倍速无关。
///
/// 极速档（speed == FILE_SPEED_MAX）：完全不等时刻表，全部块连续 send
/// （channel 无界容量给足），消费端（重采样/VAD/识别）跑多快算多快——
/// 块序列与 1.0 速完全一致（同块同序），仅送达节奏不同，因此终稿一致。
fn spawn_file_feeder(
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u16,
    speed: f64,
    tx: Sender<Vec<f32>>,
) {
    let per_chunk =
        feed_frames_per_chunk(sample_rate, FILE_FEED_CHUNK_MS) * channels.max(1) as usize;
    let max = crate::is_max_speed(speed);
    let started = Instant::now();
    std::thread::Builder::new()
        .name("file-feeder".into())
        .spawn(move || {
            if max {
                for chunk in samples.chunks(per_chunk) {
                    if tx.send(chunk.to_vec()).is_err() {
                        return; // 会话已停止
                    }
                }
                return;
            }
            // 倍速 s 下相邻块的墙钟间隔 = 块内容时长 / s（下限防 0/负值）
            let step = Duration::from_secs_f64(
                FILE_FEED_CHUNK_MS as f64 / 1000.0 / speed.clamp(0.25, 8.0),
            );
            for (i, chunk) in samples.chunks(per_chunk).enumerate() {
                // 先睡后发，按绝对时刻对齐防漂移
                let at = started + step * (i as u32);
                let now = Instant::now();
                if at > now {
                    std::thread::sleep(at - now);
                }
                if tx.send(chunk.to_vec()).is_err() {
                    return; // 会话已停止
                }
            }
        })
        .expect("spawn file feeder");
}

/// partial 是否发射（纯函数可测）：语音中且距上次 ≥300ms 才做增量解码；
/// **极速档完全跳过**——喂入跑得比实时快，字幕滚动已无意义（只会闪现
/// 过时片段），只出终稿句。麦克风与 1.0/1.5/2.0 文件档行为不变。
pub fn should_emit_partial(max_speed: bool, detected: bool, elapsed: Duration) -> bool {
    detected && !max_speed && elapsed >= Duration::from_millis(PARTIAL_TRANSCRIPT_INTERVAL_MS)
}

pub fn run_session(
    app: tauri::AppHandle,
    stop_rx: Receiver<()>,
    source: AudioSource,
    engine: Arc<Mutex<RuleEngine>>,
    voice_state: Arc<Mutex<VoiceAnalyzer>>,
    models_dir: PathBuf,
    options: SessionOptions,
) -> Result<Option<String>, String> {
    // 断句灵敏度（设置项 → Silero 阈值：标准 0.5 / 高灵敏度 0.35）
    let vad_threshold = crate::settings::vad_threshold(&options.vad_sensitivity);
    let vad_cfg = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(models_dir.join("silero_vad.onnx").to_string_lossy().into_owned()),
            threshold: vad_threshold,
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
    let vad = VoiceActivityDetector::create(&vad_cfg, 30.0)
        .ok_or_else(|| "VAD 初始化失败（模型文件损坏？请删除应用数据目录下的 models 后重新下载）".to_string())?;

    let asr_cfg = build_asr_config(&models_dir, options.hotwords.as_ref());
    let recognizer = OnlineRecognizer::create(&asr_cfg).ok_or_else(|| {
        "ASR 初始化失败（模型文件损坏？请删除应用数据目录下的 models 后重新下载）".to_string()
    })?;
    // 热词文件只在识别器创建时被读取；守卫保证任何退出路径（含初始化
    // 失败的提前 return）都会删掉临时文件，不残留
    let _hotwords_guard = options.hotwords.as_ref().map(|h| HotwordsTempFile(h.file.clone()));

    // 双引擎（B）· 离线高精侧：会话开始时惰性加载（模型存在且开关开才创建，
    // 约几百 ms）。缺失/关闭 → None 静默回退流式终稿；加载失败发一条中文
    // 日志事件后回退，不阻断会话。
    let offline: Option<OfflineRecognizer> = if options.precision_finals && sense_voice_ready(&models_dir) {
        match OfflineRecognizer::create(&build_sense_voice_config(&models_dir)) {
            Some(r) => {
                eprintln!("SenseVoice 高精度终稿引擎已加载（离线双引擎模式）");
                Some(r)
            }
            None => {
                eprintln!("SenseVoice 高精度终稿引擎加载失败，终稿回退流式引擎");
                let _ = app.emit(
                    "engine_notice",
                    serde_json::json!({ "message": "高精度识别引擎加载失败，本次会话句子定稿将使用流式引擎" }),
                );
                None
            }
        }
    } else {
        None
    };

    // 极速档：partial 完全跳过（喂入比实时快，字幕滚动无意义），
    // 只出终稿句；时间戳/语速/停顿/声音指标仍按内容时钟，统计不受影响
    let skip_partial = matches!(&source, AudioSource::File { speed, .. } if crate::is_max_speed(*speed));

    let feed = match source {
        AudioSource::Mic => Feed::Mic(start_capture()?),
        AudioSource::File { samples, sample_rate, channels, speed } => {
            // 录音增强（自动增益）：整段先重采样到 16k 单声道（与主循环逐块
            // 重采样同一函数），再做整段电平检测与线性放大——远距离/过静的
            // 手机录音拉到 VAD 与识别引擎的舒适电平。达标（增益 1.0）时也
            // 已整段重采样，主循环内对 16k 输入是直通，行为等价。
            let (samples, sample_rate, channels) = if options.enhance_audio {
                let resampled = resample_to_16k_mono(&samples, sample_rate, channels);
                let (gained, gain) = crate::audio::auto_gain(&resampled);
                if gain > 1.0 {
                    eprintln!("录音增强：电平过低（帧RMS p95 < 0.08），整段放大 {gain:.2}×");
                }
                (gained, SAMPLE_RATE as u32, 1u16)
            } else {
                (samples, sample_rate, channels)
            };
            let (tx, rx) = std::sync::mpsc::channel::<Vec<f32>>();
            spawn_file_feeder(samples, sample_rate, channels, speed, tx);
            Feed::File { rx, sample_rate, channels }
        }
    };
    // 麦克风：墙钟（与原实现一致）；文件：内容时间（样本位置/16k）
    let mut clock = match feed {
        Feed::Mic(_) => SessionClock::Wall { started: Instant::now() },
        Feed::File { .. } => SessionClock::Content { fed_samples: 0 },
    };
    // 文件播放到头（喂入线程发完并断开）→ 循环后发 session_finished
    let mut natural_end = false;
    let mut sentence_id: u64 = 0;
    let mut current_segment: Vec<f32> = Vec::new();
    let mut last_partial = Instant::now();
    // 段级持久流（延迟修复）：partial 增量解码，不再整段重转写
    let mut seg_stream = SegmentStream::new();
    // 声音层搭便车：每 ~2s 推送一次实时指标（独立轻量事件，不混入 analysis_update）
    let mut last_voice_emit = Instant::now();
    // 录音搭便车：与喂管线的同一条 16k 单声道流同步累积（麦克风/文件两路都录）
    let mut recorder = if options.record_audio { Some(SessionRecorder::new()) } else { None };

    engine.lock().unwrap().start(0);

    let app_for_finalize = app.clone();
    // 传入时钟而非算好的 now_ms：end_ms 在转写完成后取值（与原实现同一时点）。
    // 文件模式时钟只在喂入样本时推进，转写期间读到的就是当前内容位置。
    //
    // 终稿引擎选择（pick_final_text）：SenseVoice 离线高精（VAD 段独立短解码，
    // 句尾停顿期间完成）→ 段级持久流终稿（input_finished + decode 到底）→
    // transcribe_buffer 一次性转写（多段同批切出、未认领尾巴等边缘）。
    let mut finalize_segment = |samples: &[f32],
                                offline_text: Option<String>,
                                stream_text: Option<String>,
                                clock: &SessionClock| {
        if samples.is_empty() {
            return;
        }
        let fallback = transcribe_buffer(&recognizer, samples);
        let text = pick_final_text(offline_text, stream_text, fallback);
        // 终稿纠错：进规则引擎前替换「错->对」映射（当前 Paraformer 不消费
        // 解码级热词，这层保证热词类需求可感知）
        let text = crate::settings::apply_corrections(text.trim(), &options.corrections);
        if text.is_empty() {
            return;
        }
        sentence_id += 1;
        let end_ms = clock.now_ms();
        let sentence = Sentence {
            id: sentence_id,
            text,
            start_ms: sentence_start_ms(end_ms, samples.len()),
            end_ms,
        };
        let (events, snapshot) = {
            let mut eng = engine.lock().unwrap();
            let events = eng.ingest(sentence.clone());
            (events, eng.snapshot())
        };
        let _ = app_for_finalize.emit("sentence_final", &sentence);
        let _ = app_for_finalize.emit(
            "analysis_update",
            serde_json::json!({ "events": events, "snapshot": snapshot }),
        );
    };

    loop {
        if stop_rx.try_recv().is_ok() {
            break;
        }
        // 声音层搭便车（只读推送，不影响循环节奏：recv_timeout 决定节拍）
        if last_voice_emit.elapsed() >= Duration::from_millis(VOICE_UPDATE_INTERVAL_MS) {
            last_voice_emit = Instant::now();
            let _ = app.emit(
                "voice_update",
                voice_state.lock().unwrap_or_else(|p| p.into_inner()).metrics(),
            );
        }
        match feed.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                let samples = resample_to_16k_mono(&chunk, feed.sample_rate(), feed.channels());
                // 文件模式：先推进内容时钟（终稿时间戳以“当前喂到的位置”为准）
                clock.advance(samples.len());
                current_segment.extend_from_slice(&samples);
                trim_segment_to_cap(&mut current_segment, MAX_SEGMENT_SAMPLES);
                vad.accept_waveform(&samples);
                let detected = vad.detected();
                // 段级持久流：与 VAD 状态同步喂入（语音段开始建流、静音只滚预滚）
                seg_stream.push(&recognizer, detected, &samples);
                // 录音搭便车：只读旁路累积，不改变任何管线行为
                if let Some(rec) = recorder.as_mut() {
                    rec.push(&samples);
                }
                // 声音层搭便车：只读采样（vad.detected() 是纯查询），不改变任何行为
                voice_state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push_chunk(&samples, detected);

                // partial：说话中每 300ms 对持久流做一次增量解码（不重建流、
                // 不 input_finished——成本只与节流间隔内的增量相关）；
                // 极速档整体跳过（should_emit_partial，只出终稿句）
                if should_emit_partial(skip_partial, detected, last_partial.elapsed()) {
                    last_partial = Instant::now();
                    if let Some(text) = seg_stream.partial(&recognizer) {
                        if !text.trim().is_empty() {
                            let _ = app.emit("partial_transcript", serde_json::json!({ "text": text }));
                        }
                    }
                }

                // 句子定稿
                while !vad.is_empty() {
                    if let Some(seg) = vad.front() {
                        // 双引擎终稿：离线引擎在位时由它出稿（持久流直接弃掉，
                        // 省 decode 到底的 CPU）；否则取持久流终稿。
                        // 同批多段切出（罕见，仅停止/收尾瞬间）：只有首段有持久流，
                        // 后续段走 finalize 里的 transcribe_buffer 回退。
                        let (offline_text, stream_text) = if offline.is_some() {
                            seg_stream.discard();
                            (Some(transcribe_offline(offline.as_ref().unwrap(), seg.samples())), None)
                        } else {
                            (None, seg_stream.finalize(&recognizer))
                        };
                        finalize_segment(seg.samples(), offline_text, stream_text, &clock);
                        // 声音层搭便车：VAD 断句边界结算句能量
                        voice_state
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .close_sentence();
                    }
                    vad.pop();
                    current_segment.clear();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            // 麦克风：设备断开；文件：喂入线程发完全部样本（播放到头）
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if feed.is_file() {
                    natural_end = true;
                }
                break;
            }
        }
    }

    // Final flush: emit any VAD-buffered segment and any trailing live audio
    // so the sentence being spoken is not dropped when the user hits stop
    // (文件模式：这一步也是「播放到头」的终稿收尾，随后发 session_finished)。
    vad.flush();
    while !vad.is_empty() {
        if let Some(seg) = vad.front() {
            let (offline_text, stream_text) = if offline.is_some() {
                seg_stream.discard();
                (Some(transcribe_offline(offline.as_ref().unwrap(), seg.samples())), None)
            } else {
                (None, seg_stream.finalize(&recognizer))
            };
            finalize_segment(seg.samples(), offline_text, stream_text, &clock);
            // 声音层搭便车：与主循环一致的边界结算
            voice_state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .close_sentence();
        }
        vad.pop();
        // The flushed VAD segment contains the same samples as current_segment;
        // clear it so the trailing finalize below cannot emit the sentence twice.
        current_segment.clear();
    }
    if should_finalize_tail(current_segment.len(), SAMPLE_RATE, MIN_TRAILING_SEGMENT_SECONDS) {
        // 尾巴是 VAD 未认领的音频：持久流通常不存在（语音从未 detected），
        // finalize 返回 None → finalize_segment 回退 transcribe_buffer（与原实现一致）
        let (offline_text, stream_text) = if offline.is_some() {
            seg_stream.discard();
            (Some(transcribe_offline(offline.as_ref().unwrap(), &current_segment)), None)
        } else {
            (None, seg_stream.finalize(&recognizer))
        };
        finalize_segment(&current_segment, offline_text, stream_text, &clock);
        voice_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .close_sentence();
    }

    // drop feed：麦克风停止采集（原 drop(capture.stream)）；文件模式让喂入线程退出
    drop(feed);

    // 会话录音：停止与自然播完两条路径都到这里，统一落盘（失败静默降级）
    let audio_path = write_session_wav(&app, recorder.take().and_then(SessionRecorder::finish));

    // 文件播放到头：终稿 flush 完成后通知前端（前端据此调 stop_session 拿快照并进入总结页）
    if natural_end {
        let _ = app.emit("session_finished", ());
    }
    Ok(audio_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_segment_keeps_newest_samples_up_to_cap() {
        let mut buf: Vec<f32> = (0..100).map(|i| i as f32).collect();
        trim_segment_to_cap(&mut buf, 40);
        assert_eq!(buf.len(), 40);
        assert_eq!(buf[0], 60.0);
        assert_eq!(buf[39], 99.0);
    }

    #[test]
    fn trim_segment_is_no_op_when_under_cap() {
        let mut buf: Vec<f32> = (0..30).map(|i| i as f32).collect();
        trim_segment_to_cap(&mut buf, 40);
        assert_eq!(buf.len(), 30);
        assert_eq!(buf[0], 0.0);
    }

    #[test]
    fn should_finalize_tail_rejects_empty_and_trivial_buffers() {
        assert!(!should_finalize_tail(0, 16_000, 0.3));
        // 0.2s of audio is below the 0.3s threshold
        assert!(!should_finalize_tail((16_000.0 * 0.2) as usize, 16_000, 0.3));
    }

    #[test]
    fn should_finalize_tail_accepts_unclaimed_speech_tail() {
        // 0.5s of audio the VAD never claimed is worth one last transcription
        assert!(should_finalize_tail((16_000.0 * 0.5) as usize, 16_000, 0.3));
    }

    #[test]
    fn content_clock_converts_fed_samples_to_content_ms() {
        let mut clock = SessionClock::Content { fed_samples: 0 };
        assert_eq!(clock.now_ms(), 0);
        clock.advance(16_000); // 1 秒音频
        assert_eq!(clock.now_ms(), 1_000);
        clock.advance(8_000); // 半秒
        assert_eq!(clock.now_ms(), 1_500);
        // 272 秒素材（lei-jun-test.wav 量级）
        for _ in 0..272 {
            clock.advance(16_000);
        }
        assert_eq!(clock.now_ms(), 273_500);
    }

    #[test]
    fn content_clock_only_advances_for_16k_samples() {
        // advance 语义是“又喂入了多少 16k 采样”（源采样率已在循环里重采样过）
        let mut clock = SessionClock::Content { fed_samples: 100 };
        clock.advance(441);
        assert_eq!(clock.now_ms(), (100 + 441) * 1000 / 16_000);
    }

    #[test]
    fn wall_clock_counts_from_session_start() {
        let clock = SessionClock::Wall { started: Instant::now() };
        let a = clock.now_ms();
        std::thread::sleep(Duration::from_millis(5));
        let b = clock.now_ms();
        assert!(b >= a);
    }

    #[test]
    fn sentence_start_ms_pushes_back_by_sample_duration() {
        // 1 秒的句子在 10s 位置定稿 → start 9s
        assert_eq!(sentence_start_ms(10_000, 16_000), 9_000);
        // 600ms 声音（9600 样本 = 600ms）
        assert_eq!(sentence_start_ms(10_000, 9_600), 9_400);
        // 样本跨过会话起点：饱和到 0 而不是下溢
        assert_eq!(sentence_start_ms(500, 16_000), 0);
    }

    #[test]
    fn feed_frames_per_chunk_matches_chunk_duration() {
        assert_eq!(feed_frames_per_chunk(16_000, 100), 1_600);
        assert_eq!(feed_frames_per_chunk(44_100, 100), 4_410);
        assert_eq!(feed_frames_per_chunk(48_000, 100), 4_800);
        // 极端低采样率也不为 0
        assert_eq!(feed_frames_per_chunk(8_000, 100), 800);
        assert_eq!(feed_frames_per_chunk(1, 100), 1);
    }

    #[test]
    fn file_feeder_paces_at_1x_then_disconnects() {
        // 5 块 × 100ms 的 16k 单声道样本（块内含序号，验证保序完整）
        let samples: Vec<f32> = (0..5 * 1_600).map(|i| i as f32).collect();
        let (tx, rx) = std::sync::mpsc::channel();
        let started = Instant::now();
        spawn_file_feeder(samples.clone(), 16_000, 1, 1.0, tx);
        let mut got: Vec<f32> = Vec::new();
        let mut chunks = 0;
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(c) => {
                    chunks += 1;
                    got.extend_from_slice(&c);
                }
                // 发送端全部发完即断开：这是「播放到头」的信号
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("feeder stalled"),
            }
        }
        assert_eq!(chunks, 5);
        assert_eq!(got, samples);
        // 1 倍速：500ms 素材不允许瞬间倒灌（下界 400ms 容忍首块立即发送），
        // 上界放宽避免 CI 抖动误报
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(400), "elapsed {elapsed:?}");
        assert!(elapsed < Duration::from_secs(3), "elapsed {elapsed:?}");
    }

    #[test]
    fn file_feeder_paces_at_2x_then_disconnects() {
        // 20 块 × 100ms 内容 = 2s 素材；2 倍速应在 ~1s 墙钟内送完。
        // 窗口放宽（0.9–1.8s）：上界须明显低于 1 倍速的 2s，下界容忍首块立即
        // 发送——并行测试/重负载下小素材的窄窗口会抖动误报
        let samples: Vec<f32> = (0..20 * 1_600).map(|i| i as f32).collect();
        let (tx, rx) = std::sync::mpsc::channel();
        let started = Instant::now();
        spawn_file_feeder(samples.clone(), 16_000, 1, 2.0, tx);
        let mut got: Vec<f32> = Vec::new();
        let mut chunks = 0;
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(c) => {
                    chunks += 1;
                    got.extend_from_slice(&c);
                }
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("feeder stalled"),
            }
        }
        assert_eq!(chunks, 20);
        assert_eq!(got, samples);
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(900), "elapsed {elapsed:?}");
        assert!(elapsed < Duration::from_millis(1800), "elapsed {elapsed:?}");
    }

    #[test]
    fn should_emit_partial_truth_table_and_max_speed_skips() {
        // 节流常量锁定 300ms（跟手性关键参数，防无意回退到 600ms）
        assert_eq!(PARTIAL_TRANSCRIPT_INTERVAL_MS, 300);
        let hot = Duration::from_millis(PARTIAL_TRANSCRIPT_INTERVAL_MS + 10);
        let cold = Duration::from_millis(100);
        // 实时档（麦克风 / 1.0 / 1.5 / 2.0）：语音中且节流期满 → 发射
        assert!(should_emit_partial(false, true, hot));
        assert!(!should_emit_partial(false, true, cold)); // 节流期内
        assert!(!should_emit_partial(false, false, hot)); // 静音段不发
        // 极速档：无条件跳过 partial（跑得比实时快，字幕滚动无意义）
        assert!(!should_emit_partial(true, true, hot));
        assert!(!should_emit_partial(true, true, Duration::from_secs(10)));
        assert!(!should_emit_partial(true, false, hot));
    }

    #[test]
    fn file_feeder_max_speed_sends_all_chunks_without_pacing() {
        // 60 块 × 100ms = 6 秒素材：极速档应远快于 1 倍速实时（6s）送达，
        // 且块序列与 1.0 速完全一致（同块同序 → 终稿必然一致）。
        // 上界 2s：无界通道发 60 块本是毫秒级，2s 足以证明"无节流"语义，
        // 同时在并行测试/重负载下不再抖动误报
        let samples: Vec<f32> = (0..60 * 1_600).map(|i| i as f32).collect();
        let (tx, rx) = std::sync::mpsc::channel();
        let started = Instant::now();
        spawn_file_feeder(samples.clone(), 16_000, 1, crate::FILE_SPEED_MAX, tx);
        let mut got: Vec<f32> = Vec::new();
        let mut chunks = 0;
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(c) => {
                    chunks += 1;
                    got.extend_from_slice(&c);
                }
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("feeder stalled"),
            }
        }
        assert_eq!(chunks, 60);
        assert_eq!(got, samples); // 与 1.0 速的块序列逐样本一致
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(2), "elapsed {elapsed:?}");
    }

    #[test]
    fn recorder_accumulates_converted_samples_under_cap() {
        let empty = SessionRecorder::new();
        assert_eq!(empty.finish(), None); // 空会话不落盘
        let mut rec = SessionRecorder::new();
        rec.push(&[0.0, 0.5, -0.5]);
        rec.push(&[2.0]); // 超界钳位
        assert_eq!(rec.finish().unwrap(), vec![0, 16384, -16384, 32767]);
    }

    #[test]
    fn recorder_marks_truncated_over_cap() {
        // cap 参数化：小上限验证「超限即弃」语义，生产常量单测校验
        assert_eq!(MAX_RECORD_SECONDS, 30 * 60);
        assert_eq!(SAMPLE_RATE * MAX_RECORD_SECONDS, 28_800_000);

        let mut rec = SessionRecorder::new();
        rec.push_with_cap(&[0.0, 0.1], 4);
        rec.push_with_cap(&[0.2, 0.3], 4); // 累计恰好到上限 4：仍可用
        assert_eq!(rec.finish().map(|s| s.len()), Some(4));

        // 超限 → 截断 → 不落盘
        let mut rec2 = SessionRecorder::new();
        rec2.push_with_cap(&[0.0, 0.1, 0.2, 0.3, 0.4], 4);
        assert_eq!(rec2.finish(), None);

        // 截断后再 push 也不再累积（防内存继续增长），依然不落盘
        let mut rec3 = SessionRecorder::new();
        rec3.push_with_cap(&[0.0, 0.1, 0.2, 0.3, 0.4], 4);
        rec3.push_with_cap(&[1.0], 4);
        assert_eq!(rec3.finish(), None);
    }

    #[test]
    fn hotwords_temp_file_guard_removes_on_drop() {
        let dir = std::env::temp_dir().join(format!("sm-hotwords-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hotwords.txt");
        std::fs::write(&path, "DeepSeek\n米糕").unwrap();
        {
            let _guard = HotwordsTempFile(path.clone());
            assert!(path.exists()); // 守卫存活期间文件仍在（识别器创建要用）
        }
        assert!(!path.exists(), "drop 时必须删除临时文件");
        // 文件本就不存在：drop 静默（不 panic）
        drop(HotwordsTempFile(dir.join("no-such-file.txt")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn asr_config_without_hotwords_matches_original_greedy() {
        // 未配置热词：与原实现逐字段一致（麦克风路径行为不变的前提）
        let cfg = build_asr_config(Path::new("models"), None);
        assert_eq!(cfg.decoding_method.as_deref(), Some("greedy_search"));
        assert_eq!(cfg.hotwords_file, None);
        assert_eq!(cfg.hotwords_score, 0.0);
        assert_eq!(cfg.max_active_paths, 0);
        assert_eq!(cfg.model_config.num_threads, 2);
        assert!(cfg
            .model_config
            .paraformer
            .encoder
            .as_deref()
            .unwrap()
            .ends_with("encoder.int8.onnx"));
    }

    #[test]
    fn asr_config_with_hotwords_sets_fields_but_keeps_greedy() {
        let hw = AsrHotwords { file: PathBuf::from("hotwords.txt"), score: 1.5 };
        let cfg = build_asr_config(Path::new("models"), Some(&hw));
        // 热词字段原样传入（route A；对 Paraformer 惰性，换 transducer 模型即生效）
        assert_eq!(cfg.hotwords_file.as_deref(), Some("hotwords.txt"));
        assert_eq!(cfg.hotwords_score, 1.5);
        // 但解码方法必须保持 greedy_search：流式 Paraformer 的 C++ 实现
        // （online-recognizer-paraformer-impl.h 构造函数）对其它解码方法直接
        // SHERPA_ONNX_EXIT(-1) 杀进程，绝不能为热词切 modified_beam_search
        assert_eq!(cfg.decoding_method.as_deref(), Some("greedy_search"));
        assert_eq!(cfg.max_active_paths, 0);
    }

    // -----------------------------------------------------------------------
    // 段级持久流（增量解码状态机）
    // -----------------------------------------------------------------------

    #[test]
    fn stream_step_truth_table() {
        use StreamStep::*;
        // 静音且无流：滚动预滚缓冲（不建流，段间静音不进流 → 流不随静音增长）
        assert_eq!(stream_step(false, false), BufferPreRoll);
        // 语音首块：建流（先喂预滚补 VAD 段头，再喂当前块）
        assert_eq!(stream_step(true, false), CreateStream);
        // 语音进行中：直喂持久流（partial 增量解码的前提）
        assert_eq!(stream_step(true, true), FeedStream);
        // 语音刚停但流还在（切段瞬态，同轮循环内被定稿取走）：不喂
        assert_eq!(stream_step(false, true), Hold);
    }

    #[test]
    fn pre_roll_keeps_only_recent_window() {
        // 容量 = 0.6s × 16k = 9600 样本
        assert_eq!(pre_roll_samples(), 9_600);
        let mut buf: Vec<f32> = Vec::new();
        push_pre_roll(&mut buf, &(0..4_000).map(|i| i as f32).collect::<Vec<_>>(), 9_600);
        assert_eq!(buf.len(), 4_000);
        // 再推 8_000：超容量 → 保留最近 9_600（旧样本从头丢弃）
        push_pre_roll(&mut buf, &(4_000..12_000).map(|i| i as f32).collect::<Vec<_>>(), 9_600);
        assert_eq!(buf.len(), 9_600);
        assert_eq!(buf[0], 2_400.0);
        assert_eq!(*buf.last().unwrap(), 11_999.0);
        // 小块追加不足容量时原样保留
        let mut small = vec![1.0, 2.0];
        push_pre_roll(&mut small, &[3.0], 9_600);
        assert_eq!(small, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn pick_final_text_prefers_offline_then_stream_then_fallback() {
        // 离线高精优先
        assert_eq!(
            pick_final_text(Some("高精稿".into()), Some("流稿".into()), "兜底".into()),
            "高精稿"
        );
        // 离线空稿（段内无可识别语音）→ 流式终稿
        assert_eq!(
            pick_final_text(Some("  ".into()), Some("流稿".into()), "兜底".into()),
            "流稿"
        );
        assert_eq!(
            pick_final_text(Some(String::new()), None, "兜底".into()),
            "兜底"
        );
        // 无离线引擎（未下载/关闭）→ 流式终稿
        assert_eq!(
            pick_final_text(None, Some("流稿".into()), "兜底".into()),
            "流稿"
        );
        // 流终稿缺失（多段同批切出 / 未认领尾巴）→ 一次性转写兜底
        assert_eq!(pick_final_text(None, None, "兜底".into()), "兜底");
    }

    // -----------------------------------------------------------------------
    // 离线高精引擎（SenseVoice）
    // -----------------------------------------------------------------------

    #[test]
    fn sense_voice_config_paths_and_flags() {
        let cfg = build_sense_voice_config(Path::new("models"));
        let sv = &cfg.model_config.sense_voice;
        assert!(sv.model.as_deref().unwrap().ends_with("model.int8.onnx"));
        assert!(sv.model.as_deref().unwrap().contains(SENSE_VOICE_DIR));
        assert_eq!(sv.language.as_deref(), Some("zh"));
        assert!(sv.use_itn);
        assert!(cfg
            .model_config
            .tokens
            .as_deref()
            .unwrap()
            .ends_with("tokens.txt"));
        assert_eq!(cfg.model_config.num_threads, 4);
    }

    #[test]
    fn sense_voice_ready_requires_both_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(!sense_voice_ready(dir)); // 目录不存在
        let sv = dir.join(SENSE_VOICE_DIR);
        std::fs::create_dir_all(&sv).unwrap();
        assert!(!sense_voice_ready(dir)); // 空目录
        std::fs::write(sv.join("model.int8.onnx"), b"x").unwrap();
        assert!(!sense_voice_ready(dir)); // 缺 tokens.txt
        std::fs::write(sv.join("tokens.txt"), b"x").unwrap();
        assert!(sense_voice_ready(dir));
        // fp32 model.onnx 不算就绪（应用只用 int8）
        let tmp2 = tempfile::tempdir().unwrap();
        let sv2 = tmp2.path().join(SENSE_VOICE_DIR);
        std::fs::create_dir_all(&sv2).unwrap();
        std::fs::write(sv2.join("model.onnx"), b"x").unwrap();
        std::fs::write(sv2.join("tokens.txt"), b"x").unwrap();
        assert!(!sense_voice_ready(tmp2.path()));
    }

    /// 增量流解码的实模型集成验证（需本地 models/ 与 test-wavs 素材，默认忽略）：
    /// `cargo test --release --lib -- --ignored segment_stream --nocapture`
    ///
    /// 验证点：
    /// 1. 100ms 块增量喂入 + 600ms 节流 partial 的每次调用只花 O(增量) 时间
    ///    （对照：一次性 transcribe_buffer 同段要花 O(段长)，且随段变长线性变慢）
    /// 2. finalize（input_finished + decode 到底）出非空终稿
    /// 3. 增量喂入与一次性喂入的结果允许略有差异（以增量为准——这正是产品路径）
    #[test]
    #[ignore = "需要本地模型与测试素材（models/、models/test-wavs/），加载耗时数秒"]
    fn segment_stream_incremental_decode_on_real_model() {
        let models_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("models");
        let recognizer = OnlineRecognizer::create(&build_asr_config(&models_dir, None))
            .expect("流式模型未就绪（先跑 scripts/download-models.ps1）");
        let wav = models_dir.join("test-wavs").join("itn-zh-number.wav");
        let decoded = crate::decode::decode_audio_file(&wav).expect("测试素材缺失");
        let samples =
            crate::audio::resample_to_16k_mono(&decoded.samples, decoded.sample_rate, decoded.channels);
        println!("素材: {:.1}s / {} 样本", samples.len() as f32 / SAMPLE_RATE as f32, samples.len());

        // 与会话主循环同节奏：100ms 块喂入；语音中每 300ms 一次 partial。
        // 本测试没有 VAD 与实时钟：素材是连续朗读，恒按「语音中」喂入；
        // 节流按内容节奏等价模拟（每 3 块 = 300ms 音频一次 partial，与实时
        // 会话的触发频率一致）。预滚/静音路径已由 stream_step / push_pre_roll 单测覆盖。
        let mut seg = SegmentStream::new();
        let mut partial_ms: Vec<u128> = Vec::new();
        let mut partial_texts: Vec<String> = Vec::new();
        for (i, chunk) in samples.chunks(SAMPLE_RATE / 10).enumerate() {
            seg.push(&recognizer, true, chunk);
            if (i + 1) % 3 == 0 {
                let t0 = Instant::now();
                let text = seg.partial(&recognizer).unwrap_or_default();
                partial_ms.push(t0.elapsed().as_millis());
                partial_texts.push(text);
            }
        }
        assert!(partial_ms.len() >= 5, "9s 素材按 300ms 节流应触发多次 partial");
        // 增量 partial 的成本应与段长无关（O(增量)）：无论已喂入 1s 还是 8s，
        // 单次都在几十 ms 量级——这正是延迟修复的直接证据
        println!("partial 次数: {} 各次耗时(ms): {:?}", partial_ms.len(), partial_ms);
        println!("partial 文本: {:?}", partial_texts);
        assert!(
            partial_ms.iter().all(|&ms| ms < 300),
            "增量 partial 不应出现整段级耗时: {partial_ms:?}"
        );

        let t0 = Instant::now();
        let final_text = seg.finalize(&recognizer).expect("定稿时流应存活");
        let finalize_ms = t0.elapsed().as_millis();
        println!("持久流终稿({finalize_ms}ms): {final_text}");
        assert!(!final_text.trim().is_empty());

        // 对照：一次性喂入（transcribe_buffer）。两者可能有细微差异——以增量为准。
        let t0 = Instant::now();
        let one_shot = transcribe_buffer(&recognizer, &samples);
        let one_shot_ms = t0.elapsed().as_millis();
        println!("一次性转写({one_shot_ms}ms): {one_shot}");
        // 两者应大致一致：取公共字符比例作宽松校验（>60%）
        let common = one_shot
            .chars()
            .filter(|c| final_text.contains(*c))
            .count();
        let denom = one_shot.chars().filter(|c| !c.is_whitespace()).count().max(1);
        let ratio = common as f64 / denom as f64;
        println!("字符重合率: {ratio:.2}");
        assert!(ratio > 0.6, "增量与一次性喂入差异过大: {final_text} vs {one_shot}");
    }
}
