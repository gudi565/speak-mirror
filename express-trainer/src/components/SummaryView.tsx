import { useEffect, useMemo, useState } from "react";
import { hasRemoteBackend } from "../lib/settings";
import { SCENARIOS, scenarioMeta } from "../lib/scenarios";
import { REPORT_MODE_OPTIONS } from "../lib/report";
import { playGlyph, playbackAvailable } from "../lib/playback";
import { tonePanelState } from "../lib/tone";
import { useSentencePlayer } from "../hooks/useSentencePlayer";
import { ToneFlagsPanel } from "./ToneFlagsPanel";
import type { ReportMode, Scenario, Sentence, SessionSnapshot, Settings, ToneFlag } from "../types";

interface Props {
  sentences: Sentence[];
  snapshot: SessionSnapshot | null;
  settings: Settings | null;
  scenario: Scenario;
  onScenarioChange: (s: Scenario) => void;
  topic: string;
  onTopicChange: (t: string) => void;
  /** 报告模式（默认 quick：用户痛点是慢） */
  reportMode: ReportMode;
  onReportModeChange: (m: ReportMode) => void;
  onGenerate: (transcript: string | null) => void;
  onRestart: () => void;
  /** 报告流式生成中（防双击并发） */
  generating: boolean;
  /** 本次会话录音路径（无录音 = null；会话进行中不提供回放） */
  audioPath: string | null;
  /** 声调偏差标记（null = 尚未收到 tone_update：分析中/未开启/无录音） */
  toneFlags: ToneFlag[] | null;
}

function fmtDuration(ms: number): string {
  const totalSec = Math.round(ms / 1000);
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  return m > 0 ? `${m} 分 ${String(s).padStart(2, "0")} 秒` : `${s} 秒`;
}

