import { describe, expect, it } from "vitest";
import {
  appendFollowupChunk,
  beginFollowup,
  failFollowup,
  finishFollowup,
  followupInvokeArgs,
  followupSuggestions,
  isNoBackendError,
  retryFollowupAt,
} from "./followup";

/** followup_question 命令的 invoke 参数契约（Rust report.rs followup_question 对应） */
describe("followupInvokeArgs", () => {
  it("trims the question and passes report/scenario untouched", () => {
    expect(followupInvokeArgs("  为什么？ ", "# 报告", "interview")).toEqual({
      question: "为什么？",
      reportText: "# 报告",
      scenario: "interview",
    });
  });

  it("blank questions trim to empty (hook/命令侧拒绝发送)", () => {
    expect(followupInvokeArgs("   ", "R", "free").question).toBe("");
  });
});

/** 追问建议 chips：按场景固定 3 条 */
describe("followupSuggestions", () => {
  it("gives exactly 3 unique suggestions for every scenario", () => {
    const scenarios = ["free", "interview", "vlog", "workreport", "mockInterview"] as const;
    for (const scenario of scenarios) {
      const list = followupSuggestions(scenario);
      expect(list).toHaveLength(3);
      expect(new Set(list).size).toBe(3);
      for (const s of list) {
        expect(s.trim()).toBe(s); // 无需二次裁剪，可直接填入输入框
        expect(s.length).toBeGreaterThan(3);
      }
    }
  });

  it("covers the spec examples for free / interview", () => {
    expect(followupSuggestions("free")).toContain("我最需要改的一个习惯是什么？");
    expect(followupSuggestions("interview")).toContain("我的 STAR 哪个环节最弱？");
  });
});

describe("isNoBackendError", () => {
  it("flags missing-backend errors only (UI 换配置引导按钮)", () => {
    expect(isNoBackendError("未配置 API Key（或 Ollama 本地服务），追问需要 AI 后端——请到「设置 → AI 后端」配置")).toBe(true);
    expect(isNoBackendError("后端地址未配置（自定义后端需填写 baseURL）")).toBe(true);
    expect(isNoBackendError("HTTP 500：服务器错误")).toBe(false);
    expect(isNoBackendError("请求失败：连接超时")).toBe(false);
  });
});

/** 问答对状态机：事件乱序 / 重复到达幂等 */
describe("followup 状态机", () => {
  it("begin → chunk 追加 → done 落定全文；后续迟到事件为 no-op", () => {
    let items = beginFollowup([], "第一问");
    expect(items).toEqual([{ question: "第一问", answer: "", error: null, streaming: true }]);
    items = appendFollowupChunk(items, "回答");
    items = appendFollowupChunk(items, "前半");
    expect(items[0].answer).toBe("回答前半");
    expect(items[0].streaming).toBe(true);
    items = finishFollowup(items, "回答前半。全文");
    expect(items[0]).toMatchObject({ answer: "回答前半。全文", streaming: false, error: null });
    // done 之后的迟到 chunk / done：原样返回（引用不变）
    expect(appendFollowupChunk(items, "迟到")).toBe(items);
    expect(finishFollowup(items, "再次落定")).toBe(items);
    expect(failFollowup(items, "迟到错误")).toBe(items);
  });

  it("多轮追问：新一轮独立追加，历史问答完整保留", () => {
    let items = beginFollowup([], "Q1");
    items = finishFollowup(items, "A1");
    items = beginFollowup(items, "Q2");
    expect(items.map((x) => x.question)).toEqual(["Q1", "Q2"]);
    expect(items[1].streaming).toBe(true);
    items = appendFollowupChunk(items, "A2片段");
    expect(items[0]).toMatchObject({ answer: "A1", streaming: false });
    expect(items[1].answer).toBe("A2片段");
  });

  it("无流式条目时 chunk 为 no-op（孤立/迟到事件不产生空洞条目）", () => {
    expect(appendFollowupChunk([], "孤儿")).toEqual([]);
    const settled = finishFollowup(beginFollowup([], "Q"), "A");
    expect(appendFollowupChunk(settled, "孤儿")).toBe(settled);
  });

  it("fail 标记错误；retry 重置该条目并清空旧答案", () => {
    let items = beginFollowup([], "Q1");
    items = appendFollowupChunk(items, "半截");
    items = failFollowup(items, "请求失败");
    expect(items[0]).toMatchObject({ answer: "半截", error: "请求失败", streaming: false });
    // 已落定后 fail 不再改写（迟到错误）
    expect(failFollowup(items, "再错")).toBe(items);
    const retried = retryFollowupAt(items, 0);
    expect(retried).toEqual([{ question: "Q1", answer: "", error: null, streaming: true }]);
    // 不可重试：流式中 / 未失败 / 越界
    expect(retryFollowupAt(retried!, 0)).toBeNull();
    expect(retryFollowupAt(finishFollowup(beginFollowup([], "Q"), "A"), 0)).toBeNull();
    expect(retryFollowupAt([], 0)).toBeNull();
    expect(retryFollowupAt([], 5)).toBeNull();
  });
});
