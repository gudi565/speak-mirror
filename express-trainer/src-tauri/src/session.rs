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

const SAMPLE_RATE: usize = 16_000;
const MAX_SEGMENT_SECONDS: usize = 30;
const MAX_SEGMENT_SAMPLES: usize = SAMPLE_RATE * MAX_SEGMENT_SECONDS;
const PARTIAL_TRANSCRIPT_INTERVAL_MS: u64 = 600;
const MIN_TRAILING_SEGMENT_SECONDS: f32 = 0.3;

/// Keep the live buffer from growing without bound during long silences.
pub fn trim_segment_to_cap(segment: &mut Vec<f32>, cap_samples: usize) {
    if segment.len() > cap_samples {
        let drain = segment.len() - cap_samples;
        segment.drain(0..drain);
    }
}

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

    let app_for_finalize = app.clone();
    let mut finalize_segment = |samples: &[f32], recognizer: &mut TransducerRecognizer| {
        if samples.is_empty() {
            return;
        }
        let text = recognizer.transcribe(16_000, samples);
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        sentence_id += 1;
        let end_ms = started.elapsed().as_millis() as u64;
        let sentence = Sentence {
            id: sentence_id,
            text,
            start_ms: end_ms.saturating_sub(samples.len() as u64 / 16),
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
        match capture.rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                let samples = resample_to_16k_mono(&chunk, capture.sample_rate, capture.channels);
                current_segment.extend_from_slice(&samples);
                trim_segment_to_cap(&mut current_segment, MAX_SEGMENT_SAMPLES);
                vad.accept_waveform(samples);

                // partial：说话中每 600ms 对当前段跑一次识别
                if vad.is_speech() && last_partial.elapsed() > Duration::from_millis(PARTIAL_TRANSCRIPT_INTERVAL_MS) {
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
                    finalize_segment(&seg.samples, &mut recognizer);
                    current_segment.clear();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Final flush: emit any VAD-buffered segment and any trailing live audio
    // so the sentence being spoken is not dropped when the user hits stop.
    vad.flush();
    while !vad.is_empty() {
        let seg = vad.front();
        vad.pop();
        finalize_segment(&seg.samples, &mut recognizer);
    }
    if current_segment.len() as f32 > SAMPLE_RATE as f32 * MIN_TRAILING_SEGMENT_SECONDS {
        finalize_segment(&current_segment, &mut recognizer);
    }

    drop(capture.stream);
    Ok(())
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
}
