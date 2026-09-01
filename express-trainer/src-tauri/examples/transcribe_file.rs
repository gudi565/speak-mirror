//! A/B 准确率对比：同一段 wav 整段喂给两个流式引擎，分别打印转写结果。
//!
//! 旧引擎 = 流式 Zipformer 中英双语（models/zipformer，fp32，sherpa-onnx 时代产物）
//! 新引擎 = 流式 Paraformer 中英双语 int8（models/sherpa-onnx-streaming-paraformer-bilingual-zh-en）
//!
//! 用法：
//! ```text
//! cargo run --release --example transcribe_file -- <wav 路径> [models 目录]
//! ```
//! models 目录缺省为 `<crate>/../models`。

use sherpa_onnx::{
    OnlineModelConfig, OnlineParaformerModelConfig, OnlineRecognizer, OnlineRecognizerConfig,
    OnlineTransducerModelConfig, Wave,
};
use std::path::{Path, PathBuf};
use std::time::Instant;

const PARA_DIR: &str = "sherpa-onnx-streaming-paraformer-bilingual-zh-en";

fn transcribe(recognizer: &OnlineRecognizer, wave: &Wave) -> String {
    let stream = recognizer.create_stream();
    stream.accept_waveform(wave.sample_rate(), wave.samples());
    stream.input_finished();
    while recognizer.is_ready(&stream) {
        recognizer.decode(&stream);
    }
    recognizer
        .get_result(&stream)
        .map(|r| r.text)
        .unwrap_or_default()
}

fn zipformer_recognizer(zip: &Path) -> Option<OnlineRecognizer> {
    let cfg = OnlineRecognizerConfig {
        model_config: OnlineModelConfig {
            transducer: OnlineTransducerModelConfig {
                encoder: Some(zip.join("encoder.onnx").to_string_lossy().into_owned()),
                decoder: Some(zip.join("decoder.onnx").to_string_lossy().into_owned()),
                joiner: Some(zip.join("joiner.onnx").to_string_lossy().into_owned()),
            },
            tokens: Some(zip.join("tokens.txt").to_string_lossy().into_owned()),
            num_threads: 2,
            ..Default::default()
        },
        decoding_method: Some("greedy_search".into()),
        ..Default::default()
    };
    OnlineRecognizer::create(&cfg)
}

fn paraformer_recognizer(para: &Path) -> Option<OnlineRecognizer> {
    let cfg = OnlineRecognizerConfig {
        model_config: OnlineModelConfig {
            paraformer: OnlineParaformerModelConfig {
                encoder: Some(para.join("encoder.int8.onnx").to_string_lossy().into_owned()),
                decoder: Some(para.join("decoder.int8.onnx").to_string_lossy().into_owned()),
            },
            tokens: Some(para.join("tokens.txt").to_string_lossy().into_owned()),
            num_threads: 2,
            ..Default::default()
        },
        decoding_method: Some("greedy_search".into()),
        ..Default::default()
    };
    OnlineRecognizer::create(&cfg)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("用法: cargo run --release --example transcribe_file -- <wav 路径> [models 目录]");
        std::process::exit(2);
    }
    let wav_path = args[1].clone();
    let models_dir = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("models"));

    let wave = Wave::read(&wav_path)
        .unwrap_or_else(|| panic!("无法读取 wav 文件: {wav_path}"));
    eprintln!(
        "wav: {wav_path}  采样率 {} Hz  时长 {:.1}s",
        wave.sample_rate(),
        wave.num_samples() as f32 / wave.sample_rate() as f32
    );

    let zip = models_dir.join("zipformer");
    match zipformer_recognizer(&zip) {
        Some(recognizer) => {
            let t = Instant::now();
            let text = transcribe(&recognizer, &wave);
            println!(
                "===== Zipformer（旧引擎, {:.2}s）=====",
                t.elapsed().as_secs_f32()
            );
            println!("{text}");
            println!();
        }
        None => println!("===== Zipformer 模型缺失，跳过（{}）=====", zip.display()),
    }

    let para = models_dir.join(PARA_DIR);
    match paraformer_recognizer(&para) {
        Some(recognizer) => {
            let t = Instant::now();
            let text = transcribe(&recognizer, &wave);
            println!(
                "===== Paraformer（新引擎, {:.2}s）=====",
                t.elapsed().as_secs_f32()
            );
            println!("{text}");
            println!();
        }
        None => println!(
            "===== Paraformer 模型缺失，跳过（{}）=====",
            para.display()
        ),
    }
}
