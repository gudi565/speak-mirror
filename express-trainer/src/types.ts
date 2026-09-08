export interface Sentence {
  id: number;
  text: string;
  startMs: number;
  endMs: number;
}

export type FeedbackKind =
  | "fillerWord"
  | "wordPrecision"
  | "repetition"
  | "conclusionMissing"
  | "exampleMissing"
  | "emotion"
  | "hedge"
  | "timeVague"
  | "imagery"
  | "goldenQuote"
  | "aiCheckin";

export interface FeedbackEvent {
  kind: FeedbackKind;
  sentenceId: number | null;
  message: string;
  payload: Record<string, unknown>;
  /** 前端追加时分配的稳定 id（忽略反馈用，Rust 不感知） */
  uid?: number;
}

/** 情感类别统计：次数 + 平均强度（1–9） */
export interface EmotionStat {
  count: number;
  avgIntensity: number;
}

/** 声音层指标（M3，全部为会话内相对值） */
export interface VoiceMetrics {
  /** 前 3 秒有效语音是否完成基线校准 */
  baselineCalibrated: boolean;
  /** 音量动态范围（dB，相对基线 p90−p10）；句能量不足 2 句时为 null */
  volumeDynamicRangeDb: number | null;
  /** 能量稳定性（相邻句 dB 差方差）；不足 2 句时为 null */
  energyStability: number | null;
  /** 失控停顿（静音 >2s）次数 */
  runawayPauseCount: number;
  /** 最长一次失控停顿（毫秒） */
  longestPauseMs: number;
}

/** 一条声调偏差标记（Rust tone::ToneFlag；tone_update 事件与快照共用） */
export interface ToneFlag {
  /** 所属句子 id（与 Sentence.id 对齐，据此定位逐句回放） */
  sentenceId: number;
  /** 音节在句内汉字序列中的下标（0 起） */
  charIndex: number;
  /** 疑似读错的字 */
  char: string;
  /** 词典期望声调（主读音；1–4） */
  expectedTone: number;
  /** 检测到的形状（1 高平 / 2 升 / 3 降升 / 4 降 / 5 短轻） */
  detectedShape: number;
  /** 规则说明（v1）：变调/音域规则语境下的真偏差附说明（如
   *  「三声连读，前字应读作二声（升）」）；普通偏差与旧记录缺省 */
  note?: string | null;
}

export interface SessionSnapshot {
  sentenceCount: number;
  fillerCounts: [string, number][];
  fillerPerMinute: number;
  emotionCounts: [string, EmotionStat][];
  hedgeCounts: [string, number][];
  hedgeTotal: number;
  durationMs: number;
  totalChars: number;
  speechRate: number;
  avgSentenceChars: number;
  /** 金句候选句数（旧记录缺省 0） */
  goldenQuoteCount: number;
  /** 声音层终值（stop_session 时并入；会话中走 voice_update 事件） */
  voice?: VoiceMetrics | null;
  /** 声调偏差标记（v0：停止后异步分析并入；旧历史记录缺省 undefined） */
  toneFlags?: ToneFlag[];
}

/** mockInterview 仅在模拟面试流程内部使用（报告 scenario 与历史落盘），
 *  不出现在普通场景下拉与设置里（SCENARIOS 保持四项） */
export type Scenario = "free" | "interview" | "vlog" | "workreport" | "mockInterview";
/** 报告模式（0.2.3）：full = 完整八节；quick = 快速四节（约 1/3 生成时间）。
 *  与 Rust report::REPORT_MODES / parse_report_mode 对应 */
export type ReportMode = "full" | "quick";
export type AiBackend = "deepseek" | "openai" | "groq" | "ollama" | "custom";
/** 会话来源：实时麦克风 / 从文件练习 */
export type SessionSource = "mic" | "file";
/** 断句灵敏度（与 Rust settings::VAD_SENSITIVITIES 对应） */
export type VadSensitivity = "standard" | "high";

// ---------------------------------------------------------------------------
// 从文件练习：倍速档（与 Rust lib.rs 的 FILE_SPEEDS / FILE_SPEED_MAX 一致）
// ---------------------------------------------------------------------------

/** 「极速（不限速）」档哨兵值：JSON 无法序列化 Infinity，前后端约定 99 */
export const FILE_SPEED_MAX = 99;

/** 顶栏倍速徽标文案：极速档显示「极速」，其余显示如「1.5×」 */
export function speedLabel(speed: number): string {
  return speed === FILE_SPEED_MAX ? "极速" : `${speed}×`;
}

/** 规则开关键（与 Rust rules::engine::ALL_RULES 一致） */
export type RuleKey =
  | "filler_words"
  | "word_precision"
  | "repetition"
  | "conclusion_missing"
  | "example_missing"
  | "emotion_lexicon"
  | "hedge"
  | "time_vague"
  | "imagery"
  | "golden_quote";

