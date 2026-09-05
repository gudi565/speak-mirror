import { describe, expect, it } from "vitest";
import { cjkRatio, sentencesCjkRatio } from "./lang";

describe("cjkRatio", () => {
  it("returns 1 for pure Chinese text (punctuation and digits excluded)", () => {
    expect(cjkRatio("今天介绍产品设计的三个要点")).toBe(1);
    expect(cjkRatio("这个方案，好！")).toBe(1);
  });

  it("returns 0 for pure English text", () => {
    expect(cjkRatio("I think this is a great idea")).toBe(0);
  });

  it("computes the mixed ratio over CJK + latin letters only", () => {
    // 3 个汉字 + 7 个字母 = 0.3；数字与标点不参与
    expect(cjkRatio("中文句 abcdefg，123！")).toBeCloseTo(0.3);
    // 5 汉字 + 5 字母 = 0.5
    expect(cjkRatio("五个汉字啊abcde")).toBeCloseTo(0.5);
  });

  it("returns 0 for content without letters (digits / symbols / blank)", () => {
    expect(cjkRatio("123 456")).toBe(0);
    expect(cjkRatio("……？！")).toBe(0);
    expect(cjkRatio("")).toBe(0);
  });
});

describe("sentencesCjkRatio", () => {
  const s = (text: string) => ({ text });

  it("aggregates across sentences", () => {
    // 句 1 纯英文、句 2 纯中文 → 合并占比介于 0 与 1 之间
    const ratio = sentencesCjkRatio([s("Some english words here"), s("这句是中文")]);
    expect(ratio).not.toBeNull();
    expect(ratio as number).toBeGreaterThan(0);
    expect(ratio as number).toBeLessThan(1);
  });

  it("marks English sessions below the threshold", () => {
    const ratio = sentencesCjkRatio([
      s("So today I want to talk about my weekly report"),
      s("Basically we shipped three features last week"),
    ]);
    expect(ratio).toBe(0);
  });

  it("marks Chinese sessions above the threshold", () => {
    const ratio = sentencesCjkRatio([s("这周我们上线了三个功能"), s("所以整体进展顺利")]);
    expect(ratio).toBe(1);
  });

  it("returns null for empty sessions or letter-less content (treated as Chinese)", () => {
    expect(sentencesCjkRatio([])).toBeNull();
    expect(sentencesCjkRatio([s("123 456")])).toBeNull();
  });
});
