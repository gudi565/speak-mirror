use crate::audio::{resample_to_16k_mono, start_capture};
use crate::rules::engine::RuleEngine;
use crate::rules::Sentence;
use sherpa_rs::silero_vad::{SileroVad, SileroVadConfig};
use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Emitter;

pub fn run_session(
    app: tauri::AppHandle,
    stop_rx: Receiver<()>,
    engine: Arc<Mutex<RuleEngine>>,
    models_dir: PathBuf,
) -> Result<(), String> {
    let vad_cfg = SileroVadConfig {
        model: models_dir.join("silero_vad.onnx").to_string_lossy().into_owned(),
        min_silence_duration: 0.5,
        min_speech_duration: 0.25,
        max_speech_duration: 20.0,
        threshold: 0.5,
        sample_rate: 16_000,
        window_size: 512,
        provider: None,
        num_threads: Some(1),
        debug: false,
    };
    let mut vad = SileroVad::new(vad_cfg, 30.0).map_err(|e| format!("VAD 初始化失败: {e}"))?;

    let zip = models_dir.join("zipformer");
    let asr_cfg = TransducerConfig {
        encoder: zip.join("encoder.onnx").to_string_lossy().into_owned(),
        decoder: zip.join("decoder.onnx").to_string_lossy().into_owned(),
        joiner: zip.join("joiner.onnx").to_string_lossy().into_owned(),
        tokens: zip.join("tokens.txt").to_string_lossy().into_owned(),
        num_threads: 2,
        sample_rate: 16_000,
        feature_dim: 80,
        decoding_method: "greedy_search".into(),
        hotwords_file: String::new(),
        hotwords_score: 1.5,
        modeling_unit: String::new(),
        bpe_vocab: String::new(),
        blank_penalty: 0.0,
        model_type: "zipformer".into(),
        debug: false,
        provider: None,
    };
    let mut recognizer =
        TransducerRecognizer::new(asr_cfg).map_err(|e| format!("ASR 初始化失败（模型缺失？请先运行 scripts/download-models.ps1）: {e}"))?;

    let capture = start_capture()?;
    let started = Instant::now();
    let mut sentence_id: u64 = 0;
    let mut current_segment: Vec<f32> = Vec::new();
    let mut last_partial = Instant::now();

    engine.lock().unwrap().start(0);

    loop {
        if stop_rx.try_recv().is_ok() {
            break;
        }
        match capture.rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                let samples = resample_to_16k_mono(&chunk, capture.sample_rate, capture.channels);
                current_segment.extend_from_slice(&samples);
                vad.accept_waveform(samples);

                // partial：说话中每 600ms 对当前段跑一次识别
                if vad.is_speech() && last_partial.elapsed() > Duration::from_millis(600) {
                    last_partial = Instant::now();
                    let text = recognizer.transcribe(16_000, &current_segment);
                    if !text.trim().is_empty() {
                        let _ = app.emit("partial_transcript", serde_json::json!({ "text": text }));
                    }
                }

                // 句子定稿
                while !vad.is_empty() {
                    let seg = vad.front();
                    vad.pop();
                    let text = recognizer.transcribe(16_000, &seg.samples);
                    let text = text.trim().to_string();
                    current_segment.clear();
                    if text.is_empty() {
                        continue;
                    }
                    sentence_id += 1;
                    let end_ms = started.elapsed().as_millis() as u64;
                    let sentence = Sentence {
                        id: sentence_id,
                        text,
                        start_ms: end_ms.saturating_sub(seg.samples.len() as u64 / 16),
                        end_ms,
                    };
                    let (events, snapshot) = {
                        let mut eng = engine.lock().unwrap();
                        let events = eng.ingest(sentence.clone());
                        (events, eng.snapshot())
                    };
                    let _ = app.emit("sentence_final", &sentence);
                    let _ = app.emit(
                        "analysis_update",
                        serde_json::json!({ "events": events, "snapshot": snapshot }),
                    );
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(capture.stream);
    Ok(())
}
