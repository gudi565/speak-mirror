import { describe, it, expect } from "vitest";
import { SCENARIOS, scenarioLabel, scenarioMeta } from "./scenarios";

describe("scenarioLabel", () => {
  it("maps all four scenarios to Chinese names", () => {
    expect(scenarioLabel("free")).toBe("自由练习");
    expect(scenarioLabel("interview")).toBe("面试回答");
    expect(scenarioLabel("vlog")).toBe("口播视频");
    expect(scenarioLabel("workreport")).toBe("工作汇报");
  });

  it("maps mockInterview (模拟面试) without adding it to the selector", () => {
    expect(scenarioLabel("mockInterview")).toBe("模拟面试");
    // 四场景选择器保持四项：mockInterview 只在面试流程内部使用
    expect(SCENARIOS.map((s) => s.value)).not.toContain("mockInterview");
    // 未知值仍回落自由练习
    expect(scenarioMeta("mockInterview").value).toBe("free");
  });

  it("falls back to 自由练习 for unknown values", () => {
    expect(scenarioLabel("unknown")).toBe("自由练习");
    expect(scenarioLabel("")).toBe("自由练习");
  });
});

describe("SCENARIOS", () => {
  it("has four entries with non-empty labels and hints", () => {
    expect(SCENARIOS.map((s) => s.value)).toEqual([
      "free",
      "interview",
      "vlog",
      "workreport",
    ]);
    for (const s of SCENARIOS) {
      expect(s.label.length).toBeGreaterThan(0);
      expect(s.hint.length).toBeGreaterThan(0);
      expect(s.topicLabel).toContain("可空");
      expect(s.topicPlaceholder.length).toBeGreaterThan(0);
    }
  });
});

describe("scenarioMeta", () => {
  it("returns meta for known scenarios", () => {
    expect(scenarioMeta("vlog").label).toBe("口播视频");
    expect(scenarioMeta("workreport").topicLabel).toBe("汇报主题（可空）");
  });

  it("falls back to free for unknown values", () => {
    expect(scenarioMeta("bogus").value).toBe("free");
  });
});
