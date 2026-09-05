import { describe, expect, it } from "vitest";
import { SHAPE_CN, TONE_CN, toneFlagText, toneNoteText, tonePanelState } from "./tone";
import type { ToneFlag } from "../types";

function flag(overrides: Partial<ToneFlag> = {}): ToneFlag {
  return {
    sentenceId: 3,
    charIndex: 2,
    char: "妈",
    expectedTone: 1,
    detectedShape: 4,
    ...overrides,
  };
}

describe("toneFlagText", () => {
  it("renders the full hint with sentence id, char, expected tone and heard shape", () => {
    expect(toneFlagText(flag())).toBe("第 3 句「妈」应为一声（听感偏降调）");
  });

  it("covers all tones and shapes", () => {
    // 声调 1–5 与形状 1–5 的完整组合文案
    expect(toneFlagText(flag({ expectedTone: 2, detectedShape: 1 }))).toContain("应为二声（听感偏高平）");
    expect(toneFlagText(flag({ expectedTone: 3, detectedShape: 2 }))).toContain("应为三声（听感偏升调）");
    expect(toneFlagText(flag({ expectedTone: 4, detectedShape: 3 }))).toContain("应为四声（听感偏降升）");
    expect(toneFlagText(flag({ expectedTone: 1, detectedShape: 4 }))).toContain("应为一声（听感偏降调）");
    expect(toneFlagText(flag({ expectedTone: 5, detectedShape: 2 }))).toContain("应为轻声");
  });

  it("falls back to raw numbers for unknown tone/shape values", () => {
    expect(toneFlagText(flag({ expectedTone: 9, detectedShape: 8 }))).toBe(
      "第 3 句「妈」应为9声（听感偏8）",
    );
  });

  it("label tables cover exactly 1-5", () => {
    expect(Object.keys(TONE_CN).sort()).toEqual(["1", "2", "3", "4", "5"]);
    expect(Object.keys(SHAPE_CN).sort()).toEqual(["1", "2", "3", "4", "5"]);
  });
});

describe("toneNoteText", () => {
  it("passes backend rule notes through unchanged", () => {
    expect(toneNoteText(flag({ note: "三声连读，前字应读作二声（升）" }))).toBe(
      "三声连读，前字应读作二声（升）",
    );
    expect(toneNoteText(flag({ note: "一声应保持高平（音域上半区），实测位于音域底部" }))).toBe(
      "一声应保持高平（音域上半区），实测位于音域底部",
    );
  });

  it("returns null for missing note (legacy records and plain deviations)", () => {
    const { note: _note, ...legacyFlag } = flag();
    expect(toneNoteText(legacyFlag as ToneFlag)).toBeNull();
  });

  it("returns null for null/undefined/blank notes", () => {
    expect(toneNoteText(flag({ note: undefined }))).toBeNull();
    expect(toneNoteText(flag({ note: null }))).toBeNull();
    expect(toneNoteText(flag({ note: "   " }))).toBeNull();
    expect(toneNoteText(flag({ note: "" }))).toBeNull();
  });
});

describe("tonePanelState", () => {
  it("hides entirely when tone check is off", () => {
    expect(tonePanelState({ toneCheck: false, audioPath: "/a.wav", toneFlags: [flag()] })).toBe(
      "hidden",
    );
    expect(tonePanelState({ toneCheck: false, audioPath: null, toneFlags: null })).toBe("hidden");
  });

  it("shows no-audio state when there is no recording (even before any event)", () => {
    expect(tonePanelState({ toneCheck: true, audioPath: null, toneFlags: null })).toBe("noAudio");
    expect(tonePanelState({ toneCheck: true, audioPath: null, toneFlags: [] })).toBe("noAudio");
  });

  it("shows analyzing while waiting for tone_update", () => {
    expect(tonePanelState({ toneCheck: true, audioPath: "/a.wav", toneFlags: null })).toBe(
      "analyzing",
    );
  });

  it("shows clean after analysis found nothing", () => {
    expect(tonePanelState({ toneCheck: true, audioPath: "/a.wav", toneFlags: [] })).toBe("clean");
  });

  it("shows flags when analysis found deviations", () => {
    expect(
      tonePanelState({ toneCheck: true, audioPath: "/a.wav", toneFlags: [flag(), flag()] }),
    ).toBe("flags");
  });
});
