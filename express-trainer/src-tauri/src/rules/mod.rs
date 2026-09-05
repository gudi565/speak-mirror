pub mod filler;
pub mod golden_quote;
pub mod hedge;
pub mod imagery;
pub mod lang;
pub mod lexicon;
pub mod precision;
pub mod repetition;
pub mod structure;
pub mod emotion;
pub mod time_vague;
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
    /// 立场模糊（单句堆叠 ≥2 个犹豫弱化词）
    Hedge,
    /// 时间模糊（最近/过几天/回头…）
    TimeVague,
    /// 画面感（比喻标记 / 抽象词具象化建议）
    Imagery,
    /// 金句候选（比喻 + 数字结论 / 对仗强调等组合信号，正向）
    GoldenQuote,
    /// AI 周期快评（TOPIC/CONTRA/WRAP）
    AiCheckin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackEvent {
    pub kind: FeedbackKind,
    pub sentence_id: Option<u64>,
    pub message: String,
    pub payload: serde_json::Value,
}

/// 情感类别累计：次数 + 强度之和（快照再算平均强度）
#[derive(Debug, Default, Clone)]
pub struct EmotionAccumulator {
    pub count: u32,
    pub intensity_sum: u32,
}

#[derive(Debug, Default)]
pub struct SessionContext {
    pub sentences: Vec<Sentence>,
    pub filler_counts: HashMap<String, u32>,
    pub emotion_counts: HashMap<String, EmotionAccumulator>,
    /// 立场模糊词（hedges）逐词计数：无论是否达到提醒阈值都计入统计
    pub hedge_counts: HashMap<String, u32>,
    /// 金句候选句数（GoldenQuote 事件计数，规则显示与否都累计）
    pub golden_quote_count: u32,
    pub started_at_ms: u64,
}

pub trait Rule: Send {
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
        let kinds = [
            (FeedbackKind::Hedge, "hedge"),
            (FeedbackKind::TimeVague, "timeVague"),
            (FeedbackKind::Imagery, "imagery"),
            (FeedbackKind::GoldenQuote, "goldenQuote"),
            (FeedbackKind::AiCheckin, "aiCheckin"),
        ];
        for (k, want) in kinds {
            let e = FeedbackEvent { kind: k, sentence_id: None, message: "x".into(), payload: serde_json::json!({}) };
            assert_eq!(serde_json::to_value(&e).unwrap()["kind"], want);
        }
    }
}
