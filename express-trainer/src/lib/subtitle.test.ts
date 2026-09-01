import { describe, expect, it } from "vitest";
import { shouldShowPartial } from "./subtitle";

describe("shouldShowPartial", () => {
  const last = "今天我们聊聊产品规划。";

  it("设置关闭时完全不显示 partial 行", () => {
    expect(shouldShowPartial("新产品思路", last, false)).toBe(false);
    expect(shouldShowPartial("新产品思路", null, false)).toBe(false);
    expect(shouldShowPartial("", last, false)).toBe(false);
  });

  it("空白 partial（尚无语音 / 会话结束清空）不显示", () => {
    expect(shouldShowPartial("", last, true)).toBe(false);
    expect(shouldShowPartial("   ", last, true)).toBe(false);
    expect(shouldShowPartial("", null, true)).toBe(false);
  });

  it("与最近定稿句完全相同时不显示（SenseVoice 定稿后流式滞后重吐）", () => {
    expect(shouldShowPartial(last, last, true)).toBe(false);
    // 首尾空白不算新内容
    expect(shouldShowPartial(`  ${last}  `, last, true)).toBe(false);
  });

  it("partial 整段包含在最近定稿句里（无新内容）时不显示", () => {
    expect(shouldShowPartial("今天我们聊聊", last, true)).toBe(false);
    expect(shouldShowPartial("产品规划。", last, true)).toBe(false);
    expect(shouldShowPartial("聊聊产品", last, true)).toBe(false);
  });

  it("partial 带来定稿句之外的新内容时显示（可与定稿句开头重叠）", () => {
    expect(shouldShowPartial(`${last}接下来讲 roadmap`, last, true)).toBe(true);
    expect(shouldShowPartial("接下来讲 roadmap", last, true)).toBe(true);
    expect(shouldShowPartial("今天我们聊聊产品规划。接下来", last, true)).toBe(true);
  });

  it("首句尚无定稿时正常显示", () => {
    expect(shouldShowPartial("大家好", null, true)).toBe(true);
  });

  it("定稿句为空串时不误伤非空 partial", () => {
    expect(shouldShowPartial("大家好", "", true)).toBe(true);
  });
});
