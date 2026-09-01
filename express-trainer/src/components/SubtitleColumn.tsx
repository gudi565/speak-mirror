import { useEffect, useRef } from "react";
import type { Sentence } from "../types";
import { highlightFillers } from "../highlight";
import { shouldShowPartial } from "../lib/subtitle";

/**
 * 中栏字幕：定稿句为主视觉（正常字号、含标红高亮），流式 partial 为辅——
 * 仅在定稿区下方以一行灰色斜体小字显示「识别中…」（与最近定稿句重复、
 * 或设置关闭时不显示，判定见 lib/subtitle.ts 的 shouldShowPartial）
 */
export function SubtitleColumn({
  sentences,
  partial,
  fillers,
  showLivePreview,
}: {
  sentences: Sentence[];
  partial: string;
  fillers: string[];
  /** 显示实时识别预览（设置项，默认开；关闭后仅显示每句定稿） */
  showLivePreview: boolean;
}) {
  const bottomRef = useRef<HTMLDivElement>(null);
  // 定稿为主：仅在新定稿句出现时滚动到最新定稿（partial 行不触发，避免频繁滚动）
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [sentences.length]);

  const lastFinal = sentences.length > 0 ? sentences[sentences.length - 1].text : null;
  const previewVisible = shouldShowPartial(partial, lastFinal, showLivePreview);

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
        {previewVisible && (
          <p className="text-sm italic leading-relaxed text-neutral-400">
            识别中… {partial.trim()}
          </p>
        )}
        <div ref={bottomRef} />
      </div>
    </div>
  );
}
