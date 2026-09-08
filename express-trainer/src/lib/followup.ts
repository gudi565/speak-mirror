import type { Scenario } from "../types";

/**
 * 报告追问（对话式补充解答）的纯函数模块：
 * - followupInvokeArgs：followup_question 命令的 invoke 参数契约；
 * - followupSuggestions：按场景固定的 3 条追问建议 chips；
 * - 问答对状态机：begin / appendChunk / finish / fail / retryAt 全部纯函数，
 *   只作用于「流式中」的条目，事件乱序 / 重复到达天然幂等。
 * 事件契约（Rust report.rs，与 useFollowup 对应）：
 * - followup_chunk { text } / followup_done { text } / followup_error { message }
 */

/** 一轮追问（Q/A 对）。answer 随 followup_chunk 流式增长；error 非空 = 本轮失败可重试 */
export interface FollowupItem {
  question: string;
  answer: string;
  error: string | null;
  /** 流式生成中（chunk 持续追加；done / error 落定后为 false） */
  streaming: boolean;
}

/** followup_question 命令的 invoke 参数（纯函数可测；问题统一 trim） */
export function followupInvokeArgs(
  question: string,
  reportText: string,
  scenario: Scenario,
): { question: string; reportText: string; scenario: Scenario } {
  return { question: question.trim(), reportText, scenario };
}

/** 各场景固定的追问建议 chips（点击直接填入输入框） */
const SUGGESTIONS: Record<Scenario, string[]> = {
  free: [
    "我最需要改的一个习惯是什么？",
    "哪个优点最值得保持？",
    "帮我把最差的一句改写一遍",
  ],
  interview: [
    "我的 STAR 哪个环节最弱？",
    "开头怎么改能更直接？",
    "面试官会怎么评价这个回答？",
  ],
  mockInterview: [
    "逐题里哪一题最弱？",
    "我的 STAR 哪个环节最弱？",
    "整体给面试官什么印象？",
  ],
  vlog: ["开场钩子抓人吗？", "哪一段信息密度最低？", "结尾怎么引导观众行动？"],
  workreport: ["结论够不够先行？", "哪里缺数据支撑？", "「下一步」部分怎么讲更好？"],
};

/** 追问建议 chips（按场景；每场景固定 3 条） */
export function followupSuggestions(scenario: Scenario): string[] {
  return SUGGESTIONS[scenario] ?? SUGGESTIONS.free;
}

/** 追问错误是否属于「未配置 AI 后端」类（UI 换成「去设置开启」配置引导按钮） */
export function isNoBackendError(message: string): boolean {
  return message.includes("未配置");
}

// ---------------------------------------------------------------------------
// 状态机（纯函数）
// ---------------------------------------------------------------------------

/** 发起一轮追问：追加一个流式中的条目 */
export function beginFollowup(items: FollowupItem[], question: string): FollowupItem[] {
  return [...items, { question, answer: "", error: null, streaming: true }];
}

/** 流式增量：追加到最后一个流式中的条目（无流式条目时原样返回——迟到的孤立 chunk） */
export function appendFollowupChunk(items: FollowupItem[], text: string): FollowupItem[] {
  return mapLastStreaming(items, (it) => ({ ...it, answer: it.answer + text }));
}

/** 完成：以全文落定最后一个流式条目（done 事件与 invoke 返回双保险，幂等） */
export function finishFollowup(items: FollowupItem[], text: string): FollowupItem[] {
  return mapLastStreaming(items, (it) => ({ ...it, answer: text, streaming: false }));
}

/** 失败：给最后一个流式条目标错误（可重试） */
export function failFollowup(items: FollowupItem[], message: string): FollowupItem[] {
  return mapLastStreaming(items, (it) => ({ ...it, error: message, streaming: false }));
}

/** 重试指定条目：失败且未在生成才可重置回流式（清空旧答案）；不可重试返回 null */
export function retryFollowupAt(items: FollowupItem[], index: number): FollowupItem[] | null {
  const it = items[index];
  if (!it || it.streaming || it.error == null) return null;
  const next = [...items];
  next[index] = { question: it.question, answer: "", error: null, streaming: true };
  return next;
}

/** 只改动最后一个「流式中」的条目；没有流式条目时原样返回（引用不变，便于测试断言） */
function mapLastStreaming(
  items: FollowupItem[],
  f: (it: FollowupItem) => FollowupItem,
): FollowupItem[] {
  const idx = findLastStreamingIndex(items);
  if (idx === -1) return items;
  const next = [...items];
  next[idx] = f(next[idx]);
  return next;
}

function findLastStreamingIndex(items: FollowupItem[]): number {
  for (let i = items.length - 1; i >= 0; i--) {
    if (items[i].streaming) return i;
  }
  return -1;
}
