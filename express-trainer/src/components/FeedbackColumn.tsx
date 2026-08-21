import { useEffect, useRef } from "react";
import type { FeedbackEvent } from "../types";

const KIND_STYLE: Record<string, string> = {
  fillerWord: "border-red-300 text-red-700",
  wordPrecision: "border-amber-300 text-amber-700",
  repetition: "border-orange-300 text-orange-700",
  conclusionMissing: "border-blue-300 text-blue-700",
  exampleMissing: "border-teal-300 text-teal-700",
  emotion: "border-emerald-300 text-emerald-700",
};

export function FeedbackColumn({ events }: { events: FeedbackEvent[] }) {
  const bottomRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [events.length]);

  return (
    <div className="h-full space-y-2 overflow-y-auto px-4 py-4">
      {events.map((e, i) => (
        <div key={i} className={`rounded-lg border-l-4 bg-white px-3 py-2 shadow-sm ${KIND_STYLE[e.kind] ?? ""}`}>
          <div className="text-sm font-medium">{e.message}</div>
          {e.kind === "wordPrecision" && Array.isArray(e.payload.alternatives) && (
            <div className="mt-1 text-sm text-neutral-600">
              {(e.payload.alternatives as string[]).join(" / ")}
            </div>
          )}
        </div>
      ))}
      <div ref={bottomRef} />
    </div>
  );
}