export function SummaryView({
  sentences,
  snapshot,
  settings,
  scenario,
  onScenarioChange,
  topic,
  onTopicChange,
  reportMode,
  onReportModeChange,
  onGenerate,
  onRestart,
  generating,
  audioPath,
  toneFlags,
}: Props) {
  const defaultTranscript = useMemo(
    () => sentences.map((s) => s.text).join("\n"),
    [sentences],
  );
  const [transcript, setTranscript] = useState(defaultTranscript);
  const player = useSentencePlayer();

  useEffect(() => {
    setTranscript(defaultTranscript);
  }, [defaultTranscript]);

  const remoteReady = settings ? hasRemoteBackend(settings) : false;

  // 声调提示面板状态：开关关闭隐藏；无录音 / 分析中 / 无发现 / 有标记四态
  const toneState = tonePanelState({
    toneCheck: settings?.toneCheck ?? true,
    audioPath,
    toneFlags,
  });

  return (
    <div className="mx-auto max-w-3xl px-6 py-8">
      <h2 className="text-lg font-semibold">练习结束 · 总结</h2>
      <p className="mt-1 text-sm text-neutral-500">
        可以先修正转写中的错字，再生成报告——修正后的逐字稿会作为报告输入，分析质量会更好。
      </p>

      {/* 快照统计 */}
      {snapshot && snapshot.sentenceCount > 0 && (
        <div className="mt-4 grid grid-cols-2 gap-3 rounded-xl border border-neutral-200 bg-white p-4 text-sm sm:grid-cols-4">
          <div>
            <div className="text-neutral-500">时长</div>
            <div className="font-semibold">{fmtDuration(snapshot.durationMs)}</div>
          </div>
          <div>
            <div className="text-neutral-500">句数</div>
            <div className="font-semibold">{snapshot.sentenceCount}</div>
          </div>
          <div>
            <div className="text-neutral-500">字数</div>
            <div className="font-semibold">{snapshot.totalChars}</div>
          </div>
          <div>
            <div className="text-neutral-500">语速</div>
            <div className="font-semibold">
              {snapshot.speechRate > 0 ? `${snapshot.speechRate} 字/分钟` : "—"}
            </div>
          </div>
        </div>
      )}

      {/* 逐句回放（有录音时；仅会话结束后提供） */}
      {playbackAvailable(audioPath, sentences.length) && (
        <div className="mt-5">
          <div className="mb-1 flex items-baseline justify-between">
            <label className="text-sm font-medium text-neutral-700">逐句回放</label>
            <span className="text-xs text-neutral-400">录音仅保存在本机</span>
          </div>
          <div className="max-h-56 space-y-1 overflow-y-auto rounded-xl border border-neutral-200 bg-white px-3 py-2">
            {sentences.map((s) => (
              <div key={s.id} className="flex items-start gap-2 text-sm leading-6">
                <button
                  onClick={() => audioPath && player.toggle(audioPath, s.id, s.startMs, s.endMs)}
                  title={player.playingId === s.id ? "停止" : "播放这句"}
                  className="mt-0.5 shrink-0 rounded border border-neutral-300 px-1.5 py-0.5 text-xs text-neutral-600 hover:bg-neutral-100"
                >
                  {playGlyph({ playingId: player.playingId }, s.id)}
                </button>
                <span className="min-w-0 break-words">{s.text}</span>
              </div>
            ))}
          </div>
          {player.error && <p className="mt-1 text-xs text-red-600">{player.error}</p>}
        </div>
      )}

      {/* 声调提示（练习结束后的本机分析；无录音/分析中/无发现三种空态） */}
      {toneState !== "hidden" && (
        <div className="mt-5">
          <div className="mb-1 flex items-baseline justify-between">
            <label className="text-sm font-medium text-neutral-700">声调提示</label>
            <span className="text-xs text-neutral-400">词典对照的启发判断，仅供参考</span>
          </div>
          {toneState === "noAudio" && (
            <p className="rounded-xl border border-dashed border-neutral-200 bg-white px-3 py-3 text-xs text-neutral-400">
              本次练习没有会话录音，无法做声调分析。可在「设置」中开启「会话录音」后再试。
            </p>
          )}
          {toneState === "analyzing" && (
            <p className="rounded-xl border border-dashed border-neutral-200 bg-white px-3 py-3 text-xs text-neutral-400">
              声调分析中…（正在本机分析录音，稍候片刻）
            </p>
          )}
          {toneState === "clean" && (
            <p className="rounded-xl border border-dashed border-neutral-200 bg-white px-3 py-3 text-xs text-neutral-400">
              未发现明显的声调偏差。
            </p>
          )}
          {toneState === "flags" && (
            <>
              <ToneFlagsPanel
                flags={toneFlags ?? []}
                sentences={sentences}
                audioPath={audioPath}
                playingId={player.playingId}
                onToggle={(path, id, startMs, endMs) => {
                  void player.toggle(path, id, startMs, endMs);
                }}
              />
              {player.error && <p className="mt-1 text-xs text-red-600">{player.error}</p>}
            </>
          )}
        </div>
      )}

      {/* 逐字稿编辑 */}
      <div className="mt-5">
        <div className="mb-1 flex items-baseline justify-between">
          <label className="text-sm font-medium text-neutral-700">逐字稿（一行一句，可编辑）</label>
          <button
            onClick={() => setTranscript(defaultTranscript)}
            className="text-xs text-neutral-400 hover:text-neutral-600"
          >
            还原
          </button>
        </div>
        <textarea
          value={transcript}
          onChange={(e) => setTranscript(e.target.value)}
          rows={Math.min(Math.max(sentences.length + 2, 6), 20)}
          placeholder="没有识别到句子。可以直接在这里输入文字生成报告。"
          className="w-full resize-y rounded-xl border border-neutral-300 bg-white px-4 py-3 text-sm leading-6"
        />
      </div>

      {/* 场景 + 主题 */}
      <div className="mt-5 grid grid-cols-1 gap-4 sm:grid-cols-2">
        <div>
          <label className="mb-1 block text-sm font-medium text-neutral-700">场景</label>
          <select
            value={scenario}
            onChange={(e) => onScenarioChange(e.target.value as Scenario)}
            className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
          >
            {SCENARIOS.map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}
              </option>
            ))}
          </select>
          <p className="mt-1 text-xs text-neutral-400">{scenarioMeta(scenario).hint}</p>
        </div>
        <div>
          <label className="mb-1 block text-sm font-medium text-neutral-700">
            {scenarioMeta(scenario).topicLabel}
          </label>
          <input
            value={topic}
            onChange={(e) => onTopicChange(e.target.value)}
            placeholder={scenarioMeta(scenario).topicPlaceholder}
            className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
          />
        </div>
      </div>

      {/* 报告模式（默认快速——用户痛点是慢） */}
      <div className="mt-5">
        <div className="mb-1 text-sm font-medium text-neutral-700">报告模式</div>
        <div className="flex flex-wrap gap-3">
          {REPORT_MODE_OPTIONS.map((opt) => (
            <label
              key={opt.value}
              className={`flex cursor-pointer items-start gap-2 rounded-xl border px-4 py-2.5 text-sm ${
                reportMode === opt.value
                  ? "border-neutral-900 bg-neutral-900/[0.04]"
                  : "border-neutral-300 bg-white hover:border-neutral-400"
              }`}
            >
              <input
                type="radio"
                name="report-mode"
                checked={reportMode === opt.value}
                onChange={() => onReportModeChange(opt.value)}
                disabled={generating}
                className="mt-0.5 accent-neutral-900"
              />
              <span>
                <span className="font-medium text-neutral-800">{opt.label}</span>
                <span className="block text-xs text-neutral-400">{opt.hint}</span>
              </span>
            </label>
          ))}
        </div>
      </div>

      {/* 操作 */}
      <div className="mt-6 flex items-center gap-3">
        <button
          onClick={() => onGenerate(transcript.trim() ? transcript : null)}
          disabled={generating}
          className="rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
        >
          {generating ? "生成中…" : remoteReady ? "生成报告" : "生成本地报告"}
        </button>
        <button
          onClick={onRestart}
          className="rounded-lg border border-neutral-300 px-4 py-2.5 text-sm text-neutral-700 hover:bg-neutral-100"
        >
          再练一次
        </button>
        {!remoteReady && (
          <span className="text-xs text-neutral-400">
            未配置 API Key，将生成本地降级报告（可在右上角设置中配置）
          </span>
        )}
      </div>
    </div>
  );
}
