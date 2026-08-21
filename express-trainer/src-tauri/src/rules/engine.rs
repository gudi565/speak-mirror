use super::emotion::EmotionRule;
use super::filler::FillerWordsRule;
use super::precision::PrecisionRule;
use super::repetition::RepetitionRule;
use super::structure::{ConclusionMissingRule, ExampleMissingRule};
use super::{FeedbackEvent, FeedbackKind, Rule, Sentence, SessionContext};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub sentence_count: u64,
    pub filler_counts: Vec<(String, u32)>,
    pub filler_per_minute: f64,
    pub emotion_counts: Vec<(String, u32)>,
    pub duration_ms: u64,
}

pub struct RuleEngine {
    rules: Vec<Box<dyn Rule>>,
    ctx: SessionContext,
    last_end_ms: u64,
}

impl RuleEngine {
    pub fn new() -> Self {
        Self {
            rules: vec![
                Box::new(FillerWordsRule::default()),
                Box::new(PrecisionRule::default()),
                Box::new(RepetitionRule::default()),
                Box::new(ConclusionMissingRule::default()),
                Box::new(ExampleMissingRule::default()),
                Box::new(EmotionRule::default()),
            ],
            ctx: SessionContext::default(),
            last_end_ms: 0,
        }
    }

    pub fn start(&mut self, now_ms: u64) {
        self.ctx.started_at_ms = now_ms;
    }

    pub fn ingest(&mut self, sentence: Sentence) -> Vec<FeedbackEvent> {
        let mut events = Vec::new();
        for rule in &mut self.rules {
            events.extend(rule.on_sentence(&sentence, &self.ctx));
        }
        // 落地统计：规则只读 ctx，由 engine 统一累计
        for e in &events {
            match e.kind {
                FeedbackKind::FillerWord => {
                    let word = e.payload["word"].as_str().unwrap_or_default().to_string();
                    *self.ctx.filler_counts.entry(word).or_insert(0) += 1;
                }
                FeedbackKind::Emotion => {
                    let cat = e.payload["category"].as_str().unwrap_or_default().to_string();
                    *self.ctx.emotion_counts.entry(cat).or_insert(0) += 1;
                }
                _ => {}
            }
        }
        self.last_end_ms = sentence.end_ms;
        self.ctx.sentences.push(sentence);
        events
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let total_fillers: u32 = self.ctx.filler_counts.values().sum();
        let duration_ms = self.last_end_ms.saturating_sub(self.ctx.started_at_ms);
        let minutes = duration_ms as f64 / 60_000.0;
        let filler_per_minute = if minutes > 0.0 {
            (total_fillers as f64 / minutes * 10.0).round() / 10.0
        } else {
            0.0
        };
        let sort_desc =
            |m: &std::collections::HashMap<String, u32>| -> Vec<(String, u32)> {
                let mut v: Vec<(String, u32)> = m.clone().into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1));
                v
            };
        SessionSnapshot {
            sentence_count: self.ctx.sentences.len() as u64,
            filler_counts: sort_desc(&self.ctx.filler_counts),
            filler_per_minute,
            emotion_counts: sort_desc(&self.ctx.emotion_counts),
            duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: u64, text: &str, end_ms: u64) -> Sentence {
        Sentence { id, text: text.into(), start_ms: end_ms - 1000, end_ms }
    }

    #[test]
    fn ingest_runs_all_rules_and_accumulates() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        let events = engine.ingest(sent(1, "然后我真的很开心", 60_000));
        let kinds: Vec<FeedbackKind> = events.iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&FeedbackKind::FillerWord));
        assert!(kinds.contains(&FeedbackKind::Emotion));
        let snap = engine.snapshot();
        assert_eq!(snap.sentence_count, 1);
        assert_eq!(snap.filler_counts, vec![("然后".to_string(), 1)]);
        assert_eq!(snap.emotion_counts, vec![("喜".to_string(), 1)]);
    }

    #[test]
    fn repetition_rule_sees_previous_sentences_via_ctx() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "这个系统可以实时分析你的表达问题", 60_000));
        let events = engine.ingest(sent(2, "这个系统可以实时分析你的表达问题啊", 120_000));
        assert!(events.iter().any(|e| e.kind == FeedbackKind::Repetition));
    }

    #[test]
    fn snapshot_serializes_camel_case() {
        let mut engine = RuleEngine::new();
        engine.start(0);
        engine.ingest(sent(1, "然后", 60_000));
        let v = serde_json::to_value(engine.snapshot()).unwrap();
        assert_eq!(v["sentenceCount"], 1);
        assert!(v.get("fillerPerMinute").is_some());
    }
}
