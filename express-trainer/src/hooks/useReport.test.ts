import { describe, expect, it } from "vitest";
import { generateInvokeArgs } from "./useReport";

/** mode 等参数到 generate_report 命令的传递契约（Rust parse_report_mode 对应） */
describe("generateInvokeArgs", () => {
  it("passes the report mode through, defaulting to full", () => {
    const quick = generateInvokeArgs({
      scenario: "free",
      topic: "自律",
      transcript: null,
      mode: "quick",
    });
    expect(quick.mode).toBe("quick");

    // 缺省 full（与 Rust 侧 parse_report_mode(None) 同口径）
    const bare = generateInvokeArgs({ scenario: "free", topic: "自律", transcript: "x" });
    expect(bare.mode).toBe("full");
  });

  it("trims topic and nulls blanks, nulls missing qa", () => {
    const args = generateInvokeArgs({ scenario: "interview", topic: "  面试题  ", transcript: null });
    expect(args.topic).toBe("面试题");
    expect(generateInvokeArgs({ scenario: "free", topic: "   ", transcript: null }).topic).toBeNull();
    expect(args.qa).toBeNull();
  });

  it("forwards transcript and qa payload untouched", () => {
    const qa = [{ question: "为什么离职？", intent: "动机", answer: "想找舞台。", durationMs: 45000 }];
    const args = generateInvokeArgs({
      scenario: "mockInterview",
      topic: "后端开发",
      transcript: null,
      qa,
      mode: "full",
    });
    expect(args.scenario).toBe("mockInterview");
    expect(args.transcript).toBeNull();
    expect(args.qa).toEqual(qa);
  });
});
