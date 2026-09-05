import { playGlyph } from "../lib/playback";
import { toneFlagText, toneNoteText } from "../lib/tone";
import type { Sentence, ToneFlag } from "../types";

interface Props {
  flags: ToneFlag[];
  sentences: Sentence[];
  /** 会话录音 wav 路径（历史详情 = audioFile；总结页 = lastAudio） */
  audioPath: string | null;
  /** 当前正在回放的句子 id（useSentencePlayer.playingId） */
  playingId: number | null;
  /** 逐句回放切换（useSentencePlayer.toggle） */
  onToggle: (path: string, id: number, startMs: number, endMs: number) => void;
}

/**
 * 声调标记列表（总结页与历史详情共用）：每条「第 N 句「X」应为 Y 声（听感偏
 * Z）」+ 规则说明小字（v1 note，变调/音域语境的真偏差才有）+ 该句的回放按钮
 * ——看提示点播放对照，形成闭环。空态由调用方渲染。
 */
export function ToneFlagsPanel({ flags, sentences, audioPath, playingId, onToggle }: Props) {
  return (
    <div className="max-h-56 space-y-1 overflow-y-auto rounded-xl border border-neutral-200 bg-white px-3 py-2">
      {flags.map((f, i) => {
        const sentence = sentences.find((s) => s.id === f.sentenceId);
        const note = toneNoteText(f);
        return (
          <div key={`${f.sentenceId}-${f.charIndex}-${i}`} className="flex items-start gap-2 text-sm leading-6">
            {sentence && audioPath ? (
              <button
                onClick={() => onToggle(audioPath, sentence.id, sentence.startMs, sentence.endMs)}
                title={playingId === sentence.id ? "停止" : "播放这句对照"}
                className="mt-0.5 shrink-0 rounded border border-neutral-300 px-1.5 py-0.5 text-xs text-neutral-600 hover:bg-neutral-100"
              >
                {playGlyph({ playingId }, sentence.id)}
              </button>
            ) : (
              <span className="mt-0.5 w-[30px] shrink-0 text-center text-xs text-neutral-300">—</span>
            )}
            <span className="min-w-0 break-words">
              {toneFlagText(f)}
              {/* v1：变调/音域规则语境的说明（小字灰色）；普通偏差与旧记录无附注 */}
              {note && <span className="block text-xs leading-5 text-neutral-400">{note}</span>}
            </span>
          </div>
        );
      })}
    </div>
  );
}
