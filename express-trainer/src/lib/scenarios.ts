import type { Scenario } from "../types";

/**
 * 场景元数据（与 Rust settings.rs / report.rs 的 SCENARIOS 同步）。
 * 纯函数模块：供场景选择器、历史列表与报告标题共用。
 */
export interface ScenarioMeta {
  value: Scenario;
  label: string;
  /** 场景说明文案（选择器下方一行提示） */
  hint: string;
  /** 主题输入框标签 */
  topicLabel: string;
  /** 主题输入框占位示例 */
  topicPlaceholder: string;
}

export const SCENARIOS: ScenarioMeta[] = [
  {
    value: "free",
    label: "自由练习",
    hint: "无拘束即兴表达，看表达效率、结构意识与词汇精确度",
    topicLabel: "主题（可空）",
    topicPlaceholder: "如：自律",
  },
  {
    value: "interview",
    label: "面试回答",
    hint: "按 STAR 完整性与结论先行评估",
    topicLabel: "面试题（可空）",
    topicPlaceholder: "如：讲一次你推动协作的经历",
  },
  {
    value: "vlog",
    label: "口播视频",
    hint: "看开场钩子、信息密度、节奏与结尾行动号召",
    topicLabel: "视频主题（可空）",
    topicPlaceholder: "如：为什么我劝你别熬夜",
  },
  {
    value: "workreport",
    label: "工作汇报",
    hint: "看结论先行、数据支撑与背景/进展/风险/下一步结构",
    topicLabel: "汇报主题（可空）",
    topicPlaceholder: "如：Q3 项目进展",
  },
];

const LABELS: Record<string, string> = Object.fromEntries(
  SCENARIOS.map((s) => [s.value, s.label]),
);

/** 模拟面试（mockInterview）只在面试流程内部使用，不进场景选择器 */
export const MOCK_INTERVIEW_LABEL = "模拟面试";

/** 场景中文名（历史列表 / 报告标题用）；未知值回落「自由练习」 */
export function scenarioLabel(scenario: string): string {
  if (scenario === "mockInterview") return MOCK_INTERVIEW_LABEL;
  return LABELS[scenario] ?? "自由练习";
}

/** 场景元数据查找（选择器提示与主题标签用）；未知值回落 free */
export function scenarioMeta(scenario: string): ScenarioMeta {
  return SCENARIOS.find((s) => s.value === scenario) ?? SCENARIOS[0];
}
