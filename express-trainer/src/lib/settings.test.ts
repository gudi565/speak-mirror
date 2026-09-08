import { describe, expect, it } from "vitest";
import {
  DEFAULT_SETTINGS,
  BACKEND_PRESETS,
  hasRemoteBackend,
  normalizeSettings,
  settingsForStore,
  shouldShowAiNudge,
  smartLayerJustEnabled,
} from "./settings";
import { FILE_SPEED_MAX, speedLabel } from "../types";

describe("normalizeSettings", () => {
  it("fills defaults for missing fields", () => {
    const s = normalizeSettings(undefined);
    expect(s).toEqual(DEFAULT_SETTINGS);
  });

  it("fills M2 realtime-rule defaults for legacy settings", () => {
    const s = normalizeSettings({ aiBackend: "openai", apiKey: "k" });
    expect(s.ruleEnabled).toEqual({});
    expect(s.fillerHighThreshold).toBe(3);
    expect(s.customFillers).toBe("");
    expect(s.realtimeCheckinEnabled).toBe(false);
    expect(s.realtimeCheckinIntervalSec).toBe(45);
  });

  it("fills null filler goal for legacy settings and validates like Rust normalized()", () => {
    // M3 之前的历史设置无 fillerGoalPerMin → null
    expect(normalizeSettings({ aiBackend: "openai", apiKey: "k" }).fillerGoalPerMin).toBeNull();
    // 合法目标保留
    expect(normalizeSettings({ fillerGoalPerMin: 2.5 }).fillerGoalPerMin).toBe(2.5);
    // 非法值 → null；超界夹到 60（与 Rust 侧一致）
    expect(normalizeSettings({ fillerGoalPerMin: -1 }).fillerGoalPerMin).toBeNull();
    expect(normalizeSettings({ fillerGoalPerMin: Number.NaN }).fillerGoalPerMin).toBeNull();
    expect(normalizeSettings({ fillerGoalPerMin: 999 }).fillerGoalPerMin).toBe(60);
    expect(normalizeSettings({ fillerGoalPerMin: 0 }).fillerGoalPerMin).toBe(0);
  });

  it("clamps invalid M2 numeric values like Rust normalized()", () => {
    const s = normalizeSettings({
      fillerHighThreshold: -1,
      realtimeCheckinIntervalSec: 0,
    });
    expect(s.fillerHighThreshold).toBe(3);
    expect(s.realtimeCheckinIntervalSec).toBe(45);
    const capped = normalizeSettings({ fillerHighThreshold: 999, realtimeCheckinIntervalSec: 9999 });
    expect(capped.fillerHighThreshold).toBe(60);
    expect(capped.realtimeCheckinIntervalSec).toBe(45);
    const raised = normalizeSettings({ realtimeCheckinIntervalSec: 20 });
    expect(raised.realtimeCheckinIntervalSec).toBe(20);
  });

  it("falls back to deepseek/free on invalid values", () => {
    const s = normalizeSettings({ aiBackend: "bogus", scenario: "nope" });
    expect(s.aiBackend).toBe("deepseek");
    expect(s.scenario).toBe("free");
  });

  it("keeps valid partial settings", () => {
    const s = normalizeSettings({ aiBackend: "groq", apiKey: "sk-x" });
    expect(s.aiBackend).toBe("groq");
    expect(s.apiKey).toBe("sk-x");
    expect(s.obsidianVaultPath).toBe("");
  });

  it("fills onboarding flag false for legacy settings and validates type", () => {
    // M4 之前的设置无 onboardingDone → false（进入首启向导），与 Rust serde default 一致
    expect(normalizeSettings({ aiBackend: "openai", apiKey: "k" }).onboardingDone).toBe(false);
    expect(normalizeSettings(undefined).onboardingDone).toBe(false);
    // 非布尔值回落 false；合法值保留
    expect(normalizeSettings({ onboardingDone: "yes" } as never).onboardingDone).toBe(false);
    expect(normalizeSettings({ onboardingDone: 1 } as never).onboardingDone).toBe(false);
    expect(normalizeSettings({ onboardingDone: true }).onboardingDone).toBe(true);
  });

  it("defaults recording on and empty hotwords for legacy settings (M6)", () => {
    const legacy = normalizeSettings({ aiBackend: "openai", apiKey: "k" });
    expect(legacy.recordAudio).toBe(true); // 与 Rust default_record_audio 一致
    expect(legacy.hotwords).toBe("");
    expect(legacy.hotwordsScore).toBeNull();
    expect(legacy.asrCorrections).toBe("");
    // 非法类型回落默认
    expect(normalizeSettings({ recordAudio: "on" } as never).recordAudio).toBe(true);
    expect(normalizeSettings({ recordAudio: 0 } as never).recordAudio).toBe(true);
    expect(normalizeSettings({ recordAudio: false }).recordAudio).toBe(false); // 显式关闭保留
    expect(normalizeSettings({ hotwords: 5 } as never).hotwords).toBe("");
    expect(normalizeSettings({ asrCorrections: null } as never).asrCorrections).toBe("");
  });

  it("validates hotwords score like Rust normalized()", () => {
    // 合法值保留
    expect(normalizeSettings({ hotwordsScore: 2.5 }).hotwordsScore).toBe(2.5);
    // 超界夹到 [0.5, 5]
    expect(normalizeSettings({ hotwordsScore: 0.1 }).hotwordsScore).toBe(0.5);
    expect(normalizeSettings({ hotwordsScore: 99 }).hotwordsScore).toBe(5);
    // 非法值 → null（= Rust 侧默认 1.5）
    for (const bad of [-1, 0, Number.NaN, Number.POSITIVE_INFINITY, "x"]) {
      expect(normalizeSettings({ hotwordsScore: bad } as never).hotwordsScore).toBeNull();
    }
    expect(normalizeSettings({ hotwordsScore: null }).hotwordsScore).toBeNull();
  });
});

