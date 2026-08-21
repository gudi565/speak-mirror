import { useEffect, useRef } from "react";
import type { Sentence } from "../types";
import { highlightFillers } from "../highlight";

export function SubtitleColumn({
  sentences,
  partial,
  fillers,
}: {
  sentences: Sentence[];
  partial: string;
  fillers: string[];
}) {
  const bottomRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [sentences.length, partial]);

  return (
    <div className="flex h-full flex-col overflow-y-auto px-6 py-4">
      <div className="space-y-3 text-lg leading-relaxed">
        {sentences.map((s) => (
          <p key={s.id}>
            {highlightFillers(s.text, fillers).map((p, i) =>
              p.isFiller ? (
                <span key={i} className="rounded bg-red-500/20 px-0.5 font-medium text-red-600">
                  {p.text}
                </span>
              ) : (
                <span key={i}>{p.text}</span>
              )
            )}
          </p>
        ))}
        {partial && <p className="text-neutral-400">{partial}</p>}
        <div ref={bottomRef} />
      </div>
    </div>
  );
}
