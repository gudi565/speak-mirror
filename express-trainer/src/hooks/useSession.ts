import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  FeedbackEvent,
  FillerWords,
  Sentence,
  SessionSnapshot,
  VoiceMetrics,
} from "../types";

/** 非 Tauri 环境（单测/纯浏览器）的兜底词表 */
const FALLBACK_FILLERS = ["然后", "就是", "那个", "这个", "呃", "嗯"];

export interface CheckinConfig {
  /** 开关 + 远端可用由调用方合并后传入 */
  enabled: boolean;
  intervalSec: number;
  topic: string;
}

/**
 * 会话状态机：识别事件监听、快照统计、反馈事件（带前端 uid，供忽略交互）。
 * AI 周期快评由前端驱动：running 且开关开启时，每 intervalSec 秒把
 * 「上次快评以来新增的句子 id」交给 Rust check_in 命令。
 */
export function useSession(checkin?: CheckinConfig) {
  const [running, setRunning] = useState(false);
  const [partial, setPartial] = useState("");
  const [sentences, setSentences] = useState<Sentence[]>([]);
  const [events, setEvents] = useState<FeedbackEvent[]>([]);
  const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
  const [voice, setVoice] = useState<VoiceMetrics | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [fillerWords, setFillerWords] = useState<string[]>(FALLBACK_FILLERS);
  const unlisteners = useRef<(() => void)[]>([]);
  const uidRef = useRef(0);
  // 上次快评以来新增的句子 id（快评请求的增量信号）
  const newSentenceIdsRef = useRef<number[]>([]);
  const checkinPendingRef = useRef(false);
  const topicRef = useRef("");

  useEffect(() => {
    let cancelled = false;
    const regs = [
      listen<{ text: string }>("partial_transcript", (e) => setPartial(e.payload.text)),
      listen<Sentence>("sentence_final", (e) => {
        setPartial("");
        setSentences((prev) => [...prev, e.payload]);
        newSentenceIdsRef.current.push(e.payload.id);
      }),
      listen<{ events: FeedbackEvent[]; snapshot: SessionSnapshot }>(
        "analysis_update",
        (e) => {
          // aiCheckin 事件也走同一通道（Rust check_in 命令发出）
          setEvents((prev) => [
            ...prev,
            ...e.payload.events.map((ev) => ({ ...ev, uid: ++uidRef.current })),
          ]);
          setSnapshot(e.payload.snapshot);
        }
      ),
      // 声音层实时指标（Rust 会话线程每 ~2s 推送一次）
      listen<VoiceMetrics>("voice_update", (e) => setVoice(e.payload)),
      // AI 快评连续失败自动停用：右栏插入一条本地提示事件（可在设置重新开启）
      listen<{ reason: string }>("checkin_disabled", () => {
        setEvents((prev) => [
          ...prev,
          {
            kind: "aiCheckin" as const,
            sentenceId: null,
            message: "AI 快评已暂停（连接异常），可在设置重新开启",
            payload: { issueType: "autoDisabled" },
            uid: ++uidRef.current,
          },
        ]);
      }),
      listen<string>("session_error", (e) => {
        setError(e.payload);
        setRunning(false);
      }),
    ];
    Promise.all(regs).then((fns) => {
      if (cancelled) fns.forEach((f) => f());
      else unlisteners.current = fns;
    });
    return () => {
      cancelled = true;
      unlisteners.current.forEach((f) => f());
      unlisteners.current = [];
    };
  }, []);

  // 字幕红色标注词表：词库高频词 + 用户自定义（中频词误报率高，只进右栏统计）
  useEffect(() => {
    invoke<FillerWords>("get_filler_words")
      .then((w) => {
        const merged = [...w.custom, ...w.high].filter(
          (x, i, arr) => arr.indexOf(x) === i,
        );
        if (merged.length > 0) setFillerWords(merged);
      })
      .catch(() => {
        /* 非 Tauri 环境保留兜底词表 */
      });
  }, []);

  // AI 周期快评：低频、增量触发（无新句子不请求）、失败静默不打断练习
  useEffect(() => {
    topicRef.current = checkin?.topic ?? "";
  }, [checkin?.topic]);

  useEffect(() => {
    if (!running || !checkin?.enabled) return;
    const intervalMs = Math.max(15, checkin.intervalSec) * 1000;
    const timer = window.setInterval(async () => {
      if (checkinPendingRef.current || newSentenceIdsRef.current.length === 0) return;
      checkinPendingRef.current = true;
      const ids = newSentenceIdsRef.current;
      newSentenceIdsRef.current = [];
      try {
        await invoke("check_in", {
          topic: topicRef.current.trim() || null,
          recentSentenceIds: ids,
        });
      } catch {
        // 快评失败不影响练习（宁漏报：连续失败最终表现为无提示）
        newSentenceIdsRef.current = [...ids, ...newSentenceIdsRef.current];
      } finally {
        checkinPendingRef.current = false;
      }
    }, intervalMs);
    return () => {
      window.clearInterval(timer);
    };
  }, [running, checkin?.enabled, checkin?.intervalSec]);

  /** 开始前统一清空上一会话的界面状态 */
  const resetSessionState = useCallback(() => {
    setError(null);
    setSentences([]);
    setEvents([]);
    setSnapshot(null);
    setVoice(null);
    setPartial("");
    uidRef.current = 0;
    newSentenceIdsRef.current = [];
  }, []);

  const start = useCallback(
    async (plannedSec?: number | null) => {
      resetSessionState();
      setPending(true);
      try {
        await invoke("start_session", { plannedSec: plannedSec ?? null });
        setRunning(true);
      } catch (e) {
        setError(String(e));
      } finally {
        setPending(false);
      }
    },
    [resetSessionState],
  );

  /** 从本地音频文件练习：后端先完整解码校验，再按选定倍速（1.0/1.5/2.0/
   *  99=极速不限速，默认 1.0）喂入与麦克风相同的管线；时间戳与统计按素材
   *  内容时间，不受倍速影响（极速档跳过 partial 字幕，只出终稿句） */
  const startFromFile = useCallback(
    async (path: string, plannedSec?: number | null, speed?: number) => {
      resetSessionState();
      setPending(true);
      try {
        await invoke("start_session_from_file", {
          path,
          plannedSec: plannedSec ?? null,
          speed: speed ?? 1.0,
        });
        setRunning(true);
      } catch (e) {
        setError(String(e));
      } finally {
        setPending(false);
      }
    },
    [resetSessionState],
  );

  /** 停止收尾：拿最终快照（返回给调用方——模拟面试用它累积逐题时长），
   *  会话已自然结束时同样返回快照 */
  const stop = useCallback(async (): Promise<SessionSnapshot | null> => {
    setPending(true);
    let snap: SessionSnapshot | null = null;
    try {
      snap = await invoke<SessionSnapshot>("stop_session");
      setSnapshot(snap);
      // 会话终值（stop_session 已并入声音层终值）
      if (snap.voice) setVoice(snap.voice);
      setRunning(false);
      setPartial("");
    } catch (e) {
      setError(String(e));
      setRunning(false);
      setPartial("");
    } finally {
      setPending(false);
    }
    return snap;
  }, []);

  return {
    running,
    partial,
    sentences,
    events,
    snapshot,
    voice,
    error,
    pending,
    fillerWords,
    start,
    startFromFile,
    stop,
  };
}
