import { load, type Store } from "@tauri-apps/plugin-store";
import { invoke } from "@tauri-apps/api/core";
import { SCENARIOS } from "./scenarios";
import type { AiBackend, Settings } from "../types";

/** 与 Rust 侧 src-tauri/src/settings.rs 的 preset_for 保持同步 */
export const BACKEND_PRESETS: Record<
  Exclude<AiBackend, "custom">,
  { label: string; baseUrl: string; model: string }
> = {
  deepseek: { label: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-chat" },
  openai: { label: "OpenAI", baseUrl: "https://api.openai.com/v1", model: "gpt-4o-mini" },
  groq: { label: "Groq", baseUrl: "https://api.groq.com/openai/v1", model: "llama-3.3-70b-versatile" },
  ollama: { label: "Ollama（本地）", baseUrl: "http://localhost:11434/v1", model: "qwen2.5:7b" },
};

export const BACKEND_LABELS: Record<AiBackend, string> = {
  ...Object.fromEntries(
    Object.entries(BACKEND_PRESETS).map(([k, v]) => [k, v.label]),
  ) as Record<Exclude<AiBackend, "custom">, string>,
  custom: "自定义",
};

export const DEFAULT_SETTINGS: Settings = {
  aiBackend: "deepseek",
  apiKey: "",
  baseUrl: "",
  modelName: "",
  obsidianVaultPath: "",
  scenario: "free",
  ruleEnabled: {},
  fillerHighThreshold: 3,
  customFillers: "",
  realtimeCheckinEnabled: false,
  realtimeCheckinIntervalSec: 45,
  fillerGoalPerMin: null,
  onboardingDone: false,
  recordAudio: true,
  hotwords: "",
  hotwordsScore: null,
  asrCorrections: "",
  precisionFinals: true,
  vadSensitivity: "standard",
  enhanceAudio: true,
  showLivePreview: true,
  toneCheck: true,
  aiNudgeDismissed: false,
};

/** 与 Rust 侧 normalized() 一致：非法取值回落默认 */
export function normalizeSettings(raw: unknown): Settings {
  const s = { ...DEFAULT_SETTINGS, ...((raw ?? {}) as Partial<Settings>) };
  const backends: AiBackend[] = ["deepseek", "openai", "groq", "ollama", "custom"];
  if (!backends.includes(s.aiBackend)) s.aiBackend = "deepseek";
  if (!SCENARIOS.some((x) => x.value === s.scenario)) s.scenario = "free";
  if (!s.ruleEnabled || typeof s.ruleEnabled !== "object") s.ruleEnabled = {};
  if (typeof s.fillerHighThreshold !== "number" || !Number.isFinite(s.fillerHighThreshold) || s.fillerHighThreshold < 0) {
    s.fillerHighThreshold = 3;
  }
  s.fillerHighThreshold = Math.min(s.fillerHighThreshold, 60);
  if (typeof s.customFillers !== "string") s.customFillers = "";
  if (typeof s.realtimeCheckinEnabled !== "boolean") s.realtimeCheckinEnabled = false;
  if (typeof s.realtimeCheckinIntervalSec !== "number" || !Number.isFinite(s.realtimeCheckinIntervalSec)) {
    s.realtimeCheckinIntervalSec = 45;
  }
  // 与 Rust normalized() 对齐：0 或 >600 回落 45，再夹到 [15, 600]
  if (s.realtimeCheckinIntervalSec <= 0 || s.realtimeCheckinIntervalSec > 600) {
    s.realtimeCheckinIntervalSec = 45;
  }
  s.realtimeCheckinIntervalSec = Math.min(Math.max(s.realtimeCheckinIntervalSec, 15), 600);
  // 口头禅目标（可空）：非法值 → null，上限与阈值一致夹到 60
  if (
    s.fillerGoalPerMin == null ||
    typeof s.fillerGoalPerMin !== "number" ||
    !Number.isFinite(s.fillerGoalPerMin) ||
    s.fillerGoalPerMin < 0
  ) {
    s.fillerGoalPerMin = null;
  } else {
    s.fillerGoalPerMin = Math.min(s.fillerGoalPerMin, 60);
  }
  // 首启引导标志（M4）：非布尔值回落 false（与 Rust serde default 一致）
  if (typeof s.onboardingDone !== "boolean") s.onboardingDone = false;
  // 会话录音（默认 true，与 Rust default_record_audio 一致）
  if (typeof s.recordAudio !== "boolean") s.recordAudio = true;
  if (typeof s.hotwords !== "string") s.hotwords = "";
  // 热词权重：非法值（非数/非有限/非正）→ null（= Rust 侧默认 1.5）；夹到 [0.5, 5]
  if (
    s.hotwordsScore == null ||
    typeof s.hotwordsScore !== "number" ||
    !Number.isFinite(s.hotwordsScore) ||
    s.hotwordsScore <= 0
  ) {
    s.hotwordsScore = null;
  } else {
    s.hotwordsScore = Math.min(Math.max(s.hotwordsScore, 0.5), 5);
  }
  if (typeof s.asrCorrections !== "string") s.asrCorrections = "";
  // 高精度终稿（默认 true，与 Rust default_precision_finals 一致）
  if (typeof s.precisionFinals !== "boolean") s.precisionFinals = true;
  // 断句灵敏度：白名单外回落 standard（与 Rust normalized() 一致）
  if (s.vadSensitivity !== "high" && s.vadSensitivity !== "standard") {
    s.vadSensitivity = "standard";
  }
  // 录音增强（默认 true，与 Rust default_enhance_audio 一致）
  if (typeof s.enhanceAudio !== "boolean") s.enhanceAudio = true;
  // 实时识别预览（默认 true，与 Rust default_show_live_preview 一致）
  if (typeof s.showLivePreview !== "boolean") s.showLivePreview = true;
  // 声调偏差检查（默认 true，与 Rust default_tone_check 一致）
  if (typeof s.toneCheck !== "boolean") s.toneCheck = true;
  // AI 智能层激活横幅「暂不提醒」（默认 false，与 Rust serde default 一致）
  if (typeof s.aiNudgeDismissed !== "boolean") s.aiNudgeDismissed = false;
  return s;
}

/** Ollama 本地服务无需 Key */
export function hasRemoteBackend(s: Settings): boolean {
  return s.aiBackend === "ollama" || s.apiKey.trim().length > 0;
}

/**
 * 主界面「AI 智能层未开启」横幅是否显示（纯函数可测）：
 * 设置已加载 且 未配置远端后端 且 未点过「暂不提醒」。
 * 配置 Key（或切到 Ollama）后条件自然不再成立，横幅自动消失。
 */
export function shouldShowAiNudge(settings: Settings | null | undefined): boolean {
  return settings != null && !settings.aiNudgeDismissed && !hasRemoteBackend(settings);
}

/**
 * 设置保存后智能层是否「从无到有」开启（设置页一次性绿色提示的判定，纯函数可测）：
 * 保存前无远端、保存后有远端（填了 Key / 切到 Ollama）。
 */
export function smartLayerJustEnabled(before: Settings, after: Settings): boolean {
  return !hasRemoteBackend(before) && hasRemoteBackend(after);
}

const STORE_FILE = "settings.json";
let storePromise: Promise<Store> | null = null;

function getStore(): Promise<Store> {
  storePromise ??= load(STORE_FILE, { autoSave: true });
  return storePromise;
}

/**
 * 存进 store 的设置体（纯函数可测）：安全模式（Key 在系统凭据管理器）下
 * apiKey 清空不落盘；降级模式保留明文（与 Rust 侧 set_api_key 语义配对）。
 */
export function settingsForStore(s: Settings, secureKey: boolean): Settings {
  return { ...s, apiKey: secureKey ? "" : s.apiKey };
}

export interface SaveSettingsOutcome {
  /** true = API Key 已写入系统凭据管理器（store 无明文） */
  secureKey: boolean;
  /** 降级时的中文提示（SettingsView 展示） */
  keyMessage: string | null;
}

/** 读取设置；在非 Tauri 环境（单测/纯浏览器）返回默认值。
 *  API Key 的生效值来自系统凭据管理器（Rust 侧含旧明文自动迁移）；
 *  读不到命令（非 Tauri）时保留 store 值。 */
export async function loadSettings(): Promise<Settings> {
  let fromStore: unknown;
  try {
    const store = await getStore();
    fromStore = await store.get("settings");
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
  const s = normalizeSettings(fromStore);
  try {
    const key = await invoke<string | null>("get_api_key");
    s.apiKey = key ?? "";
  } catch {
    /* 非 Tauri 环境：保留 store 里的值（通常为空） */
  }
  return s;
}

export async function saveSettings(s: Settings): Promise<SaveSettingsOutcome> {
  // 先把 Key 写进系统凭据管理器（空串 = 清除）；失败降级为明文落盘
  let secureKey = true;
  let keyMessage: string | null = null;
  try {
    const r = await invoke<{ secure: boolean; message: string | null }>("set_api_key", {
      key: s.apiKey,
    });
    secureKey = r.secure;
    keyMessage = r.message ?? null;
  } catch (e) {
    secureKey = false;
    keyMessage = `无法访问系统凭据管理器（${String(e)}），API Key 将明文保存在本机设置文件中`;
  }
  const store = await getStore();
  await store.set("settings", settingsForStore(s, secureKey));
  await store.save();
  return { secureKey, keyMessage };
}
