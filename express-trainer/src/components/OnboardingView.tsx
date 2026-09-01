import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AiBackendFields } from "./AiBackendFields";
import {
  downloadFileLabel,
  formatBytes,
  progressPercent,
  type DownloadProgressState,
} from "../lib/onboarding";
import type { ModelsStatus, Settings } from "../types";

interface Props {
  settings: Settings;
  onUpdate: (patch: Partial<Settings>) => void;
  onSave: (next: Settings) => Promise<unknown>;
  /** 完成引导：写 onboardingDone 并进入主界面 */
  onFinish: () => Promise<void>;
}

const STEPS = ["下载引擎", "AI 后端（可选）", "开始使用"];

/** 首启引导三步向导（M4）：引擎下载（默认全量，含高精终稿引擎）→ AI 后端配置（可跳过）→ 试录指引 */
export function OnboardingView({ settings, onUpdate, onSave, onFinish }: Props) {
  const [step, setStep] = useState(1);
  const [status, setStatus] = useState<ModelsStatus | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [progress, setProgress] = useState<Record<string, DownloadProgressState>>({});
  const [error, setError] = useState<string | null>(null);
  // 「仅下载必需组件」的两步确认：先点次级链接，再显式确认
  const [confirmSkip, setConfirmSkip] = useState(false);
  const [finishing, setFinishing] = useState(false);
  const unlisteners = useRef<(() => void)[]>([]);

  const modelsReady = status !== null && status.missing.length === 0;
  /** 高精终稿引擎（SenseVoice）未安装：0.2.2 起默认必装，引导补装（定稿精度明显下降） */
  const recommendedPending =
    modelsReady &&
    ((status?.recommendedMissing ?? status?.optionalMissing)?.length ?? 0) > 0;

  // 事件监听 + 首次完整性检查
  useEffect(() => {
    let cancelled = false;
    const regs = [
      listen<DownloadProgressState & { file: string }>("download_progress", (e) => {
        const { file, received, total } = e.payload;
        setProgress((prev) => ({
          ...prev,
          [file]: { received, total },
        }));
      }),
      listen<ModelsStatus>("download_done", (_e) => {
        // 必需模型就绪；可选引擎状态以下载命令的返回值/check 为准
        setStatus((prev) => (prev ? { ...prev, missing: [] } : prev));
      }),
    ];
    Promise.all(regs).then((fns) => {
      if (cancelled) fns.forEach((f) => f());
      else unlisteners.current = fns;
    });
    invoke<ModelsStatus>("check_models")
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch((e) => {
        if (!cancelled) setError(`模型检查失败：${String(e)}`);
      });
    return () => {
      cancelled = true;
      unlisteners.current.forEach((f) => f());
      unlisteners.current = [];
    };
  }, []);

  /** 下载识别引擎：默认全量（必需 + SenseVoice 高精引擎）；仅当用户显式
   *  确认后才传 requiredOnly 跳过高精引擎。失败时刷新状态——必需组件可能
   *  已就绪（如仅高精引擎下载失败），别让「下一步」被卡住 */
  const startDownload = async (scope: "full" | "requiredOnly") => {
    setError(null);
    setProgress({});
    setDownloading(true);
    try {
      const s = await invoke<ModelsStatus>("download_models", { scope });
      setStatus(s);
    } catch (e) {
      setError(String(e));
      invoke<ModelsStatus>("check_models")
        .then((s) => setStatus(s))
        .catch(() => {});
    } finally {
      setDownloading(false);
      setConfirmSkip(false);
    }
  };

  const goToStep3 = async () => {
    await onSave(settings);
    setStep(3);
  };

  const finish = async () => {
    setFinishing(true);
    try {
      await onFinish();
    } finally {
      setFinishing(false);
    }
  };

  const progressEntries = Object.entries(progress);

  return (
    <div className="flex min-h-screen flex-col bg-neutral-50 text-neutral-900">
      {/* 步骤指示 */}
      <div className="border-b border-neutral-200 bg-white px-6 py-4">
        <div className="mx-auto flex max-w-xl items-center gap-2">
          <span className="text-base font-semibold">SpeakMirror 表达镜</span>
          <span className="text-sm text-neutral-400">· 首次使用引导</span>
        </div>
        <div className="mx-auto mt-3 flex max-w-xl items-center gap-2">
          {STEPS.map((label, i) => {
            const n = i + 1;
            const active = step === n;
            const done = step > n;
            return (
              <div key={label} className="flex flex-1 items-center gap-2">
                <span
                  className={`flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-xs font-semibold ${
                    done
                      ? "bg-green-500 text-white"
                      : active
                        ? "bg-neutral-900 text-white"
                        : "border border-neutral-300 text-neutral-400"
                  }`}
                >
                  {done ? "✓" : n}
                </span>
                <span className={`text-xs ${active ? "font-medium" : "text-neutral-400"}`}>
                  {label}
                </span>
                {i < STEPS.length - 1 && <span className="h-px flex-1 bg-neutral-200" />}
              </div>
            );
          })}
        </div>
      </div>

      <main className="mx-auto w-full max-w-xl flex-1 px-6 py-8">
        {step === 1 && (
          <section>
            <h2 className="text-lg font-semibold">
              第 1 步 · 下载识别引擎（约 460MB，含高精度终稿引擎，只需一次）
            </h2>
            <p className="mt-1 text-sm text-neutral-500">
              全部离线运行，只需下载一次。推荐完整安装：实时识别引擎（约 230MB）
              负责滚动字幕，高精度终稿引擎（SenseVoice，安装后约 230MB）负责每句
              定稿——精度与标点明显更好。下载源会自动按 ghfast.top 反代 →
              HuggingFace / hf-mirror 顺序尝试；中断后可断点续传。
            </p>

            {status === null && !error && (
              <p className="mt-6 text-sm text-neutral-400">正在检查模型…</p>
            )}

            {modelsReady && !recommendedPending && (
              <div className="mt-6 rounded-lg border border-green-200 bg-green-50 px-4 py-3 text-sm text-green-700">
                识别引擎已全部就绪（位于 {status?.modelsDir}）。
              </div>
            )}

            {/* 高精终稿引擎缺失：0.2.2 起默认必装，引导补装（未安装时终稿回退流式引擎） */}
            {recommendedPending && !downloading && (
              <div className="mt-4 rounded-lg border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
                <p className="font-medium">
                  高精度终稿引擎（SenseVoice）未安装：句子定稿精度会明显下降，且不含标点
                </p>
                <p className="mt-1 text-xs">
                  推荐现在补装（压缩包约 1GB，安装后约 230MB，支持断点续传）；
                  也可稍后在「设置 → 识别组件」中补装。
                </p>
                <button
                  onClick={() => startDownload("full")}
                  className="mt-2 rounded-lg bg-neutral-900 px-4 py-2 text-xs font-medium text-white hover:bg-neutral-700"
                >
                  补装高精引擎（推荐）
                </button>
              </div>
            )}

            {!modelsReady && (
              <>
                <div className="mt-6">
                  {downloading ? (
                    <div className="space-y-4 rounded-lg border border-neutral-200 bg-white p-4">
                      <p className="text-sm font-medium">正在下载…（请保持网络通畅，可随时关闭应用，下次继续）</p>
                      {progressEntries.length === 0 && (
                        <p className="text-sm text-neutral-400">正在连接下载源…</p>
                      )}
                      {progressEntries.map(([file, p]) => {
                        const pct = progressPercent(p.received, p.total);
                        return (
                          <div key={file}>
                            <div className="mb-1 flex items-center justify-between text-xs">
                              <span>{downloadFileLabel(file)}</span>
                              <span className="text-neutral-500">
                                {formatBytes(p.received)}
                                {p.total > 0 ? ` / ${formatBytes(p.total)}` : "（大小未知）"}
                              </span>
                            </div>
                            <div className="h-2 overflow-hidden rounded-full bg-neutral-100">
                              <div
                                className={`h-full bg-neutral-900 ${pct === null ? "w-1/3 animate-pulse" : ""}`}
                                style={pct !== null ? { width: `${pct}%` } : undefined}
                              />
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  ) : (
                    <button
                      onClick={() => startDownload("full")}
                      className="rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700"
                    >
                      {error ? "重试下载（推荐·完整）" : "开始下载（推荐·完整）"}
                    </button>
                  )}
                </div>

                {error && (
                  <div className="mt-4 whitespace-pre-wrap rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
                    {error}
                  </div>
                )}

                {/* 跳过路径必须显式：折叠 → 次级链接 → 确认文案，确认后才只下必需组件 */}
                {!downloading && (
                  <details className="mt-4 rounded-lg border border-neutral-200 bg-white px-4 py-3 text-sm">
                    <summary className="cursor-pointer font-medium text-neutral-500">
                      网络或磁盘受限？查看精简安装选项
                    </summary>
                    {confirmSkip ? (
                      <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-3 text-xs text-amber-800">
                        <p>
                          不装高精引擎，句子定稿精度会明显下降，且不含标点。可在设置里随时补装。
                        </p>
                        <div className="mt-2 flex items-center gap-2">
                          <button
                            onClick={() => startDownload("requiredOnly")}
                            className="rounded-lg bg-amber-700 px-3 py-1.5 font-medium text-white hover:bg-amber-800"
                          >
                            确认，仅下载必需组件（约 230MB）
                          </button>
                          <button
                            onClick={() => setConfirmSkip(false)}
                            className="rounded-lg border border-amber-300 px-3 py-1.5 text-amber-700 hover:bg-amber-100"
                          >
                            取消
                          </button>
                        </div>
                      </div>
                    ) : (
                      <div className="mt-3 space-y-2">
                        <p className="text-xs text-neutral-500">
                          只下载实时识别引擎（Paraformer + 断句模型，约 230MB），
                          跳过高精度终稿引擎（SenseVoice）。
                        </p>
                        <button
                          onClick={() => setConfirmSkip(true)}
                          className="text-xs text-neutral-400 underline hover:text-neutral-600"
                        >
                          仅下载必需组件（不推荐，识别精度会明显下降）
                        </button>
                      </div>
                    )}
                  </details>
                )}

                <details className="mt-6 rounded-lg border border-neutral-200 bg-white px-4 py-3 text-sm">
                  <summary className="cursor-pointer font-medium text-neutral-700">
                    手动下载（自动下载反复失败时）
                  </summary>
                  <div className="mt-3 space-y-2 text-neutral-600">
                    <p>从源码运行：在项目根目录执行以下命令，完成后重启应用：</p>
                    <pre className="overflow-x-auto rounded bg-neutral-100 px-3 py-2 text-xs">
                      powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1
                    </pre>
                    <p>
                      安装版手动放置：从发布页下载 silero_vad.onnx 与
                      sherpa-onnx-streaming-paraformer-bilingual-zh-en.tar.bz2，解压到
                      %APPDATA%\com.speakmirror.desktop\models（保留 tokens.txt 与两个
                      *.int8.onnx）。
                    </p>
                  </div>
                </details>
              </>
            )}

            <div className="mt-8 flex items-center justify-between">
              <span />
              <button
                disabled={!modelsReady}
                onClick={() => setStep(2)}
                className="rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-40"
              >
                下一步
              </button>
            </div>
          </section>
        )}

        {step === 2 && (
          <section>
            <h2 className="text-lg font-semibold">第 2 步 · 配置 AI 后端（可跳过）</h2>
            <p className="mt-1 text-sm text-neutral-500">
              不配置也能完整使用：实时字幕、词库反馈、声音仪表、本地统计报告全部离线可用。
              配置后可生成逐句改写的 AI 完整报告与练习中周期快评；API Key 仅保存在本机。
            </p>
            <div className="mt-5 rounded-lg border border-neutral-200 bg-white p-4">
              <AiBackendFields settings={settings} onUpdate={onUpdate} onSave={onSave} />
            </div>
            <div className="mt-8 flex items-center justify-between">
              <button
                onClick={() => setStep(1)}
                className="rounded-lg border border-neutral-300 px-4 py-2 text-sm text-neutral-700 hover:bg-neutral-100"
              >
                上一步
              </button>
              <button
                onClick={goToStep3}
                className="rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700"
              >
                {settings.apiKey.trim() || settings.aiBackend === "ollama" ? "保存并继续" : "跳过，继续"}
              </button>
            </div>
          </section>
        )}

        {step === 3 && (
          <section>
            <h2 className="text-lg font-semibold">第 3 步 · 试录 30 秒</h2>
            <ol className="mt-3 list-decimal space-y-2 pl-5 text-sm text-neutral-700">
              <li>进入主界面，在顶栏选择场景（自由练习 / 面试回答），可填写主题或面试题</li>
              <li>点「开始练习」，对着麦克风说 30 秒——试试自我介绍</li>
              <li>左边看语速与声音仪表，中间字幕里的口头禅会标红，右边是实时提醒</li>
              <li>点「结束练习」→ 生成报告（未配 AI 时为本地统计报告）</li>
            </ol>
            <p className="mt-4 rounded-lg bg-neutral-100 px-4 py-3 text-sm text-neutral-600">
              首次使用建议先用「自由练习」熟悉节奏；麦克风权限弹窗请允许，否则无法采集声音。
              没有麦克风也可以用顶栏的「从文件练习」：选一段 wav/mp3 录音，应用会按 1 倍速走完全相同的实时分析。
            </p>
            <div className="mt-8 flex items-center justify-between">
              <button
                onClick={() => setStep(2)}
                className="rounded-lg border border-neutral-300 px-4 py-2 text-sm text-neutral-700 hover:bg-neutral-100"
              >
                上一步
              </button>
              <button
                onClick={finish}
                disabled={finishing}
                className="rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-40"
              >
                {finishing ? "进入中…" : "进入主界面"}
              </button>
            </div>
          </section>
        )}
      </main>

      <footer className="border-t border-neutral-200 bg-white px-6 py-3">
        <div className="mx-auto flex max-w-xl justify-end">
          <button
            onClick={finish}
            className="text-xs text-neutral-400 underline hover:text-neutral-600"
          >
            跳过引导，直接进入
          </button>
        </div>
      </footer>
    </div>
  );
}
