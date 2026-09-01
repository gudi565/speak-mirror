import { describe, expect, it } from "vitest";
import {
  INTERVIEW_ROLES,
  QUESTION_COUNTS,
  buildQaPayload,
  editAnswer,
  interviewProgress,
  interviewRoleLabel,
  isInterviewRole,
  makeAnswers,
  recordAnswer,
  summarizeAnswers,
} from "./interview";
import type { InterviewQuestion } from "../types";

function q(index: number, question: string, intent: string): InterviewQuestion {
  return { index, question, intent };
}

const QUESTIONS: InterviewQuestion[] = [
  q(1, "请自我介绍。", "开场定位"),
  q(2, "讲一次失败经历。", "复盘"),
  q(3, "为什么离职？", "动机"),
];

describe("INTERVIEW_ROLES / 白名单", () => {
  it("has seven roles with Chinese labels and hints", () => {
    expect(INTERVIEW_ROLES.map((r) => r.value)).toEqual([
      "general",
      "backend",
      "frontend",
      "product",
      "ops",
      "management",
      "custom",
    ]);
    for (const r of INTERVIEW_ROLES) {
      expect(r.label.length).toBeGreaterThan(0);
      expect(r.hint.length).toBeGreaterThan(0);
    }
    expect(QUESTION_COUNTS).toEqual([3, 5, 8]);
  });

  it("isInterviewRole accepts whitelist and rejects others", () => {
    expect(isInterviewRole("backend")).toBe(true);
    expect(isInterviewRole("custom")).toBe(true);
    expect(isInterviewRole("hr")).toBe(false);
    expect(isInterviewRole("")).toBe(false);
  });

  it("interviewRoleLabel maps and falls back to 通用面试", () => {
    expect(interviewRoleLabel("backend")).toBe("后端开发");
    expect(interviewRoleLabel("general")).toBe("通用面试");
    expect(interviewRoleLabel("bogus")).toBe("通用面试");
  });
});

describe("interviewProgress（题目进度）", () => {
  it("labels and walks through questions", () => {
    const p0 = interviewProgress(0, 3);
    expect(p0.label).toBe("第 1 / 3 题");
    expect(p0.isLast).toBe(false);
    expect(p0.nextIndex).toBe(1);

    const p2 = interviewProgress(2, 3);
    expect(p2.label).toBe("第 3 / 3 题");
    expect(p2.isLast).toBe(true);
    expect(p2.nextIndex).toBeNull();
  });

  it("clamps out-of-range indices", () => {
    expect(interviewProgress(-1, 3).current).toBe(0);
    expect(interviewProgress(99, 3).current).toBe(2);
    expect(interviewProgress(99, 3).isLast).toBe(true);
    expect(interviewProgress(0, 0).nextIndex).toBeNull();
  });
});

describe("答案累积（makeAnswers / recordAnswer / editAnswer）", () => {
  it("records answer text and duration per question", () => {
    const answers = makeAnswers(3);
    expect(answers.texts).toEqual(["", "", ""]);
    const a1 = recordAnswer(answers, 0, "第一句\n第二句", 63_400);
    expect(a1.texts[0]).toBe("第一句\n第二句");
    expect(a1.durations[0]).toBe(63_400);
    // 原容器不被修改（不可变更新）
    expect(answers.texts[0]).toBe("");
    // 修改其他题互不影响
    const a2 = recordAnswer(a1, 2, "最后一题", 5_000);
    expect(a2.texts[1]).toBe("");
    expect(a2.durations[2]).toBe(5_000);
  });

  it("ignores out-of-range indices and negative durations", () => {
    const answers = makeAnswers(2);
    expect(recordAnswer(answers, 5, "x", 1)).toEqual(answers);
    expect(recordAnswer(answers, -1, "x", 1)).toEqual(answers);
    expect(recordAnswer(answers, 0, "x", -100).durations[0]).toBe(0);
  });

  it("editAnswer updates summary-page text only", () => {
    const answers = recordAnswer(makeAnswers(2), 0, "原稿", 10_000);
    const edited = editAnswer(answers, 0, "修正稿");
    expect(edited.texts[0]).toBe("修正稿");
    expect(edited.durations[0]).toBe(10_000); // 时长保留
  });
});

describe("summarizeAnswers / buildQaPayload", () => {
  it("counts answered, skipped and total chars (whitespace excluded)", () => {
    const answers = {
      texts: ["你好 世界", "", "   "],
      durations: [10_000, 0, 0],
    };
    expect(summarizeAnswers(answers)).toEqual({ answered: 1, skipped: 2, totalChars: 4 });
  });

  it("builds qa payload aligned with questions, null when nothing answered", () => {
    const answers = makeAnswers(3);
    expect(buildQaPayload(QUESTIONS, answers)).toBeNull(); // 全空 → 不可生成报告

    const recorded = recordAnswer(
      recordAnswer(answers, 0, "回答一", 61_000),
      1,
      "回答二\n第二句",
      0,
    );
    const qa = buildQaPayload(QUESTIONS, recorded);
    expect(qa).not.toBeNull();
    expect(qa).toHaveLength(3);
    expect(qa![0]).toEqual({
      question: "请自我介绍。",
      intent: "开场定位",
      answer: "回答一",
      durationMs: 61_000,
    });
    expect(qa![1].answer).toBe("回答二\n第二句");
    expect(qa![1].durationMs).toBeNull(); // 时长未知 → null
    expect(qa![2].answer).toBe(""); // 跳过的题保留题面（后端标 skipped）
  });
});
