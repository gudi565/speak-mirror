pub mod filler;
pub mod lexicon;
pub mod precision;
pub mod repetition;
pub mod structure;
pub mod emotion;
pub mod engine;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sentence {
    pub id: u64,
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FeedbackKind {
    FillerWord,
    WordPrecision,
    Repetition,
    ConclusionMissing,
    ExampleMissing,
    Emotion,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackEvent {
    pub kind: FeedbackKind,
    pub sentence_id: Option<u64>,
    pub message: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct SessionContext {
    pub sentences: Vec<Sentence>,
    pub filler_counts: HashMap<String, u32>,
    pub emotion_counts: HashMap<String, u32>,
    pub started_at_ms: u64,
}

pub trait Rule {
    fn on_sentence(&mut self, sentence: &Sentence, ctx: &SessionContext) -> Vec<FeedbackEvent>;
    fn name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_serializes_to_camel_case() {
        let s = Sentence { id: 1, text: "你好".into(), start_ms: 0, end_ms: 1000 };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["startMs"], 0);
        assert_eq!(v["endMs"], 1000);
    }

    #[test]
    fn feedback_event_serializes_kind_camel_case() {
        let e = FeedbackEvent {
            kind: FeedbackKind::FillerWord,
            sentence_id: Some(3),
            message: "口头禅".into(),
            payload: serde_json::json!({"word": "然后"}),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "fillerWord");
        assert_eq!(v["sentenceId"], 3);
    }
}
