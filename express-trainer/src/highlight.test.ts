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
});
