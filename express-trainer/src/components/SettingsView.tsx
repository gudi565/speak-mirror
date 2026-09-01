import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { hasRemoteBackend } from "../lib/settings";
import { SCENARIOS } from "../lib/scenarios";
import { downloadFileLabel, formatBytes, progressPercent } from "../lib/onboarding";
import { AiBackendFields } from "./AiBackendFields";
import type { LexiconCandidate, ModelsStatus, RuleKey, Settings } from "../types";

interface Props {
  settings: Settings;
  onUpdate: (patch: Partial<Settings>) => void;
  onSave: (next: Settings) => Promise<{ secureKey: boolean; keyMessage: string | null } | null>;
  onClose: () => void;
}

/** 规则开关键与中文标签（与 Rust rules::engine::ALL_RULES 一致） */
const RULE_ITEMS: { key: RuleKey; label: string }[] = [
  { key: "filler_words", label: "口头禅（填充词分级计数）" },
  { key: "word_precision", label: "词汇精确度（笼统词 → 精准替代）" },
  { key: "repetition", label: "重复提醒" },
  { key: "conclusion_missing", label: "结论缺失提醒" },
  { key: "example_missing", label: "缺举例提醒" },
  { key: "emotion_lexicon", label: "情感词（七大类 · 强度）" },
  { key: "hedge", label: "立场模糊（单句堆叠弱化词）" },
  { key: "time_vague", label: "时间模糊（最近 / 回头 / 过几天…）" },
  { key: "imagery", label: "画面感（比喻标记 · 具象化建议）" },
  { key: "golden_quote", label: "金句捕捉（比喻 / 数字结论 / 对仗强调，正向）" },
];

