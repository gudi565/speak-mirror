export interface Sentence {
  id: number;
  text: string;
  startMs: number;
  endMs: number;
}

export type FeedbackKind =
  | "fillerWord"
  | "wordPrecision"
  | "repetition"
  | "conclusionMissing"
  | "exampleMissing"
  | "emotion";

export interface FeedbackEvent {
  kind: FeedbackKind;
  sentenceId: number | null;
  message: string;
  payload: Record<string, unknown>;
}

export interface SessionSnapshot {
  sentenceCount: number;
  fillerCounts: [string, number][];
  fillerPerMinute: number;
  emotionCounts: [string, number][];
  durationMs: number;
}
