import { useCallback, useEffect, useRef, useState } from "react";
import { readFile } from "@tauri-apps/plugin-fs";
import { playReducer, sentenceSlice, type PlayState } from "../lib/playback";

/**
 * 逐句回放（仅会话结束后使用）：tauri fs 读录音 wav 字节 → Web Audio
 * decodeAudioData → 播放句子切片。同一视图内按文件路径缓存解码结果；
 * 再点同一句 = 停止；切句 = 直接换（旧 source stop 掉）。
 */
export function useSentencePlayer() {
  const [state, setState] = useState<PlayState>({ playingId: null });
  const [error, setError] = useState<string | null>(null);
  const stateRef = useRef(state);
  stateRef.current = state;
  const ctxRef = useRef<AudioContext | null>(null);
  const buffersRef = useRef(new Map<string, AudioBuffer>());
  const sourceRef = useRef<AudioBufferSourceNode | null>(null);

  const stopInternal = useCallback(() => {
    const src = sourceRef.current;
    sourceRef.current = null;
    if (src) {
      src.onended = null; // 手动停止不触发 onended 分支
      try {
        src.stop();
      } catch {
        /* 已结束的 source 再 stop 会抛错，忽略 */
      }
    }
  }, []);

  /** 切换某句的播放/停止 */
  const toggle = useCallback(
    async (path: string, id: number, startMs: number, endMs: number) => {
      setError(null);
      // 正在播这句 → 停止
      if (stateRef.current.playingId === id) {
        stopInternal();
        setState(playReducer(stateRef.current, { type: "stop" }));
        return;
      }
      // 切到别的句（或从静止开始播）
      stopInternal();
      setState({ playingId: id });
      try {
        const ctx = (ctxRef.current ??= new AudioContext());
        if (ctx.state === "suspended") {
          await ctx.resume(); // 浏览器自动播放策略：点击即手势，恢复即可
        }
        let buffer = buffersRef.current.get(path);
        if (!buffer) {
          const bytes = await readFile(path);
          const raw = bytes.buffer.slice(
            bytes.byteOffset,
            bytes.byteOffset + bytes.byteLength,
          ) as ArrayBuffer;
          buffer = await ctx.decodeAudioData(raw);
          buffersRef.current.set(path, buffer);
        }
        // await 期间用户可能已点了别的句 / 停止：以最新状态为准
        if (stateRef.current.playingId !== id) return;
        const { offsetSec, durationSec } = sentenceSlice(startMs, endMs, buffer.duration);
        const src = ctx.createBufferSource();
        src.buffer = buffer;
        src.onended = () => {
          if (stateRef.current.playingId === id) {
            setState(playReducer(stateRef.current, { type: "ended", id }));
          }
        };
        sourceRef.current = src;
        src.start(0, offsetSec, durationSec);
      } catch (e) {
        if (stateRef.current.playingId === id) {
          setState({ playingId: null });
        }
        setError(`回放失败：${String(e)}`);
      }
    },
    [stopInternal],
  );

  // 组件卸载时停掉在播的 source
  useEffect(() => stopInternal, [stopInternal]);

  return { playingId: state.playingId, error, toggle };
}