/** 词库候选的替代词输入解析：中英文逗号 / 顿号分隔，去空白与空项 */
export function splitAlternatives(raw: string): string[] {
  return raw
    .split(/[,，、]/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** 流式 Paraformer 目录名（与 Rust downloader::PARA_DIR_NAME 一致），
 *  用于从 missing 清单区分必需组件的归属 */
const PARA_DIR_PREFIX = "sherpa-onnx-streaming-paraformer-bilingual-zh-en";

/** 组件在位状态行：绿点已就绪 / 红点缺失 */
function ModelRow({ ok, label }: { ok: boolean; label: string }) {
  return (
    <div className="flex items-center gap-2">
      <span
        className={`inline-block h-2 w-2 shrink-0 rounded-full ${ok ? "bg-green-500" : "bg-red-400"}`}
      />
      <span className="min-w-0 flex-1 text-sm text-neutral-700">{label}</span>
      <span className={`shrink-0 text-xs ${ok ? "text-green-600" : "text-red-500"}`}>
        {ok ? "已就绪" : "缺失"}
      </span>
    </div>
  );
}

export function SettingsView({ settings, onUpdate, onSave, onClose }: Props) {
  const [savedHint, setSavedHint] = useState(false);
  // API Key 降级明文时的提示（系统凭据管理器写入失败）
  const [keyWarning, setKeyWarning] = useState<string | null>(null);

  // 词库候选（词库自生长）：独立于设置体，走 growth 命令
  const [candidates, setCandidates] = useState<LexiconCandidate[] | null>(null);
  const [altInputs, setAltInputs] = useState<Record<string, string>>({});
  const [lexHint, setLexHint] = useState<string | null>(null);
  const [lexBusy, setLexBusy] = useState<string | null>(null);

  // 识别组件在位状态（三组模型检测 + SenseVoice 缺失时补装）
  const [models, setModels] = useState<ModelsStatus | null>(null);
  const [modelBusy, setModelBusy] = useState(false);
  const [modelError, setModelError] = useState<string | null>(null);
  const [modelProgress, setModelProgress] = useState<{
    file: string;
    received: number;
    total: number;
  } | null>(null);

  useEffect(() => {
    invoke<LexiconCandidate[]>("list_lexicon_candidates")
      .then((list) => setCandidates(list))
      .catch(() => setCandidates([])); // 非 Tauri 环境显示空态
  }, []);

  useEffect(() => {
    let un: (() => void) | undefined;
    invoke<ModelsStatus>("check_models")
      .then((s) => setModels(s))
      .catch(() => setModels(null)); // 非 Tauri 环境显示检测失败
    listen<{ file: string; received: number; total: number }>(
      "download_progress",
      (e) => setModelProgress(e.payload),
    ).then((f) => {
      un = f;
    });
    return () => un?.();
  }, []);

  /** 补装高精引擎（必需组件缺失时同一下载全量补齐）：调 download_models，
   *  失败时刷新状态——必需组件可能已就绪（如仅高精引擎下载失败） */
  const installModels = async () => {
    if (modelBusy) return;
    setModelError(null);
    setModelBusy(true);
    try {
      setModels(await invoke<ModelsStatus>("download_models", { scope: "full" }));
    } catch (e) {
      setModelError(String(e));
      invoke<ModelsStatus>("check_models")
        .then((s) => setModels(s))
        .catch(() => {});
    } finally {
      setModelBusy(false);
      setModelProgress(null);
    }
  };

  const flashLexHint = (msg: string) => {
    setLexHint(msg);
    setTimeout(() => setLexHint(null), 2000);
  };

  const handleAddCandidate = async (word: string) => {
    if (lexBusy) return;
    setLexBusy(word);
    try {
      const next = await invoke<LexiconCandidate[]>("add_lexicon_entry", {
        word,
        alternatives: splitAlternatives(altInputs[word] ?? ""),
      });
      setCandidates(next);
      setAltInputs((prev) => {
        const { [word]: _drop, ...rest } = prev;
        return rest;
      });
      flashLexHint(`已加入词库：「${word}」（对之后的练习立即生效）`);
    } catch (e) {
      flashLexHint(String(e));
    } finally {
      setLexBusy(null);
    }
  };

  const handleDismissCandidate = async (word: string) => {
    if (lexBusy) return;
    setLexBusy(word);
    try {
      setCandidates(
        await invoke<LexiconCandidate[]>("dismiss_lexicon_candidate", { word }),
      );
    } catch (e) {
      flashLexHint(String(e));
    } finally {
      setLexBusy(null);
    }
  };

  const toggleRule = (key: RuleKey, checked: boolean) => {
    onUpdate({ ruleEnabled: { ...settings.ruleEnabled, [key]: checked } });
  };

  const handleSave = async () => {
    const outcome = await onSave(settings);
    setKeyWarning(outcome?.keyMessage ?? null);
    setSavedHint(true);
    setTimeout(() => setSavedHint(false), 1500);
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div className="max-h-[90vh] w-full max-w-lg overflow-y-auto rounded-xl bg-white shadow-xl">
        <div className="flex items-center justify-between border-b border-neutral-200 px-6 py-4">
          <h2 className="text-base font-semibold">设置</h2>
          <button
            onClick={onClose}
            className="rounded-lg px-2 py-1 text-sm text-neutral-500 hover:bg-neutral-100"
          >
            ✕
          </button>
        </div>

        <div className="space-y-5 px-6 py-5">
          {/* AI 后端（与首启向导共用配置块） */}
          <AiBackendFields settings={settings} onUpdate={onUpdate} onSave={onSave} />

          <div>
            <label className="mb-1 block text-sm font-medium text-neutral-700">
              Obsidian Vault 路径
            </label>
            <input
              value={settings.obsidianVaultPath}
              onChange={(e) => onUpdate({ obsidianVaultPath: e.target.value })}
              placeholder="D:\\Obsidian\\MyVault（留空则导出时用保存对话框）"
              className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
            />
            <p className="mt-1 text-xs text-neutral-400">
              配置后报告将直接写入 Vault 的「表达训练」目录
            </p>
          </div>

          <div>
            <label className="mb-1 block text-sm font-medium text-neutral-700">默认场景</label>
            <select
              value={settings.scenario}
              onChange={(e) => onUpdate({ scenario: e.target.value as Settings["scenario"] })}
              className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
            >
              {SCENARIOS.map((s) => (
                <option key={s.value} value={s.value}>
                  {s.label}
                </option>
              ))}
            </select>
          </div>

          {/* 实时规则 */}
          <div>
            <label className="mb-1 block text-sm font-medium text-neutral-700">
              实时规则（每条可单独关闭）
            </label>
            <div className="space-y-1.5 rounded-lg border border-neutral-200 p-3">
              {RULE_ITEMS.map(({ key, label }) => (
                <label key={key} className="flex items-center gap-2 text-sm text-neutral-700">
                  <input
                    type="checkbox"
                    checked={settings.ruleEnabled[key] !== false}
                    onChange={(e) => toggleRule(key, e.target.checked)}
                  />
                  {label}
                </label>
              ))}
            </div>
            <p className="mt-1 text-xs text-neutral-400">
              防刷屏是底线：同类提醒 3 句内不重复；口头禅按词计数不受此限
            </p>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="mb-1 block text-sm font-medium text-neutral-700">
                口头禅高频阈值
              </label>
              <input
                type="number"
                min={0}
                max={60}
                step={0.5}
                value={settings.fillerHighThreshold}
                onChange={(e) =>
                  onUpdate({ fillerHighThreshold: Number(e.target.value) || 0 })
                }
                className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
              />
              <p className="mt-1 text-xs text-neutral-400">次/分钟；中频词达到该频率才提醒</p>
            </div>
            <div>
              <label className="mb-1 block text-sm font-medium text-neutral-700">
                口头禅目标频率
              </label>
              <input
                type="number"
                min={0}
                max={60}
                step={0.5}
                value={settings.fillerGoalPerMin ?? ""}
                onChange={(e) =>
                  onUpdate({
                    fillerGoalPerMin:
                      e.target.value === "" || !Number.isFinite(Number(e.target.value))
                        ? null
                        : Number(e.target.value),
                  })
                }
                placeholder="可空，如：1"
                className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
              />
              <p className="mt-1 text-xs text-neutral-400">
                次/分钟；留空不设目标，历史页会显示与目标的差距
              </p>
            </div>
          </div>

          <div>
            <label className="mb-1 block text-sm font-medium text-neutral-700">
              自定义口头禅
            </label>
            <input
              value={settings.customFillers}
              onChange={(e) => onUpdate({ customFillers: e.target.value })}
              placeholder="逗号分隔，如：老铁, 绝绝子"
              className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
            />
            <p className="mt-1 text-xs text-neutral-400">合并进词库，视同高频词</p>
          </div>

          {/* 词库候选（词库自生长）：练习中反复出现、尚未收录的高频词 */}
          <div>
            <label className="mb-1 block text-sm font-medium text-neutral-700">
              词库候选
            </label>
            {candidates == null ? (
              <p className="text-xs text-neutral-400">加载中…</p>
            ) : candidates.length === 0 ? (
              <p className="rounded-lg border border-dashed border-neutral-200 px-3 py-3 text-xs text-neutral-400">
                暂无候选词。多次练习后，反复出现且未收录的高频词会出现在这里，可补充替代词或口头禅。
              </p>
            ) : (
              <div className="space-y-2 rounded-lg border border-neutral-200 p-3">
                {candidates.map((c) => (
                  <div key={c.word} className="flex flex-wrap items-center gap-2 text-sm">
                    <span className="font-medium">{c.word}</span>
                    <span className="text-xs text-neutral-400">{c.count} 次</span>
                    <input
                      value={altInputs[c.word] ?? ""}
                      onChange={(e) =>
                        setAltInputs((prev) => ({ ...prev, [c.word]: e.target.value }))
                      }
                      placeholder="替代词（逗号分隔；留空则加入为口头禅）"
                      className="min-w-40 flex-1 rounded border border-neutral-300 px-2 py-1 text-sm"
                    />
                    <button
                      onClick={() => handleAddCandidate(c.word)}
                      disabled={lexBusy !== null}
                      className="rounded bg-neutral-900 px-2.5 py-1 text-xs font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
                    >
                      加入词库
                    </button>
                    <button
                      onClick={() => handleDismissCandidate(c.word)}
                      disabled={lexBusy !== null}
                      className="rounded border border-neutral-300 px-2.5 py-1 text-xs text-neutral-600 hover:bg-neutral-100 disabled:opacity-50"
                    >
                      忽略
                    </button>
                  </div>
                ))}
              </div>
            )}
            {lexHint && <p className="mt-1 text-xs text-green-700">{lexHint}</p>}
            <p className="mt-1 text-xs text-neutral-400">
              候选来自你的练习逐字稿（仅统计本机文本）；加入后立即生效，不重启应用
            </p>
          </div>

          {/* 会话录音（回放） */}
          <div className="rounded-lg border border-neutral-200 p-3">
            <label className="flex items-center gap-2 text-sm font-medium text-neutral-700">
              <input
                type="checkbox"
                checked={settings.recordAudio}
                onChange={(e) => onUpdate({ recordAudio: e.target.checked })}
              />
              会话录音（结束后可逐句回放）
            </label>
            <p className="mt-1 text-xs text-neutral-400">
              音频只保存在本机的应用数据目录，不上传、不联网；单次录音最长保留 30 分钟
            </p>
          </div>

          {/* 识别热词 */}
          <div>
            <div className="mb-1 flex items-baseline justify-between">
              <label className="block text-sm font-medium text-neutral-700">识别热词</label>
              <label className="flex items-center gap-1 text-xs text-neutral-500">
                权重
                <input
                  type="number"
                  min={0.5}
                  max={5}
                  step={0.1}
                  value={settings.hotwordsScore ?? ""}
                  onChange={(e) =>
                    onUpdate({
                      hotwordsScore:
                        e.target.value === "" || !Number.isFinite(Number(e.target.value))
                          ? null
                          : Number(e.target.value),
                    })
                  }
                  placeholder="默认 1.5"
                  className="w-20 rounded border border-neutral-300 px-2 py-1"
                />
              </label>
            </div>
            <input
              value={settings.hotwords}
              onChange={(e) => onUpdate({ hotwords: e.target.value })}
              placeholder="逗号或换行分隔，如：DeepSeek, 米糕, 小米SU7"
              className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
            />
            <p className="mt-1 text-xs text-neutral-400">
              提示识别引擎优先输出这些人名/术语；权重常用 1.5–2.5，越大越倾向热词。当前内置的
              Paraformer 模型不参与解码级热词，建议同时配置下方的纠错映射
            </p>
          </div>

          {/* 识别纠错映射 */}
          <div>
            <label className="mb-1 block text-sm font-medium text-neutral-700">
              识别纠错映射
            </label>
            <textarea
              value={settings.asrCorrections}
              onChange={(e) => onUpdate({ asrCorrections: e.target.value })}
              placeholder={"每行一条「错->对」，如：\n深seek->DeepSeek\n米糕->米糕（品牌）"}
              rows={3}
              className="w-full resize-y rounded-lg border border-neutral-300 px-3 py-2 text-sm"
            />
            <p className="mt-1 text-xs text-neutral-400">
              这是识别结果的文本纠错，不是热词：不改识别过程，只在句子定稿入分析前把「错」替换为「对」
            </p>
          </div>

          {/* 识别：双引擎终稿 + 断句灵敏度 */}
          <div className="rounded-lg border border-neutral-200 p-3">
            <label className="flex items-center gap-2 text-sm font-medium text-neutral-700">
              <input
                type="checkbox"
                checked={settings.precisionFinals}
                onChange={(e) => onUpdate({ precisionFinals: e.target.checked })}
              />
              高精度终稿（双引擎）
            </label>
            <p className="mt-1 text-xs text-neutral-400">
              句子定稿时用离线高精引擎（SenseVoice，约 230MB，首启引导默认下载）重新识别，
              标点与数字更规范；字幕实时性不受影响。未安装时可在下方「识别组件」补装；
              未安装或关闭时沿用流式引擎终稿
            </p>
            <div className="mt-3">
              <label className="flex items-center gap-2 text-sm font-medium text-neutral-700">
                <input
                  type="checkbox"
                  checked={settings.showLivePreview}
                  onChange={(e) => onUpdate({ showLivePreview: e.target.checked })}
                />
                显示实时识别预览（关闭后仅显示每句定稿，观感更稳）
              </label>
              <p className="mt-1 text-xs text-neutral-400">
                练习中栏以每句定稿为主；开启后在定稿句下方以灰色小字显示「识别中…」的
                流式临时内容（流式结果天然比定稿粗略，仅供参考）
              </p>
            </div>
            <div className="mt-3">
              <label className="mb-1 block text-sm font-medium text-neutral-700">断句灵敏度</label>
              <select
                value={settings.vadSensitivity}
                onChange={(e) =>
                  onUpdate({ vadSensitivity: e.target.value as Settings["vadSensitivity"] })
                }
                className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
              >
                <option value="standard">标准（默认）</option>
                <option value="high">高灵敏度（远距离 / 小声录音）</option>
              </select>
              <p className="mt-1 text-xs text-neutral-400">
                高灵敏度更容易保住轻尾音、不易漏切小声句；嘈杂环境可能把噪声误判成语音多切几句
              </p>
            </div>
            <div className="mt-3">
              <label className="flex items-center gap-2 text-sm font-medium text-neutral-700">
                <input
                  type="checkbox"
                  checked={settings.enhanceAudio}
                  onChange={(e) => onUpdate({ enhanceAudio: e.target.checked })}
                />
                录音增强（自动增益，适合过静的手机录音）
              </label>
              <p className="mt-1 text-xs text-neutral-400">
                「从文件练习」解码后检测整段电平，过低时自动放大（最多 8 倍），改善过静录音的断句与识别；
                仅对文件练习生效，麦克风实时录音暂不启用。过静录音建议与上面的「高灵敏度」断句同时开启，两者互补
              </p>
            </div>
          </div>

          {/* 识别组件：三组模型在位状态，SenseVoice 缺失时可补装 */}
          <div className="rounded-lg border border-neutral-200 p-3">
            <label className="block text-sm font-medium text-neutral-700">识别组件</label>
            {models == null ? (
              <p className="mt-2 text-xs text-neutral-400">无法检测组件状态（非应用环境）</p>
            ) : (
              <>
                <div className="mt-2 space-y-1.5">
                  <ModelRow
                    ok={!models.missing.includes("silero_vad.onnx")}
                    label="断句模型（Silero VAD）"
                  />
                  <ModelRow
                    ok={!models.missing.some((m) => m.startsWith(PARA_DIR_PREFIX))}
                    label="实时识别引擎（流式 Paraformer）"
                  />
                  <ModelRow
                    ok={(models.recommendedMissing ?? models.optionalMissing ?? []).length === 0}
                    label="高精度终稿引擎（SenseVoice）"
                  />
                </div>
                {(models.missing.length > 0 ||
                  (models.recommendedMissing ?? models.optionalMissing ?? []).length > 0) &&
                  !modelBusy && (
                    <>
                      {(models.recommendedMissing ?? models.optionalMissing ?? []).length > 0 && (
                        <p className="mt-2 text-xs text-amber-700">
                          高精引擎未安装：句子定稿精度会明显下降，且不含标点
                        </p>
                      )}
                      <button
                        onClick={installModels}
                        className="mt-1 rounded-lg bg-neutral-900 px-3 py-1.5 text-xs font-medium text-white hover:bg-neutral-700"
                      >
                        {models.missing.length > 0
                          ? "下载缺失组件（推荐全量）"
                          : "补装高精引擎（推荐）"}
                      </button>
                    </>
                  )}
                {modelBusy && (
                  <div className="mt-2">
                    <div className="mb-1 flex items-center justify-between text-xs">
                      <span>
                        正在下载 {downloadFileLabel(modelProgress?.file ?? "")}（可关闭应用，下次续传）
                      </span>
                      {modelProgress && (
                        <span className="text-neutral-500">
                          {formatBytes(modelProgress.received)}
                          {modelProgress.total > 0
                            ? ` / ${formatBytes(modelProgress.total)}`
                            : "（大小未知）"}
                        </span>
                      )}
                    </div>
                    <div className="h-2 overflow-hidden rounded-full bg-neutral-100">
                      <div
                        className={`h-full bg-neutral-900 ${
                          !modelProgress || progressPercent(modelProgress.received, modelProgress.total) === null
                            ? "w-1/3 animate-pulse"
                            : ""
                        }`}
                        style={
                          modelProgress
                            ? {
                                width: `${progressPercent(modelProgress.received, modelProgress.total) ?? 0}%`,
                              }
                            : undefined
                        }
                      />
                    </div>
                  </div>
                )}
                {modelError && (
                  <p className="mt-2 whitespace-pre-wrap text-xs text-red-600">{modelError}</p>
                )}
                <p className="mt-1 text-xs text-neutral-400">
                  推荐全量约 460MB（三组组件全装，模型在 {models.modelsDir}）；
                  跳过 SenseVoice 会导致定稿精度明显下降
                </p>
              </>
            )}
          </div>

          {/* AI 周期快评 */}
          <div className="rounded-lg border border-neutral-200 p-3">
            <label className="flex items-center gap-2 text-sm font-medium text-neutral-700">
              <input
                type="checkbox"
                checked={settings.realtimeCheckinEnabled}
                onChange={(e) => onUpdate({ realtimeCheckinEnabled: e.target.checked })}
              />
              AI 周期快评（练习中每段时间让 AI 看一眼：跑偏 / 矛盾 / 该收结论）
            </label>
            {settings.realtimeCheckinEnabled && (
              <div className="mt-3 grid grid-cols-2 gap-3">
                <div>
                  <label className="mb-1 block text-sm text-neutral-600">频率（秒）</label>
                  <input
                    type="number"
                    min={15}
                    max={600}
                    step={5}
                    value={settings.realtimeCheckinIntervalSec}
                    onChange={(e) =>
                      onUpdate({
                        realtimeCheckinIntervalSec: Number(e.target.value) || 45,
                      })
                    }
                    className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
                  />
                </div>
                <p className="self-end text-xs text-neutral-400">
                  需要 API Key（或 Ollama）；无新句子时自动跳过，同类提醒 90 秒冷却
                </p>
              </div>
            )}
            {settings.realtimeCheckinEnabled && !hasRemoteBackend(settings) && (
              <p className="mt-2 text-xs text-amber-600">
                尚未配置 API Key，快评不会实际发起请求
              </p>
            )}
          </div>

          <p className="rounded-lg bg-neutral-50 px-3 py-2 text-xs text-neutral-500">
            {hasRemoteBackend(settings)
              ? "当前已配置远端 AI 后端，可生成完整 AI 报告。"
              : "尚未配置 API Key，报告将使用本地降级模式（纯统计 + 规则汇总）。"}
          </p>
        </div>

        <div className="flex items-center justify-end gap-3 border-t border-neutral-200 px-6 py-4">
          {keyWarning ? (
            <span className="max-w-72 text-left text-xs text-amber-600" title={keyWarning}>
              {keyWarning}
            </span>
          ) : (
            savedHint && <span className="text-sm text-green-600">已保存</span>
          )}
          <button
            onClick={onClose}
            className="rounded-lg border border-neutral-300 px-4 py-2 text-sm text-neutral-700 hover:bg-neutral-100"
          >
            关闭
          </button>
          <button
            onClick={handleSave}
            className="rounded-lg bg-neutral-900 px-4 py-2 text-sm font-medium text-white hover:bg-neutral-700"
          >
            保存
          </button>
        </div>
      </div>
    </div>
  );
}
