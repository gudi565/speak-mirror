/**
 * 声调提示（纯函数）：ToneFlag 的文案与面板状态机。
 * 与 Rust tone.rs 的 tone_number_cn / shape_label_cn 同口径；
 * 组件侧（ToneFlagsPanel / SummaryView / HistoryView）只做渲染。
 */

import type { ToneFlag } from "../types";

/** 声调数字 → 中文（1–4；5 = 轻声） */
export const TONE_CN: Record<number, string> = {
  1: "一",
  2: "二",
  3: "三",
  4: "四",
  5: "轻",
};

/** 检测形状 → 听感描述（1 高平 / 2 升 / 3 降升 / 4 降 / 5 短轻） */
export const SHAPE_CN: Record<number, string> = {
  1: "高平",
  2: "升调",
  3: "降升",
  4: "降调",
  5: "短轻",
};

/** 一条标记的完整提示文案：第 N 句「X」应为 Y 声（听感偏 Z） */
export function toneFlagText(flag: ToneFlag): string {
  const tone = TONE_CN[flag.expectedTone] ?? String(flag.expectedTone);
  const shape = SHAPE_CN[flag.detectedShape] ?? String(flag.detectedShape);
  return `第 ${flag.sentenceId} 句「${flag.char}」应为${tone}声（听感偏${shape}）`;
}

/**
 * 标记附注（note）展示文案：后端规则说明原样透传；缺失/空白 → null（不渲染）。
 * 旧记录与普通偏差没有 note 字段，面板据此省略附注小字。
 */
export function toneNoteText(flag: ToneFlag): string | null {
  const note = typeof flag.note === "string" ? flag.note.trim() : "";
  return note.length > 0 ? note : null;
}

/** 声调提示面板状态（空态三分类 + 有发现） */
export type TonePanelState = "hidden" | "noAudio" | "analyzing" | "clean" | "flags";

/**
 * 面板状态判定（纯函数）：
 * - hidden：设置里关闭了声调检查 → 面板整体不显示
 * - noAudio：本次会话没有录音（未开启会话录音 / 截断 / 写失败）
 * - analyzing：开关开、有录音、但还没收到 tone_update（后端分析中）
 * - clean：已分析、无发现
 * - flags：有标记 → 列表
 */
export function tonePanelState(opts: {
  toneCheck: boolean;
  audioPath: string | null;
  toneFlags: ToneFlag[] | null;
}): TonePanelState {
  if (!opts.toneCheck) return "hidden";
  if (!opts.audioPath) return "noAudio";
  if (opts.toneFlags === null) return "analyzing";
  return opts.toneFlags.length > 0 ? "flags" : "clean";
}
