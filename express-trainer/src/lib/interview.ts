import type { InterviewQuestion, QaItem, Scenario } from "../types";

/**
 * 模拟面试的岗位方向、题数白名单与流程纯函数。
 * 纯函数（进度 / 答案累积 / qa 组装）可在无 Tauri 环境单测；
 * 与 Rust interview.rs 的 ROLES / QUESTION_COUNTS 保持同步。
 */

export type InterviewRole =
  | "general"
  | "backend"
  | "frontend"
  | "product"
  | "ops"
  | "management"
  | "custom";

export interface InterviewRoleMeta {
  value: InterviewRole;
  label: string;
  /** 选择器下方的中文说明 */
  hint: string;
}

export const INTERVIEW_ROLES: InterviewRoleMeta[] = [
  { value: "general", label: "通用面试", hint: "自我介绍、动机、经历深挖等各行业通用题" },
  { value: "backend", label: "后端开发", hint: "架构取舍、线上故障、慢查询、高并发等" },
  { value: "frontend", label: "前端开发", hint: "性能优化、状态管理、组件化、跨端一致等" },
  { value: "product", label: "产品经理", hint: "需求判断、数据复盘、竞品分析、说服协作等" },
  { value: "ops", label: "运营", hint: "活动策划、增长渠道、指标拆解、危机处理等" },
  { value: "management", label: "管理岗位", hint: "带团队、绩效沟通、破局跨部门、艰难决策等" },
  { value: "custom", label: "自定义岗位", hint: "粘贴职位描述（JD），AI 按 JD 出题；离线时回落通用题库" },
];

/** 题数白名单（与 Rust QUESTION_COUNTS 一致） */
export const QUESTION_COUNTS = [3, 5, 8] as const;
export type QuestionCount = (typeof QUESTION_COUNTS)[number];

export function isInterviewRole(v: string): v is InterviewRole {
  return INTERVIEW_ROLES.some((r) => r.value === v);
}

/** 岗位中文名（报告 topic 与历史列表用） */
export function interviewRoleLabel(role: string): string {
  return INTERVIEW_ROLES.find((r) => r.value === role)?.label ?? "通用面试";
}

/** 模拟面试报告专用场景值（四场景选择器不含它） */
export const MOCK_SCENARIO: Scenario = "mockInterview";

// ---------------------------------------------------------------------------
// 流程纯函数（题进度 / 答案累积 / qa 组装）
// ---------------------------------------------------------------------------

export interface InterviewProgress {
  /** 当前题（0 起始；与 current 相同口径） */
  current: number;
  total: number;
  /** 展示用「第 x / N 题」的 x（1 起始） */
  label: string;
  isLast: boolean;
  /** 下一题下标；已是最后一题为 null（应进入汇总） */
  nextIndex: number | null;
}

/** 题目进度（纯函数）：由当前下标与总题数推导展示与流转信息 */
export function interviewProgress(current: number, total: number): InterviewProgress {
  const clamped = Math.max(0, Math.min(current, Math.max(total - 1, 0)));
  const next = clamped + 1;
  return {
    current: clamped,
    total,
    label: `第 ${clamped + 1} / ${total} 题`,
    isLast: next >= total,
    nextIndex: next < total ? next : null,
  };
}

/** 一轮面试的逐题作答状态（text 为可编辑的回答逐字稿；空串 = 未作答/跳过） */
export interface InterviewAnswers {
  /** 与题目同序的回答文本 */
  texts: string[];
  /** 每题回答时长（毫秒，0 = 未知/未作答） */
  durations: number[];
}

/** 初始化逐题答案容器 */
export function makeAnswers(count: number): InterviewAnswers {
  return { texts: new Array(count).fill(""), durations: new Array(count).fill(0) };
}

/** 记录一题的回答（结束回答时调用；越界下标原样返回） */
export function recordAnswer(
  answers: InterviewAnswers,
  index: number,
  text: string,
  durationMs: number,
): InterviewAnswers {
  if (index < 0 || index >= answers.texts.length) return answers;
  const texts = [...answers.texts];
  const durations = [...answers.durations];
  texts[index] = text;
  durations[index] = Math.max(0, Math.round(durationMs));
  return { texts, durations };
}

/** 更新汇总页编辑后的某题回答文本 */
export function editAnswer(
  answers: InterviewAnswers,
  index: number,
  text: string,
): InterviewAnswers {
  if (index < 0 || index >= answers.texts.length) return answers;
  const texts = [...answers.texts];
  texts[index] = text;
  return { texts, durations: answers.durations };
}

/** 作答情况统计：作答题数 / 跳过题数 / 总字数（非空白字符） */
export function summarizeAnswers(answers: InterviewAnswers): {
  answered: number;
  skipped: number;
  totalChars: number;
} {
  let answered = 0;
  let totalChars = 0;
  for (const t of answers.texts) {
    const clean = t.trim();
    if (clean.length > 0) {
      answered += 1;
      totalChars += clean.replace(/\s/g, "").length;
    }
  }
  return { answered, skipped: answers.texts.length - answered, totalChars };
}

/** 组装报告请求的 qa 载荷；没有任何作答（全空）时返回 null（不可生成报告） */
export function buildQaPayload(
  questions: InterviewQuestion[],
  answers: InterviewAnswers,
): QaItem[] | null {
  if (questions.length === 0) return null;
  const qa: QaItem[] = questions.map((q, i) => ({
    question: q.question,
    intent: q.intent,
    answer: (answers.texts[i] ?? "").trim(),
    durationMs: answers.durations[i] > 0 ? answers.durations[i] : null,
  }));
  return qa.some((item) => item.answer.length > 0) ? qa : null;
}
