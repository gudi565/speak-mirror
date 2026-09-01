import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { QaItem, ReportMode, ReportStatus, Scenario } from "../types";

export interface GenerateArgs {
  scenario: Scenario;
  topic: string;
  transcript: string | null;
  /** 模拟面试（mockInterview）必传：逐题问答（Rust 由此合并逐字稿与统计） */
  qa?: QaItem[] | null;
  /** 报告模式：full（完整八节）/ quick（快速四节）；缺省 full（Rust 同口径缺省） */
  mode?: ReportMode;
}

/** generate_report 命令的 invoke 参数（纯函数可测：mode 等参数传递契约） */
export function generateInvokeArgs(args: GenerateArgs) {
  return {
    scenario: args.scenario,
    topic: args.topic.trim() || null,
    transcript: args.transcript,
    qa: args.qa ?? null,
    mode: args.mode ?? "full",
  };
}

/**
 * 会话结束报告状态机。
 * 事件契约（与 Rust report.rs 对应）：
 * - report_chunk { text }  流式增量
 * - report_done  { text }  完成（含全文）
 * - report_error { message }
 */
export function useReport() {
  const [status, setStatus] = useState<ReportStatus>("idle");
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const lastArgs = useRef<GenerateArgs | null>(null);
  // 流式进行中防重入：极快双击/多入口并发会导致 report_chunk 交错串流
  const generatingRef = useRef(false);

  useEffect(() => {
    let cancelled = false;
    const regs = [
      listen<{ text: string }>("report_chunk", (e) => {
        setText((prev) => prev + e.payload.text);
        setStatus((s) => (s === "error" ? s : "streaming"));
      }),
      listen<{ text: string }>("report_done", (e) => {
        setText(e.payload.text);
        setStatus("done");
        setError(null);
      }),
      listen<{ message: string }>("report_error", (e) => {
        setError(e.payload.message);
        setStatus("error");
      }),
    ];
    Promise.all(regs).then((fns) => {
      if (cancelled) fns.forEach((f) => f());
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const generate = useCallback(async (args: GenerateArgs) => {
    if (generatingRef.current) return;
    generatingRef.current = true;
    setText("");
    setError(null);
    setStatus("streaming");
    // lastArgs 存全量参数（含 mode）：ReportView 的「重试」沿用同一模式
    lastArgs.current = args;
    try {
      // 返回值 = 报告全文；渲染主要靠 report_chunk / report_done 事件
      const full = await invoke<string>("generate_report", generateInvokeArgs(args));
      setText(full);
      setStatus("done");
    } catch (e) {
      setError(String(e));
      setStatus("error");
    } finally {
      generatingRef.current = false;
    }
  }, []);

  const retry = useCallback(() => {
    if (lastArgs.current) generate(lastArgs.current);
  }, [generate]);

  const reset = useCallback(() => {
    setText("");
    setError(null);
    setStatus("idle");
    lastArgs.current = null;
  }, []);

  return { status, text, error, generate, retry, reset };
}
