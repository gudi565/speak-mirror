/**
 * 逐句回放（纯函数）：录音 wav → Web Audio 切片播放的换算与播放状态机。
 * 组件侧（useSentencePlayer）只做 IO 与 AudioContext 编排，逻辑全部在这里可单测。
 */

/** 句尾余量（秒）：终稿 end_ms 是转写完成点，补一点尾音听起来才完整 */
export const SLICE_TAIL_PAD_SEC = 0.2;

/** 最短播放时长（秒）：防止 end≈start 时出现 0 时长 start() 报错 */
export const MIN_SLICE_DURATION_SEC = 0.05;

export interface PlaySlice {
  /** 从录音的哪一秒开始播 */
  offsetSec: number;
  /** 播多少秒 */
  durationSec: number;
}

/**
 * 句子 [startMs, endMs] → 录音内切片（秒）。
 * - 负 start 归零（会话开头的第一句）
 * - 句尾补 SLICE_TAIL_PAD_SEC 余量
 * - totalSec（解码出的录音时长）存在时夹到文件末尾
 * - 至少 MIN_SLICE_DURATION_SEC
 */
export function sentenceSlice(
  startMs: number,
  endMs: number,
  totalSec?: number,
): PlaySlice {
  const startSec = Math.max(0, startMs / 1000);
  let endSec = Math.max(startSec, endMs / 1000) + SLICE_TAIL_PAD_SEC;
  if (totalSec != null && Number.isFinite(totalSec) && totalSec > 0) {
    endSec = Math.min(endSec, totalSec);
  }
  const durationSec = Math.max(MIN_SLICE_DURATION_SEC, endSec - startSec);
  return { offsetSec: startSec, durationSec };
}

// ---------------------------------------------------------------------------
// 播放状态机（reducer 形式）：同一时刻至多一句在播；点同一句 = 停止
// ---------------------------------------------------------------------------

export interface PlayState {
  playingId: number | null;
}

export type PlayAction =
  | { type: "start"; id: number }
  | { type: "stop" }
  /** 自然播完（AudioBufferSourceNode.onended）：只清理仍是当前句的播放 */
  | { type: "ended"; id: number };

export function playReducer(state: PlayState, action: PlayAction): PlayState {
  switch (action.type) {
    case "start":
      return { playingId: action.id };
    case "stop":
      return { playingId: null };
    case "ended":
      return state.playingId === action.id ? { playingId: null } : state;
  }
}

/** 按钮图标：正在播的句子显示 ⏸（再点停止），其余 ▶ */
export function playGlyph(state: PlayState, id: number): "▶" | "⏸" {
  return state.playingId === id ? "⏸" : "▶";
}

/** 是否可回放：需要有录音路径且有带时间戳的句子 */
export function playbackAvailable(
  audioPath: string | null | undefined,
  sentenceCount: number,
): boolean {
  return !!audioPath && sentenceCount > 0;
}
