import { describe, expect, it } from "vitest";
import {
  fmtDuration,
  goalStatus,
  goalY,
  overallScore,
  scoreSeries,
  scoreTone,
  sourceBadge,
  sourceLabel,
  sparklinePoints,
} from "./history";

describe("sparklinePoints", () => {
  it("maps an increasing series left to right, rising", () => {
    const pts = sparklinePoints([1, 2, 3], 100, 50);
    expect(pts).toHaveLength(3);
    expect(pts[0].x).toBeCloseTo(6);
    expect(pts[2].x).toBeCloseTo(94);
    expect(pts[0].y).toBeCloseTo(44); // 最小值在底部
    expect(pts[2].y).toBeCloseTo(6); // 最大值在顶部
    // 严格递减的 y（值越大 y 越小）
    expect(pts[0].y).toBeGreaterThan(pts[1].y);
    expect(pts[1].y).toBeGreaterThan(pts[2].y);
  });

  it("renders a flat series as a centered horizontal line", () => {
    const pts = sparklinePoints([5, 5, 5, 5], 120, 40);
    expect(pts.every((p) => p.y === 20)).toBe(true);
  });

  it("renders a single point centered", () => {
    const pts = sparklinePoints([7], 120, 40);
    expect(pts).toEqual([{ x: 60, y: 20 }]);
  });

  it("returns empty for empty series", () => {
    expect(sparklinePoints([], 100, 40)).toEqual([]);
  });
});

describe("goalY", () => {
  it("places the goal between the series values", () => {
    const values = [2, 4, 6]; // min 2 max 6 span 4
    // goal 4 = 中点 → y = 6 + 28*0.5 = 20（height 40, pad 6）
    expect(goalY(values, 4, 40)).toBeCloseTo(20);
  });

  it("is null without values or invalid goal", () => {
    expect(goalY([], 3, 40)).toBeNull();
    expect(goalY([1, 2], Number.NaN, 40)).toBeNull();
  });
});

describe("goalStatus", () => {
  it("met when current at or below goal", () => {
    expect(goalStatus(2, 3)).toEqual({ met: true, diff: -1 });
    expect(goalStatus(3, 3)).toEqual({ met: true, diff: 0 });
  });

  it("diff is positive and rounded when above goal", () => {
    expect(goalStatus(3.44, 2)).toEqual({ met: false, diff: 1.4 });
    expect(goalStatus(6, 2)).toEqual({ met: false, diff: 4 });
  });
});

describe("fmtDuration", () => {
  it("formats minutes and seconds", () => {
    expect(fmtDuration(0)).toBe("0 秒");
    expect(fmtDuration(59_000)).toBe("59 秒");
    expect(fmtDuration(187_400)).toBe("3 分 07 秒");
  });
});

describe("sourceBadge / sourceLabel", () => {
  it("file sessions get the file badge", () => {
    expect(sourceBadge("file")).toBe("🎧");
    expect(sourceLabel("file")).toBe("从文件练习");
  });

  it("mic sessions (and legacy/undefined) get the mic badge", () => {
    expect(sourceBadge("mic")).toBe("🎤");
    expect(sourceLabel("mic")).toBe("实时练习");
    // 旧记录 / 缺省值按实时处理
    expect(sourceBadge(undefined)).toBe("🎤");
    expect(sourceBadge(null)).toBe("🎤");
    expect(sourceLabel(undefined)).toBe("实时练习");
  });
});

describe("overallScore / scoreTone / scoreSeries（评分入档）", () => {
  it("overallScore reads scores.overall and tolerates missing/invalid", () => {
    expect(overallScore({ scores: { overall: 78, 表达效率: 80 } })).toBe(78);
    expect(overallScore({ scores: {} })).toBeNull();
    expect(overallScore({ scores: null })).toBeNull();
    expect(overallScore({})).toBeNull();
    expect(overallScore({ scores: { overall: Number.NaN } })).toBeNull();
    expect(overallScore({ scores: { 表达效率: 80 } })).toBeNull();
  });

  it("scoreTone buckets by score band", () => {
    expect(scoreTone(90)).toContain("green");
    expect(scoreTone(75)).toContain("blue");
    expect(scoreTone(60)).toContain("amber");
    expect(scoreTone(30)).toContain("neutral");
  });

  it("scoreSeries skips records without scores (趋势图跳点)", () => {
    // 输入按时间正序（与 HistoryView 的 chrono 一致）
    const summaries: { date: string; scores?: Record<string, number> | null }[] = [
      { date: "2026-08-27 10:00:00", scores: null }, // 旧记录
      { date: "2026-08-28 10:00:00", scores: { overall: 65, 结构: 60 } },
      { date: "2026-08-29 10:00:00" }, // 本地降级：无评分
      { date: "2026-08-30 10:00:00", scores: { overall: 72 } },
    ];
    const series = scoreSeries(summaries);
    expect(series.values).toEqual([65, 72]);
    expect(series.dates).toEqual(["2026-08-28 10:00:00", "2026-08-30 10:00:00"]);
    // 全部无评分 → 空序列（第四条折线隐藏）
    expect(scoreSeries([{ date: "d" }, { date: "d2", scores: null }])).toEqual({
      values: [],
      dates: [],
    });
  });
});
