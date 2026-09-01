import { useMemo, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { invoke } from "@tauri-apps/api/core";
import { scenarioLabel } from "../lib/scenarios";
import { layoutSkeleton, reportModeLabel, shouldUseSkeleton } from "../lib/report";
import type { ReportMode, ReportStatus, SaveOutcome, Scenario } from "../types";

interface Props {
  status: ReportStatus;
  text: string;
  error: string | null;
  scenario: Scenario;
  topic: string;
  transcript: string | null;
  /** 本次报告的模式（骨架节结构按 mode 取八节/四节；重试沿用同一模式） */
  mode: ReportMode;
  onRetry: () => void;
  onRestart: () => void;
}

function today(): string {
  const d = new Date();
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${mm}-${dd}`;
}

async function invokeSave(kind: "report" | "transcript", topic: string, content: string) {
  const safeTopic = topic.trim().replace(/[\/\\:*?"<>|]/g, "_").slice(0, 50);
  return invoke<SaveOutcome>("export_obsidian", {
    content,
    title: safeTopic,
    kind,
  });
}

/**
 * 流式中的骨架视图：顶部进度条（x / N 节）+ 各节标题骨架。
 * 已到的节按流入标题渲染正文；未到的节浅灰占位「生成中…」；
 * 完成的节标题前打 ✓。骨架分节逻辑在 lib/report.ts（纯函数，有测试）。
 */
function SkeletonView({ text, mode }: { text: string; mode: ReportMode }) {
  const layout = useMemo(() => layoutSkeleton(text, mode), [text, mode]);
  const pct = Math.round((layout.doneCount / Math.max(layout.total, 1)) * 100);
  return (
    <div>
      <div className="mb-4">
        <div className="mb-1 flex items-center justify-between text-xs text-neutral-400">
          <span>{reportModeLabel(mode)} · 逐节生成</span>
          <span className="tabular-nums">
            {layout.doneCount} / {layout.total} 节
          </span>
        </div>
        <div className="h-1 overflow-hidden rounded bg-neutral-100">
          <div
            className="h-full rounded bg-neutral-400 transition-all duration-300"
            style={{ width: `${pct}%` }}
          />
        </div>
      </div>
      {layout.preamble && (
        <div className="mb-2 text-neutral-600">
          <Markdown remarkPlugins={[remarkGfm]}>{layout.preamble}</Markdown>
        </div>
      )}
      <div>
        {layout.rows.map((row) => (
          <section key={row.index}>
            <h2 className="flex items-center gap-2">
              {row.done && <span className="text-green-600">✓</span>}
              {row.title}
            </h2>
            {row.body ? (
              <Markdown remarkPlugins={[remarkGfm]}>{row.body}</Markdown>
            ) : (
              <p className="text-neutral-300">生成中…</p>
            )}
          </section>
        ))}
      </div>
    </div>
  );
}

export function ReportView({
  status,
  text,
  error,
  scenario,
  topic,
  transcript,
  mode,
  onRetry,
  onRestart,
}: Props) {
  const [hint, setHint] = useState<string | null>(null);
  const busy = status === "streaming";
  // AI 报告（契约：无 h1、按 ## 分节）走骨架；本地降级报告（h1 开头、
  // 节结构不同）秒出且直接整篇渲染，不套骨架
  const useSkeleton = busy && shouldUseSkeleton(text);

  const flash = (msg: string) => {
    setHint(msg);
    setTimeout(() => setHint(null), 2500);
  };

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      flash("已复制到剪贴板");
    } catch {
      flash("复制失败（剪贴板不可用）");
    }
  };

  const handleSaveReport = async () => {
    try {
      const name = topic.trim()
        ? `${today()}-${topic.trim().replace(/[\/\\:*?"<>|]/g, "_").slice(0, 50)}-报告.md`
        : `${today()}-表达训练报告.md`;
      const outcome = await invoke<SaveOutcome>("save_text_file", {
        content: text,
        suggestedName: name,
      });
      flash(outcome.saved ? `已保存到 ${outcome.path}` : "已取消保存");
    } catch (e) {
      flash(`保存失败：${e}`);
    }
  };

  const handleExportObsidian = async (kind: "report" | "transcript") => {
    const content = kind === "report" ? text : (transcript ?? "");
    if (!content.trim()) {
      flash(kind === "report" ? "报告内容为空" : "逐字稿为空");
      return;
    }
    try {
      const outcome = await invokeSave(kind, topic, content);
      flash(outcome.saved ? `已导出到 ${outcome.path}` : "已取消导出");
    } catch (e) {
      flash(`导出失败：${e}`);
    }
  };

  return (
    <div className="mx-auto flex min-h-0 max-w-3xl flex-1 flex-col px-6 py-6">
      <div className="flex items-center justify-between">
        <h2 className="flex items-center gap-2 text-lg font-semibold">
          {scenarioLabel(scenario)}报告
          <span
            title={mode === "quick" ? "四节快速报告" : "八节完整报告"}
            className="rounded-full border border-neutral-200 bg-neutral-50 px-2 py-0.5 text-xs font-normal text-neutral-500"
          >
            {reportModeLabel(mode)}
          </span>
        </h2>
        <div className="flex items-center gap-3 text-sm">
          {hint && <span className="max-w-72 truncate text-neutral-500">{hint}</span>}
          {status === "streaming" && <span className="text-neutral-400">生成中…</span>}
          <button
            onClick={onRestart}
            disabled={busy}
            className="rounded-lg border border-neutral-300 px-4 py-2 text-neutral-700 hover:bg-neutral-100 disabled:opacity-50"
          >
            再练一次
          </button>
        </div>
      </div>

      {status === "error" ? (
        <div className="mt-6 rounded-xl border border-red-200 bg-red-50 px-5 py-6 text-sm">
          <div className="font-medium text-red-700">报告生成失败</div>
          <div className="mt-1 break-all text-red-600">{error}</div>
          <div className="mt-4 flex gap-3">
            <button
              onClick={onRetry}
              className="rounded-lg bg-red-600 px-4 py-2 text-sm font-medium text-white hover:bg-red-500"
            >
              重试
            </button>
            <button
              onClick={onRestart}
              className="rounded-lg border border-red-300 px-4 py-2 text-sm text-red-700 hover:bg-red-100"
            >
              返回
            </button>
          </div>
        </div>
      ) : (
        <>
          <article className="report-md mt-4 min-h-0 flex-1 overflow-y-auto rounded-xl border border-neutral-200 bg-white px-6 py-5 text-sm leading-7">
            {useSkeleton ? (
              <SkeletonView text={text} mode={mode} />
            ) : (
              <Markdown remarkPlugins={[remarkGfm]}>{text || (busy ? "正在连接模型…" : "")}</Markdown>
            )}
          </article>

          {status === "done" && (
            <div className="mt-4 flex flex-wrap items-center gap-3">
              <button
                onClick={handleCopy}
                className="rounded-lg bg-neutral-900 px-4 py-2 text-sm font-medium text-white hover:bg-neutral-700"
              >
                复制全文
              </button>
              <button
                onClick={handleSaveReport}
                className="rounded-lg border border-neutral-300 px-4 py-2 text-sm text-neutral-700 hover:bg-neutral-100"
              >
                保存 .md
              </button>
              <button
                onClick={() => handleExportObsidian("report")}
                className="rounded-lg border border-neutral-300 px-4 py-2 text-sm text-neutral-700 hover:bg-neutral-100"
              >
                导出到 Obsidian
              </button>
              <button
                onClick={() => handleExportObsidian("transcript")}
                className="rounded-lg border border-neutral-300 px-4 py-2 text-sm text-neutral-700 hover:bg-neutral-100"
              >
                导出逐字稿
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}
