import type { ReportMode } from "../types";

/**
 * 报告骨架与流式分节（纯函数模块，供 ReportView「骨架先行」渲染）。
 *
 * 契约依据（src-tauri/prompts/*.md 的输出格式）：
 * - AI 报告为纯 Markdown，按 `## ` 标题行分节，节顺序固定：
 *   完整版八节 / 快速版四节，第一节之前不得有任何内容，末尾一行 SCORE 注释。
 * - 本地降级报告以 `# 表达训练 · 本地报告`（h1）开头、节标题不同——
 *   见 shouldUseSkeleton：含 h1 的文本不走骨架，直接按原文渲染。
 */

/** 完整版八节标题（与四份场景 prompt 的节标题一字不差） */
export const FULL_SECTIONS = [
  "一、总评",
  "二、亮点",
  "三、逐句改写",
  "四、可替换词汇表",
  "五、行为模式分析",
  "六、怎么说的",
  "七、数据区",
  "八、下次练习重点",
] as const;

/** 快速版四节标题（prompts/quick.md） */
export const QUICK_SECTIONS = [
  "一、总评",
  "二、亮点",
  "三、最需要改进的一个问题",
  "四、下次重点",
] as const;

/** 按模式取骨架标题（骨架先行的占位顺序） */
export function skeletonSections(mode: ReportMode): string[] {
  return mode === "quick" ? [...QUICK_SECTIONS] : [...FULL_SECTIONS];
}

/** 报告模式中文名（总结页选择器与历史详情标签共用）；未知/缺省按完整版 */
export function reportModeLabel(mode?: ReportMode | string | null): string {
  return mode === "quick" ? "快速报告" : "完整报告";
}

/** 总结页的报告模式选项（默认 quick——用户痛点是慢） */
export const REPORT_MODE_OPTIONS: { value: ReportMode; label: string; hint: string }[] = [
  { value: "quick", label: "快速报告", hint: "约 1/3 时间，只有总评与重点" },
  { value: "full", label: "完整报告", hint: "八节深度分析" },
];

/**
 * 移除（流式中途可能出现的）SCORE 评分注释：完整的 `<!--SCORE:…-->`
 * 整段移除；未闭合的尾巴（`<!--SCORE:{…` 还没流完）从标记起裁掉。
 * 与 Rust report::strip_score_marker 同一逻辑（终稿 Rust 已剥离，这里只管流式中途）。
 */
export function stripScoreMarker(md: string): string {
  const start = md.indexOf("<!--SCORE:");
  if (start === -1) return md;
  const end = md.indexOf("-->", start);
  if (end === -1) return md.slice(0, start).trimEnd();
  // 与 Rust 版完全对齐：移除标记 → 收掉三连空行 → 尾部规整为单个换行
  return ((md.slice(0, start) + md.slice(end + 3)).replace(/\n{3,}/g, "\n\n").trimEnd() + "\n");
}

/** 骨架渲染的一节：index 为骨架槽位（-1 = 首个 ## 之前的前言） */
export interface ReportSection {
  index: number;
  /** 显示标题（已开始的节用流入的标题原文——容忍 mockInterview「三、逐题点评」这类场景差异） */
  title: string;
  /** 该节已流入的正文（已裁掉 SCORE 标记） */
  body: string;
  /** 下一节的标题已出现 → 本节视为完成 */
  done: boolean;
}

/** 骨架渲染的完整视图模型（ReportView 直接消费） */
export interface SkeletonLayout {
  /** 首个 ## 之前的内容（AI 报告契约为空；非空时兜底原样展示，不给标题） */
  preamble: string;
  /** 已出现 + 未出现（占位）的全部节，按骨架顺序 */
  rows: ReportSection[];
  /** 已完成节数（下一节开始才算完成；进度条 x / N 的 x） */
  doneCount: number;
  /** 总节数（骨架槽数；流入节数更多时取流入数） */
  total: number;
}

/**
 * 把流式报告文本按 `## ` 标题行切块，归位到骨架槽位：
 * 第 i 个出现的标题归入槽 i；未出现的槽以骨架标题占位（body 空）。
 * 纯函数：每帧对全量 text 重算（流式文本量小，成本可忽略）。
 */
export function layoutSkeleton(text: string, mode: ReportMode): SkeletonLayout {
  const skeleton = skeletonSections(mode);
  const blocks: { heading: string | null; lines: string[] }[] = [
    { heading: null, lines: [] },
  ];
  for (const line of text.split("\n")) {
    const m = /^##\s+(.*)$/.exec(line);
    if (m && m[1].trim()) blocks.push({ heading: m[1].trim(), lines: [] });
    else blocks[blocks.length - 1].lines.push(line);
  }
  const preamble = stripScoreMarker(blocks[0].lines.join("\n")).trim();
  const headed = blocks.filter((b) => b.heading !== null);
  const arrived: ReportSection[] = headed.map((b, i) => ({
    index: i,
    title: b.heading as string,
    body: stripScoreMarker(b.lines.join("\n")).trim(),
    done: i + 1 < headed.length,
  }));
  const total = Math.max(skeleton.length, arrived.length);
  const rows: ReportSection[] = [];
  for (let i = 0; i < total; i++) {
    const sec = arrived.find((s) => s.index === i);
    rows.push(sec ?? { index: i, title: skeleton[i] ?? `第 ${i + 1} 节`, body: "", done: false });
  }
  return { preamble, rows, doneCount: rows.filter((r) => r.done).length, total };
}

/**
 * 是否走骨架渲染：文本为空（尚未连上模型）或不含 h1 标题（AI 报告契约：
 * 第一节之前不得有任何内容）。本地降级报告以 `# 表达训练 · 本地报告`
 * 开头且节结构与骨架不同 → 直接按原文渲染，不套骨架。
 */
export function shouldUseSkeleton(text: string): boolean {
  const t = text.trim();
  if (!t) return true;
  return !/^#\s/m.test(t);
}
