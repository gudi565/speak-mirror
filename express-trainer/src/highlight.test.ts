import { describe, it, expect } from "vitest";
import { highlightFillers } from "./highlight";

describe("highlightFillers", () => {
  it("marks filler occurrences", () => {
    const parts = highlightFillers("然后我想说然后就是", ["然后", "就是"]);
    expect(parts.filter((p) => p.isFiller)).toHaveLength(3);
    expect(parts.map((p) => p.text).join("")).toBe("然后我想说然后就是");
  });

  it("returns single part when no filler", () => {
    expect(highlightFillers("干净的一句话", ["然后"])).toEqual([
      { text: "干净的一句话", isFiller: false },
    ]);
  });

  it("keeps Chinese substring behavior unchanged", () => {
    // 含 CJK 的词条保持子串匹配（回归红线）；命中顺序按文本出现顺序
    const parts = highlightFillers("然后那个然后就是", ["然后就是", "那个"]);
    expect(parts.filter((p) => p.isFiller).map((p) => p.text)).toEqual(["那个", "然后就是"]);
  });

  it("matches English fillers on word boundaries only", () => {
    // like 不命中 likely / unlike（子词不标红）
    const clean = highlightFillers("this is likely fine unlike before", ["like"]);
    expect(clean.filter((p) => p.isFiller)).toHaveLength(0);
    expect(clean.map((p) => p.text).join("")).toBe("this is likely fine unlike before");

    const parts = highlightFillers("I like it, like, a lot", ["like"]);
    expect(parts.filter((p) => p.isFiller)).toHaveLength(2);
  });

  it("matches phrases and apostrophe words as whole units", () => {
    // 短语整体匹配；so 不命中 sorted；don't 照常命中
    const parts = highlightFillers("It's sort of fine, so we sorted it, don't you think?", [
      "sort of",
      "so",
      "don't",
    ]);
    expect(parts.filter((p) => p.isFiller).map((p) => p.text)).toEqual([
      "sort of",
      "so",
      "don't",
    ]);
  });

  it("matches English case-insensitively (sentence-initial capitals)", () => {
    const parts = highlightFillers("Um, You know, this works", ["um", "you know"]);
    expect(parts.filter((p) => p.isFiller).map((p) => p.text)).toEqual(["Um", "You know"]);
  });

  it("does not let ASCII lookup collide with Chinese words", () => {
    // 小写集合比对不误伤 CJK 词条（未列入词表的「就是」不标红）
    const parts = highlightFillers("然后就是", ["然后"]);
    expect(parts.filter((p) => p.isFiller).map((p) => p.text)).toEqual(["然后"]);
    expect(parts.map((p) => p.text).join("")).toBe("然后就是");
  });
});
