import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Scenario } from "../types";
import {
  appendFollowupChunk,
  beginFollowup,
  failFollowup,
  finishFollowup,
  followupInvokeArgs,
  retryFollowupAt,
  type FollowupItem,
} from "../lib/followup";

/** 一轮追问的远端上下文（每轮都带完整报告；历史追问不重复发送） */
export interface FollowupContext {
  scenario: Scenario;
  reportText: string;
}

/**
 * 报告追问状态机（报告页「追问与解答」分区用）。
 * 事件契约（与 Rust report.rs 对应）：
 * - followup_chunk { text }  回答流式增量
 * - followup_done  { text }  回答完成（含全文）
 * - followup_error { message }
 * 事件与 invoke 返回双保险落定（状态机只作用于「流式中」的条目，幂等）；
 * generatingRef 防并发（与 useReport 同模式）。
 */
export function useFollowup() {
  const [items, setItems] = useState<FollowupItem[]>([]);
  const [busy, setBusy] = useState(false);
  const generatingRef = useRef(false);
  const lastContext = useRef<FollowupContext | null>(null);

  useEffect(() => {
    let cancelled = false;
    const regs = [
      listen<{ text: string }>("followup_chunk", (e) => {
        setItems((prev) => appendFollowupChunk(prev, e.payload.text));
      }),
      listen<{ text: string }>("followup_done", (e) => {
        setItems((prev) => finishFollowup(prev, e.payload.text));
      }),
      listen<{ message: string }>("followup_error", (e) => {
        setItems((prev) => failFollowup(prev, e.payload.message));
      }),
    ];
    Promise.all(regs).then((fns) => {
      if (cancelled) fns.forEach((f) => f());
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const ask = useCallback(async (question: string, ctx: FollowupContext) => {
    const q = question.trim();
    if (!q || generatingRef.current) return;
    generatingRef.current = true;
    setBusy(true);
    lastContext.current = ctx;
    setItems((prev) => beginFollowup(prev, q));
    try {
      const full = await invoke<string>(
        "followup_question",
        followupInvokeArgs(q, ctx.reportText, ctx.scenario),
      );
      // done 事件通常已落定全文；此处兜底（幂等：仅对仍在流式的条目生效）
      setItems((prev) => finishFollowup(prev, full));
    } catch (e) {
      // followup_error 事件已标错误；此处兜底（同样幂等）
      setItems((prev) => failFollowup(prev, String(e)));
    } finally {
      generatingRef.current = false;
      setBusy(false);
    }
  }, []);

  /** 重试指定条目（沿用该条目的问题与最近一次的报告上下文） */
  const retry = useCallback(
    async (index: number) => {
      if (generatingRef.current || !lastContext.current) return;
      const ctx = lastContext.current;
      const target = items[index];
      if (!target || target.streaming || target.error == null) return;
      const question = target.question;
      setItems((prev) => retryFollowupAt(prev, index) ?? prev);
      generatingRef.current = true;
      setBusy(true);
      try {
        const full = await invoke<string>(
          "followup_question",
          followupInvokeArgs(question, ctx.reportText, ctx.scenario),
        );
        setItems((prev) => finishFollowup(prev, full));
      } catch (e) {
        setItems((prev) => failFollowup(prev, String(e)));
      } finally {
        generatingRef.current = false;
        setBusy(false);
      }
    },
    [items],
  );

  return { items, busy, ask, retry };
}
