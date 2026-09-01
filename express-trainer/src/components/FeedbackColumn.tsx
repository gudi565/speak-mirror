import { useEffect, useRef } from "react";
import type { FeedbackEvent } from "../types";

const KIND_STYLE: Record<string, string> = {
  fillerWord: "border-red-300 text-red-700",
  wordPrecision: "border-amber-300 text-amber-700",
  repetition: "border-orange-300 text-orange-700",
  conclusionMissing: "border-blue-300 text-blue-700",
  exampleMissing: "border-teal-300 text-teal-700",
  emotion: "border-emerald-300 text-emerald-700",
  hedge: "border-violet-300 text-violet-700",
  timeVague: "border-sky-300 text-sky-700",
  imagery: "border-lime-300 text-lime-700",
  // 金句为正向提示：绿色系（加粗边框以示强调，与情感词的 emerald、画面感的 lime 区分）
  goldenQuote: "border-green-400 text-green-700",
  aiCheckin: "border-fuchsia-300 text-fuchsia-700",
};

/** 中频口头禅降为琥珀色（高频保持红色） */
function fillerToneClass(e: FeedbackEvent): string | undefined {
  if (e.kind !== "fillerWord") return undefined;
  return e.payload.tier === "medium"
    ? "border-amber-300 text-amber-700"
    : "border-red-300 text-red-700";
}

function payloadDetail(e: FeedbackEvent): string | null {
  if (e.kind === "wordPrecision" && Array.isArray(e.payload.alternatives)) {
    const alts = (e.payload.alternatives as string[]).slice(0, 4).join(" / ");
    return alts || null;
  }
  if (e.kind === "timeVague" && typeof e.payload.suggestion === "string") {
    return e.payload.suggestion;
  }
  if (e.kind === "imagery" && Array.isArray(e.payload.suggestions)) {
    const alts = (e.payload.suggestions as string[]).slice(0, 3).join(" / ");
    return alts || null;
  }
  if (e.kind === "goldenQuote" && typeof e.payload.quote === "string") {
    return e.payload.quote;
  }
  return null;
}

export function FeedbackColumn({
  events,
  onDismiss,
}: {
  events: FeedbackEvent[];
  /** 点 ✕ 忽略该条（本会话内不再显示，仅影响 UI 不影响统计） */
  onDismiss?: (uid: number) => void;
}) {
  const bottomRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [events.length]);

  return (
    <div className="h-full space-y-2 overflow-y-auto px-4 py-4">
      {events.map((e, i) => {
        const detail = payloadDetail(e);
        return (
          <div
            key={e.uid ?? i}
            className={`group relative rounded-lg border-l-4 bg-white px-3 py-2 shadow-sm ${
              fillerToneClass(e) ?? KIND_STYLE[e.kind] ?? ""
            }`}
          >
            {onDismiss && e.uid !== undefined && (
              <button
                onClick={() => onDismiss(e.uid as number)}
                title="忽略（本会话内不再显示，不影响统计）"
                className="absolute right-1.5 top-1.5 rounded px-1 text-xs text-neutral-300 opacity-0 transition-opacity hover:text-neutral-500 group-hover:opacity-100"
              >
                ✕
              </button>
            )}
            <div className="text-sm font-medium">{e.message}</div>
            {detail && <div className="mt-1 text-sm text-neutral-600">{detail}</div>}
          </div>
        );
      })}
      <div ref={bottomRef} />
    </div>
  );
}