export interface Settings {
  aiBackend: AiBackend;
  apiKey: string;
  baseUrl: string;
  modelName: string;
  obsidianVaultPath: string;
  scenario: Scenario;
  /** 每条规则单独开关；缺省键 = 开启 */
  ruleEnabled: Partial<Record<RuleKey, boolean>>;
  /** 口头禅高频阈值（次/分钟）：中频口头禅词频达到该值后才逐次提醒 */
  fillerHighThreshold: number;
  /** 用户自定义填充词（中英文逗号分隔） */
  customFillers: string;
  /** AI 周期快评开关（默认关闭） */
  realtimeCheckinEnabled: boolean;
  /** AI 周期快评频率（秒） */
  realtimeCheckinIntervalSec: number;
  /** 口头禅目标频率（次/分钟，M3 成长档案）；null = 未设目标 */
  fillerGoalPerMin: number | null;
  /** 首启引导是否已完成（M4）；false = 启动时进入三步向导 */
  onboardingDone: boolean;
  /** 会话录音（结束后逐句回放；音频仅保存在本机 appdata，默认开） */
  recordAudio: boolean;
  /** 识别热词（逗号/换行分隔）；空 = 不启用 */
  hotwords: string;
  /** 热词权重（常用 1.5–2.5）；null = 默认 1.5 */
  hotwordsScore: number | null;
  /** 识别纠错映射（每行一条「错->对」，如 深seek->DeepSeek） */
  asrCorrections: string;
  /** 高精度终稿（双引擎，默认开）：SenseVoice 就绪时句子定稿走离线引擎 */
  precisionFinals: boolean;
  /** 断句灵敏度：standard（Silero 0.5，默认）/ high（0.35，远距离/小声） */
  vadSensitivity: VadSensitivity;
  /** 录音增强（自动增益，默认开）：文件模式过静录音整段放大，改善断句与识别 */
  enhanceAudio: boolean;
  /** 显示实时识别预览（默认开）：中栏在定稿句下方显示「识别中…」流式小字；
   *  关闭后仅显示每句定稿，观感更稳 */
  showLivePreview: boolean;
  /** 声调偏差检查（默认开）：练习结束后对录音做纯本机的声调分析，
   *  疑似偏差在总结页「声调提示」面板列出；不上传录音 */
  toneCheck: boolean;
  /** AI 智能层激活横幅已「暂不提醒」（true = 主界面不再显示横幅）；
   *  配置远端后端后横幅条件自然不再成立 */
  aiNudgeDismissed: boolean;
}

export interface FillerWords {
  high: string[];
  medium: string[];
  custom: string[];
}

/** 模型完整性状态（Rust downloader::ModelsStatus） */
export interface ModelsStatus {
  modelsDir: string;
  /** 必需模型（VAD + Paraformer）缺失文件；非空 = 不可用 */
  missing: string[];
  /** SenseVoice 缺失文件（旧字段，与 recommendedMissing 一致；保留兼容） */
  optionalMissing?: string[];
  /** 推荐安装（0.2.2 起默认必装）但缺失的组件文件：目前即 SenseVoice。
   *  非空时前端提示「补装高精引擎」；跳过安装必须经用户显式确认 */
  recommendedMissing?: string[];
}

/** 词库候选条目（Rust growth::CandidateEntry）：高频出现、尚未收录的词 */
export interface LexiconCandidate {
  word: string;
  count: number;
}

export interface TranscriptResult {
  sentences: Sentence[];
  snapshot: SessionSnapshot;
}

export interface TestConnectionResult {
  ok: boolean;
  latencyMs: number;
  error: string | null;
}

export interface SaveOutcome {
  saved: boolean;
  path: string | null;
}

export type ReportStatus = "idle" | "streaming" | "done" | "error";

// ---------------------------------------------------------------------------
// 模拟面试
// ---------------------------------------------------------------------------

/** 一道模拟面试题（Rust interview::InterviewQuestion） */
export interface InterviewQuestion {
  index: number;
  question: string;
  intent: string;
}

/** generate_interview_questions 的返回值 */
export interface InterviewQuestionsResult {
  /** ai = LLM 出题；offline = 内置离线题库 */
  source: "ai" | "offline";
  questions: InterviewQuestion[];
  role: string;
}

/** 模拟面试报告的逐题问答（Rust report::QaItem） */
export interface QaItem {
  question: string;
  intent: string;
  /** 用户（修正后）的回答逐字稿，一行一句；空串 = 未作答/跳过 */
  answer: string;
  /** 该题回答时长（毫秒）；缺省 = 未知 */
  durationMs?: number | null;
}

/** 历史列表的摘要行（Rust history::SessionSummary） */
export interface SessionSummary {
  id: string;
  date: string;
  scenario: Scenario;
  topic: string;
  /** mic / file */
  source: SessionSource;
  /** 从文件练习时的源音频文件名（麦克风为 null） */
  fileName: string | null;
  /** 会话录音 wav 绝对路径（无录音为 null） */
  audioFile: string | null;
  durationMs: number;
  fillerPerMinute: number;
  speechRate: number;
  runawayPauseCount: number;
  longestPauseMs: number;
  /** 报告评分（SCORE 标记解析；本地降级报告与旧记录缺省 undefined） */
  scores?: Record<string, number> | null;
}

/** 一次会话的完整落盘记录（Rust history::SessionRecord） */
export interface SessionRecord {
  id: string;
  date: string;
  scenario: Scenario;
  topic: string;
  /** mic / file（旧记录缺省 mic） */
  source?: SessionSource;
  /** 从文件练习时的源音频文件名（麦克风省略） */
  fileName?: string | null;
  /** 会话录音 wav 绝对路径（录音关闭/截断时省略） */
  audioFile?: string | null;
  snapshot: SessionSnapshot;
  transcript: Sentence[];
  report: string;
  aiBackendUsed: string;
  /** 报告评分（SCORE 标记解析；本地降级报告与旧记录缺省） */
  scores?: Record<string, number> | null;
  /** 报告模式 full / quick（旧记录缺省 full） */
  reportMode?: ReportMode;
}