describe("settingsForStore", () => {
  it("strips the api key in secure mode (keyring holds it)", () => {
    const s = { ...DEFAULT_SETTINGS, apiKey: "sk-secret" };
    const stored = settingsForStore(s, true);
    expect(stored.apiKey).toBe("");
    // 其余字段原样；原对象不被改动
    expect(stored.obsidianVaultPath).toBe(s.obsidianVaultPath);
    expect(s.apiKey).toBe("sk-secret");
  });

  it("keeps the api key in degraded mode (keyring unavailable)", () => {
    const s = { ...DEFAULT_SETTINGS, apiKey: "sk-secret" };
    expect(settingsForStore(s, false).apiKey).toBe("sk-secret");
  });
});

describe("BACKEND_PRESETS", () => {
  it("every non-custom backend has base url and model", () => {
    for (const p of Object.values(BACKEND_PRESETS)) {
      expect(p.baseUrl).toMatch(/^https?:\/\//);
      expect(p.model.length).toBeGreaterThan(0);
    }
  });
});

describe("hasRemoteBackend", () => {
  it("requires a key except for ollama", () => {
    expect(hasRemoteBackend(DEFAULT_SETTINGS)).toBe(false);
    expect(hasRemoteBackend({ ...DEFAULT_SETTINGS, apiKey: "sk-x" })).toBe(true);
    expect(hasRemoteBackend({ ...DEFAULT_SETTINGS, aiBackend: "ollama" })).toBe(true);
  });
});

describe("aiNudgeDismissed + shouldShowAiNudge（AI 智能层激活横幅）", () => {
  it("defaults false for legacy settings and validates type", () => {
    // 旧设置无该字段 → false（主界面显示横幅），与 Rust serde default 一致
    expect(normalizeSettings({ aiBackend: "openai", apiKey: "k" }).aiNudgeDismissed).toBe(false);
    expect(DEFAULT_SETTINGS.aiNudgeDismissed).toBe(false);
    expect(normalizeSettings(undefined).aiNudgeDismissed).toBe(false);
    // 非布尔值回落 false；合法值保留
    expect(normalizeSettings({ aiNudgeDismissed: "yes" } as never).aiNudgeDismissed).toBe(false);
    expect(normalizeSettings({ aiNudgeDismissed: 1 } as never).aiNudgeDismissed).toBe(false);
    expect(normalizeSettings({ aiNudgeDismissed: true }).aiNudgeDismissed).toBe(true);
  });

  it("shows the banner only when settings loaded, no remote, and not dismissed", () => {
    // 设置未加载 → 不显示（避免首帧闪烁）
    expect(shouldShowAiNudge(null)).toBe(false);
    expect(shouldShowAiNudge(undefined)).toBe(false);
    // 默认（无 Key）→ 显示
    expect(shouldShowAiNudge(DEFAULT_SETTINGS)).toBe(true);
    // 配置远端后条件不再成立 → 消失
    expect(shouldShowAiNudge({ ...DEFAULT_SETTINGS, apiKey: "sk-x" })).toBe(false);
    expect(shouldShowAiNudge({ ...DEFAULT_SETTINGS, aiBackend: "ollama" })).toBe(false);
    // 「暂不提醒」→ 不再显示
    expect(shouldShowAiNudge({ ...DEFAULT_SETTINGS, aiNudgeDismissed: true })).toBe(false);
    // 已配置远端时 dismissed 不影响结果（横幅本就不显示）
    expect(
      shouldShowAiNudge({ ...DEFAULT_SETTINGS, apiKey: "k", aiNudgeDismissed: false }),
    ).toBe(false);
  });
});

describe("smartLayerJustEnabled（设置页一次性绿色提示的判定）", () => {
  it("is true only when saving crosses from no-remote to remote", () => {
    // 无 → 有（填了 Key / 切到 Ollama）
    expect(
      smartLayerJustEnabled(DEFAULT_SETTINGS, { ...DEFAULT_SETTINGS, apiKey: "sk-x" }),
    ).toBe(true);
    expect(
      smartLayerJustEnabled(DEFAULT_SETTINGS, { ...DEFAULT_SETTINGS, aiBackend: "ollama" }),
    ).toBe(true);
  });

  it("is false when remote existed before or after save stays remote-less", () => {
    // 有 → 有（换 Key / 换后端）
    expect(
      smartLayerJustEnabled(
        { ...DEFAULT_SETTINGS, apiKey: "a" },
        { ...DEFAULT_SETTINGS, apiKey: "b" },
      ),
    ).toBe(false);
    // 无 → 无（Ollama 切回 DeepSeek 未填 Key）
    expect(
      smartLayerJustEnabled({ ...DEFAULT_SETTINGS, aiBackend: "ollama" }, DEFAULT_SETTINGS),
    ).toBe(false);
    // 无 → 无（什么都不改）
    expect(smartLayerJustEnabled(DEFAULT_SETTINGS, DEFAULT_SETTINGS)).toBe(false);
    // 有 → 无（清掉 Key）：不算「开启」
    expect(smartLayerJustEnabled({ ...DEFAULT_SETTINGS, apiKey: "a" }, DEFAULT_SETTINGS)).toBe(
      false,
    );
  });
});

describe("recognition settings (dual-engine finals + VAD sensitivity)", () => {
  it("defaults to precisionFinals=true and standard sensitivity", () => {
    expect(DEFAULT_SETTINGS.precisionFinals).toBe(true);
    expect(DEFAULT_SETTINGS.vadSensitivity).toBe("standard");
    expect(normalizeSettings(undefined)).toEqual(DEFAULT_SETTINGS);
  });

  it("fills defaults for legacy settings without the new fields", () => {
    const s = normalizeSettings({ aiBackend: "openai", apiKey: "k" });
    expect(s.precisionFinals).toBe(true);
    expect(s.vadSensitivity).toBe("standard");
  });

  it("keeps valid values and falls back to standard on invalid sensitivity", () => {
    expect(normalizeSettings({ vadSensitivity: "high" }).vadSensitivity).toBe("high");
    // 非法值（含 undefined / 旧版本未知枚举）→ standard，与 Rust normalized() 一致
    expect(normalizeSettings({ vadSensitivity: "loud" }).vadSensitivity).toBe("standard");
    expect(normalizeSettings({ vadSensitivity: undefined }).vadSensitivity).toBe("standard");
    // precisionFinals 仅接受布尔；显式关闭 round-trip 保留
    expect(normalizeSettings({ precisionFinals: false }).precisionFinals).toBe(false);
    expect(normalizeSettings({ precisionFinals: "yes" }).precisionFinals).toBe(true);
  });

  it("defaults enhanceAudio=true and keeps explicit off (与 Rust default_enhance_audio 一致)", () => {
    expect(DEFAULT_SETTINGS.enhanceAudio).toBe(true);
    // 旧设置无该字段 → 默认开；非法类型回落开
    expect(normalizeSettings({ aiBackend: "openai", apiKey: "k" }).enhanceAudio).toBe(true);
    expect(normalizeSettings({ enhanceAudio: 0 } as never).enhanceAudio).toBe(true);
    expect(normalizeSettings({ enhanceAudio: "on" } as never).enhanceAudio).toBe(true);
    // 显式关闭 round-trip 保留
    expect(normalizeSettings({ enhanceAudio: false }).enhanceAudio).toBe(false);
  });

  it("defaults showLivePreview=true and keeps explicit off (与 Rust default_show_live_preview 一致)", () => {
    // 定稿为主、预览为辅：默认显示「识别中…」预览行
    expect(DEFAULT_SETTINGS.showLivePreview).toBe(true);
    // 旧设置无该字段 → 默认开；非法类型回落开
    expect(normalizeSettings({ aiBackend: "openai", apiKey: "k" }).showLivePreview).toBe(true);
    expect(normalizeSettings({ showLivePreview: 0 } as never).showLivePreview).toBe(true);
    expect(normalizeSettings({ showLivePreview: "off" } as never).showLivePreview).toBe(true);
    // 显式关闭 round-trip 保留（关闭后仅显示每句定稿，观感更稳）
    expect(normalizeSettings({ showLivePreview: false }).showLivePreview).toBe(false);
  });

  it("defaults toneCheck=true and keeps explicit off (与 Rust default_tone_check 一致)", () => {
    // 声调偏差检查默认开（练习结束后的本机分析）
    expect(DEFAULT_SETTINGS.toneCheck).toBe(true);
    // 旧设置无该字段 → 默认开；非法类型回落开
    expect(normalizeSettings({ aiBackend: "openai", apiKey: "k" }).toneCheck).toBe(true);
    expect(normalizeSettings({ toneCheck: 0 } as never).toneCheck).toBe(true);
    expect(normalizeSettings({ toneCheck: "on" } as never).toneCheck).toBe(true);
    // 显式关闭 round-trip 保留
    expect(normalizeSettings({ toneCheck: false }).toneCheck).toBe(false);
  });
});

describe("file speed options", () => {
  it("max sentinel is 99 (JSON 无法传 Infinity) and renders as 极速", () => {
    expect(FILE_SPEED_MAX).toBe(99);
    expect(speedLabel(FILE_SPEED_MAX)).toBe("极速");
    expect(speedLabel(99)).toBe("极速");
  });

  it("labels realtime speeds as N×", () => {
    expect(speedLabel(1.0)).toBe("1×");
    expect(speedLabel(1.5)).toBe("1.5×");
    expect(speedLabel(2.0)).toBe("2×");
  });
});
