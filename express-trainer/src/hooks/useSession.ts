import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { FeedbackEvent, Sentence, SessionSnapshot } from "../types";

const DEFAULT_FILLERS = ["然后", "就是", "那个", "呃", "嗯", "其实", "比如说"];

export function useSession() {
  const [running, setRunning] = useState(false);
  const [partial, setPartial] = useState("");
  const [sentences, setSentences] = useState<Sentence[]>([]);
  const [events, setEvents] = useState<FeedbackEvent[]>([]);
  const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const unlisteners = useRef<(() => void)[]>([]);

  useEffect(() => {
    let cancelled = false;
    const regs = [
      listen<{ text: string }>("partial_transcript", (e) => setPartial(e.payload.text)),
      listen<Sentence>("sentence_final", (e) => {
        setPartial("");
        setSentences((prev) => [...prev, e.payload]);
      }),
      listen<{ events: FeedbackEvent[]; snapshot: SessionSnapshot }>(
        "analysis_update",
        (e) => {
          setEvents((prev) => [...prev, ...e.payload.events]);
          setSnapshot(e.payload.snapshot);
        }
      ),
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

  const start = useCallback(async () => {
    setError(null);
    setSentences([]);
    setEvents([]);
    setSnapshot(null);
    setPartial("");
    setPending(true);
    try {
      await invoke("start_session");
      setRunning(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setPending(false);
    }
  }, []);

  const stop = useCallback(async () => {
    setPending(true);
    try {
      const snap = await invoke<SessionSnapshot>("stop_session");
      setSnapshot(snap);
      setRunning(false);
      setPartial("");
    } catch (e) {
      setError(String(e));
      setRunning(false);
      setPartial("");
    } finally {
      setPending(false);
    }
  }, []);

  return { running, partial, sentences, events, snapshot, error, pending, fillerWords: DEFAULT_FILLERS, start, stop };
}
