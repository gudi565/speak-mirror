import { invoke } from "@tauri-apps/api/core";
import type { SessionRecord, SessionSummary } from "../types";

/**
 * 成长档案（历史页）的纯函数助手 + Tauri 命令封装。
 * 纯函数（sparklinePoints / goalStatus / goalY）可在无 Tauri 环境单测。
 */

export interface Point {
  x: number;
  y: number;
}

/**
 * 数值序列 → SVG 坐标点（从左到右按时间顺序）。
 * - 全等序列 → 水平居中线（避免除零）
 * - 单点 → 居中单点
 * - pad 为四周留白；y 轴翻转（值越大越靠上）
 */
export function sparklinePoints(
  values: number[],
  width: number,
  height: number,
  pad = 6,
): Point[] {
  if (values.length === 0) return [];
  const innerW = Math.max(width - pad * 2, 1);
  const innerH = Math.max(height - pad * 2, 1);
  const xs = (i: number): number =>
    values.length === 1 ? width / 2 : pad + (innerW * i) / (values.length - 1);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const span = max - min;
  const ys = (v: number): number =>
    span === 0 ? height / 2 : pad + innerH * (1 - (v - min) / span);
  return values.map((v, i) => ({ x: xs(i), y: ys(v) }));
}

/** 目标线在图内的 y 坐标（与 sparklinePoints 同一映射）；超出取值范围时夹边 */
export function goalY(
  values: number[],
  goal: number,
  height: number,
  pad = 6,
): number | null {
  if (values.length === 0 || !Number.isFinite(goal)) return null;
  const innerH = Math.max(height - pad * 2, 1);
  const min = Math.min(...values, goal);
  const max = Math.max(...values, goal);
  const span = max - min;
  if (span === 0) return height / 2;
  return pad + innerH * (1 - (goal - min) / span);
}

export interface GoalStatus {
  /** 当前值是否达标（≤ 目标） */
  met: boolean;
  /** 与目标的差（次/分钟，正 = 还差多少，负 = 超出目标多少；保留 1 位小数） */
  diff: number;
}

/** 最近一次口头禅频率 vs 目标 */
export function goalStatus(current: number, goal: number): GoalStatus {
  const diff = Math.round((current - goal) * 10) / 10;
  return { met: diff <= 0, diff };
}

export function fmtDuration(ms: number): string {
  const totalSec = Math.round(ms / 1000);
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  return m > 0 ? `${m} 分 ${String(s).padStart(2, "0")} 秒` : `${s} 秒`;
}

// ---------------------------------------------------------------------------
// 评分入档（B）：总分徽章 / 趋势取值（对无评分记录跳点）
// ---------------------------------------------------------------------------

/** 一条记录的总分（scores.overall）；无评分（本地降级 / 旧记录）为 null */
export function overallScore(summary: {
  scores?: Record<string, number> | null;
}): number | null {
  const overall = summary.scores?.["overall"];
  return typeof overall === "number" && Number.isFinite(overall) ? overall : null;
}

/** 总分徽章的配色档位（dataviz 纪律：用文本色阶而非花哨彩色） */
export function scoreTone(overall: number): string {
  if (overall >= 85) return "bg-green-100 text-green-800";
  if (overall >= 70) return "bg-blue-100 text-blue-800";
  if (overall >= 55) return "bg-amber-100 text-amber-800";
  return "bg-neutral-200 text-neutral-700";
}

/** 报告总分趋势序列：只含有评分的记录（无评分跳点），保持时间正序 */
export function scoreSeries(
  summaries: { date: string; scores?: Record<string, number> | null }[],
): { values: number[]; dates: string[] } {
  const values: number[] = [];
  const dates: string[] = [];
  for (const s of summaries) {
    const overall = overallScore(s);
    if (overall != null) {
      values.push(overall);
      dates.push(s.date);
    }
  }
  return { values, dates };
}

/** 来源小标记：🎧 文件练习 / 🎤 实时练习 */
export function sourceBadge(source: string | undefined | null): string {
  return source === "file" ? "🎧" : "🎤";
}

/** 来源中文名（tooltip / 无障碍用） */
export function sourceLabel(source: string | undefined | null): string {
  return source === "file" ? "从文件练习" : "实时练习";
}

/** 非 Tauri 环境（单测/纯浏览器）抛错由调用方兜底 */
export async function fetchSessionSummaries(): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_sessions");
}

export async function fetchSessionDetail(id: string): Promise<SessionRecord> {
  return invoke<SessionRecord>("get_session", { id });
}

export async function removeSession(id: string): Promise<void> {
  return invoke("delete_session", { id });
}
