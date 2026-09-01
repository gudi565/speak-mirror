import { describe, expect, it } from "vitest";
import {
  FULL_SECTIONS,
  QUICK_SECTIONS,
  REPORT_MODE_OPTIONS,
  layoutSkeleton,
  reportModeLabel,
  shouldUseSkeleton,
  skeletonSections,
  stripScoreMarker,
} from "./report";

describe("skeletonSections", () => {
  it("full mode has the eight contract headings in order", () => {
    expect(skeletonSections("full")).toEqual([
      "一、总评",
      "二、亮点",
      "三、逐句改写",
      "四、可替换词汇表",
      "五、行为模式分析",
      "六、怎么说的",
      "七、数据区",
      "八、下次练习重点",
    ]);
    expect(skeletonSections("full")).toEqual([...FULL_SECTIONS]);
  });

  it("quick mode has the four quick-report headings", () => {
    expect(skeletonSections("quick")).toEqual([
      "一、总评",
      "二、亮点",
      "三、最需要改进的一个问题",
      "四、下次重点",
    ]);
    expect(skeletonSections("quick")).toEqual([...QUICK_SECTIONS]);
  });
});

describe("reportModeLabel", () => {
  it("maps modes to Chinese labels with full as fallback", () => {
    expect(reportModeLabel("quick")).toBe("快速报告");
    expect(reportModeLabel("full")).toBe("完整报告");
    expect(reportModeLabel(undefined)).toBe("完整报告"); // 旧记录缺省
    expect(reportModeLabel(null)).toBe("完整报告");
    expect(reportModeLabel("turbo")).toBe("完整报告"); // 未知值兜底
  });
});

describe("REPORT_MODE_OPTIONS", () => {
  it("defaults to quick (用户痛点是慢)", () => {
    expect(REPORT_MODE_OPTIONS[0].value).toBe("quick");
    expect(REPORT_MODE_OPTIONS.map((o) => o.value)).toEqual(["quick", "full"]);
    expect(REPORT_MODE_OPTIONS[0].hint).toContain("1/3");
    expect(REPORT_MODE_OPTIONS[1].hint).toContain("八节");
  });
});

describe("stripScoreMarker", () => {
  it("removes a complete marker and collapses leftover blank lines", () => {
    const md = "## 八、下次练习重点\n1. 先给结论。\n\n<!--SCORE:{\"overall\":78}-->";
    expect(stripScoreMarker(md)).toBe("## 八、下次练习重点\n1. 先给结论。\n");
  });

  it("truncates an unclosed marker tail (流式中途)", () => {
    expect(stripScoreMarker("正文\n<!--SCORE:{\"ov")).toBe("正文");
  });

  it("leaves plain text untouched", () => {
    expect(stripScoreMarker("普通正文\n没有标记")).toBe("普通正文\n没有标记");
  });
});

describe("shouldUseSkeleton", () => {
  it("uses skeleton for empty text or h1-free AI reports", () => {
    expect(shouldUseSkeleton("")).toBe(true);
    expect(shouldUseSkeleton("   \n ")).toBe(true);
    expect(shouldUseSkeleton("## 一、总评\n内容")).toBe(true);
  });

  it("skips skeleton for local fallback reports (h1 preamble)", () => {
    expect(shouldUseSkeleton("# 表达训练 · 本地报告\n\n> 未配置 AI 后端\n\n## 数据统计")).toBe(false);
  });
});

describe("layoutSkeleton", () => {
  it("shows pure skeleton placeholders before any chunk arrives", () => {
    const layout = layoutSkeleton("", "full");
    expect(layout.preamble).toBe("");
    expect(layout.rows).toHaveLength(8);
    expect(layout.rows.every((r) => r.body === "" && !r.done)).toBe(true);
    expect(layout.rows[0].title).toBe("一、总评");
    expect(layout.doneCount).toBe(0);
    expect(layout.total).toBe(8);
  });

  it("fills sections by heading order and marks finished ones", () => {
    const text =
      "## 一、总评\n**评分：78/100（良好）**\n定位一句话。\n\n## 二、亮点\n- 「#3」引用。\n\n## 三、逐句改写\n";
    const layout = layoutSkeleton(text, "full");
    // 前三节已到：前两节完成（下一节开始），第三节进行中，其余占位
    expect(layout.rows[0].done).toBe(true);
    expect(layout.rows[0].body).toContain("78/100");
    expect(layout.rows[1].done).toBe(true);
    expect(layout.rows[1].body).toContain("「#3」");
    expect(layout.rows[2].done).toBe(false);
    expect(layout.rows[2].body).toBe("");
    expect(layout.rows[3].title).toBe("四、可替换词汇表"); // 骨架占位标题
    expect(layout.doneCount).toBe(2);
    expect(layout.total).toBe(8);
  });

  it("quick mode uses four slots and quick headings for placeholders", () => {
    const text = "## 一、总评\n快速总评。\n";
    const layout = layoutSkeleton(text, "quick");
    expect(layout.total).toBe(4);
    expect(layout.rows[1].title).toBe("二、亮点");
    expect(layout.rows[2].title).toBe("三、最需要改进的一个问题");
    expect(layout.rows[3].title).toBe("四、下次重点");
    expect(layout.doneCount).toBe(0); // 只到了第一节，尚无下一节开始
  });

  it("prefers the streamed heading over the skeleton title (mockInterview 第三节为逐题点评)", () => {
    const text = "## 一、总评\nA\n\n## 二、亮点\nB\n\n## 三、逐题点评\nC\n";
    const layout = layoutSkeleton(text, "full");
    expect(layout.rows[2].title).toBe("三、逐题点评");
    expect(layout.rows[2].done).toBe(false); // 最后一节流式中恒未完成
  });

  it("keeps preamble content (before the first ##) as an untitled lead block", () => {
    const text = "开场一段不应该出现的内容\n\n## 一、总评\n正文\n";
    const layout = layoutSkeleton(text, "full");
    expect(layout.preamble).toBe("开场一段不应该出现的内容");
    expect(layout.rows[0].body).toBe("正文");
  });

  it("strips a partially-streamed SCORE marker from the last section body", () => {
    const text =
      "## 一、总评\nA\n\n## 二、亮点\nB\n\n## 三、最需要改进的一个问题\nC\n\n## 四、下次重点\n1. 动作。\n\n<!--SCORE:{\"ov";
    const layout = layoutSkeleton(text, "quick");
    expect(layout.rows[3].title).toBe("四、下次重点");
    expect(layout.rows[3].body).toBe("1. 动作。");
    expect(layout.rows[3].done).toBe(false); // 最后一节流式中恒未完成
    expect(layout.doneCount).toBe(3);
  });

  it("extends beyond the skeleton if more headings arrive (兜底不丢内容)", () => {
    const text = ["一", "二", "三", "四", "五"].map((t) => `## ${t}\n${t}内容`).join("\n\n");
    const layout = layoutSkeleton(text, "quick");
    expect(layout.total).toBe(5);
    expect(layout.rows[4].title).toBe("五");
    expect(layout.doneCount).toBe(4);
  });
});
