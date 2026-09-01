import { describe, expect, it } from "vitest";
import {
  MIN_SLICE_DURATION_SEC,
  SLICE_TAIL_PAD_SEC,
  playbackAvailable,
  playGlyph,
  playReducer,
  sentenceSlice,
} from "./playback";

describe("sentenceSlice", () => {
  it("converts ms to seconds with tail padding", () => {
    // 1.2s → 4.8s 的句子：从 1.2s 播到 4.8s + 0.2s 余量 = 3.8s
    const s = sentenceSlice(1200, 4800);
    expect(s.offsetSec).toBeCloseTo(1.2);
    expect(s.durationSec).toBeCloseTo(3.6 + SLICE_TAIL_PAD_SEC);
  });

  it("clamps negative start to zero", () => {
    const s = sentenceSlice(-500, 1500);
    expect(s.offsetSec).toBe(0);
    expect(s.durationSec).toBeCloseTo(1.5 + SLICE_TAIL_PAD_SEC);
  });

  it("clamps to total duration when slice would overrun the file", () => {
    // 句子到 59.9s，但录音只有 60s：余量后是 60.1s → 夹到 60
    const s = sentenceSlice(58_000, 59_900, 60);
    expect(s.offsetSec).toBeCloseTo(58);
    expect(s.durationSec).toBeCloseTo(2.0);
    // 句子本身越界（时间戳异常）：不产生负时长
    const weird = sentenceSlice(59_000, 120_000, 60);
    expect(weird.offsetSec + weird.durationSec).toBeCloseTo(60);
    expect(weird.durationSec).toBeGreaterThan(0);
  });

  it("enforces minimum duration for degenerate ranges", () => {
    // end == start：只剩余量；end < start：至少最短时长
    const flat = sentenceSlice(1000, 1000);
    expect(flat.durationSec).toBeCloseTo(SLICE_TAIL_PAD_SEC);
    // start 已在录音末尾（时间戳异常）：仍给最短时长（播放器会立刻自然结束）
    const inverted = sentenceSlice(1000, 1000, 1.0);
    expect(inverted.offsetSec).toBeCloseTo(1.0);
    expect(inverted.durationSec).toBeCloseTo(MIN_SLICE_DURATION_SEC);
  });

  it("ignores invalid totalSec", () => {
    for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, 0, -3]) {
      const s = sentenceSlice(0, 1000, bad);
      expect(s.durationSec).toBeCloseTo(1 + SLICE_TAIL_PAD_SEC);
    }
  });
});

describe("playReducer", () => {
  it("starts and stops a sentence", () => {
    let s = playReducer({ playingId: null }, { type: "start", id: 3 });
    expect(s).toEqual({ playingId: 3 });
    s = playReducer(s, { type: "stop" });
    expect(s).toEqual({ playingId: null });
  });

  it("switches directly to another sentence", () => {
    let s = playReducer({ playingId: 1 }, { type: "start", id: 2 });
    expect(s).toEqual({ playingId: 2 });
  });

  it("natural end only clears the still-current sentence", () => {
    // 句 1 播完时已经切到句 2：不能把句 2 的状态清掉
    let s = playReducer({ playingId: 2 }, { type: "ended", id: 1 });
    expect(s).toEqual({ playingId: 2 });
    // 播完的正是当前句：清掉
    s = playReducer(s, { type: "ended", id: 2 });
    expect(s).toEqual({ playingId: null });
  });
});

describe("playGlyph", () => {
  it("shows pause only for the playing sentence", () => {
    const state = { playingId: 7 };
    expect(playGlyph(state, 7)).toBe("⏸");
    expect(playGlyph(state, 8)).toBe("▶");
    expect(playGlyph({ playingId: null }, 7)).toBe("▶");
  });
});

describe("playbackAvailable", () => {
  it("needs both an audio file and sentences", () => {
    expect(playbackAvailable(null, 5)).toBe(false);
    expect(playbackAvailable("/x/y.wav", 0)).toBe(false);
    expect(playbackAvailable("/x/y.wav", 5)).toBe(true);
    expect(playbackAvailable(undefined, 1)).toBe(false);
  });
});
