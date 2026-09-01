import type { SessionSnapshot, VoiceMetrics } from "../types";

interface Props {
  snapshot: SessionSnapshot | null;
  /** 声音层实时指标（voice_update 事件流；结束后由 snapshot.voice 供值） */
  voice: VoiceMetrics | null;
}

function fmtSec(ms: number): string {
  return (ms / 1000).toFixed(1);
}

/** 声音仪表：语速 / 失控停顿 / 音量动态 三格（全部为会话内相对值） */
function VoiceGauge({ snapshot, voice }: Props) {
  const pauses = voice?.runawayPauseCount ?? 0;
  const longest = voice?.longestPauseMs ?? 0;
  const dynamic = voice?.volumeDynamicRangeDb;
  const cell = "rounded-lg border border-neutral-200 px-3 py-2.5";
  const label = "text-xs text-neutral-500";
  const value = "mt-1 text-lg font-semibold tabular-nums";
  return (
    <div>
      <div className="mb-1 text-neutral-500">声音仪表</div>
      <div className="grid grid-cols-3 gap-2">
        <div className={cell}>
          <div className={label}>语速</div>
          <div className={value}>
            {snapshot && snapshot.speechRate > 0 ? snapshot.speechRate : "—"}
            <span className="ml-0.5 text-xs font-normal text-neutral-400">字/分</span>
          </div>
        </div>
        <div className={cell}>
          <div className={label}>失控停顿</div>
          <div className={value}>
            {pauses}
            <span className="ml-0.5 text-xs font-normal text-neutral-400">次</span>
            {longest > 0 && (
              <div className="text-xs font-normal text-neutral-400">
                最长 {fmtSec(longest)}s
              </div>
            )}
          </div>
        </div>
        <div className={cell}>
          <div className={label}>音量动态</div>
          <div className={value}>
            {dynamic != null ? (
              <>
                {dynamic}
                <span className="ml-0.5 text-xs font-normal text-neutral-400">dB</span>
              </>
            ) : voice?.baselineCalibrated ? (
              <span className="text-base font-normal text-neutral-400">积累中</span>
            ) : (
              <span className="text-base font-normal text-neutral-400">校准中</span>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

export function StatsPanel({ snapshot, voice }: Props) {
  if (!snapshot) {
    return <div className="px-4 py-4 text-sm text-neutral-400">开始说话后这里会显示统计</div>;
  }
  const seconds = Math.round(snapshot.durationMs / 1000);
  const mm = String(Math.floor(seconds / 60)).padStart(2, "0");
  const ss = String(seconds % 60).padStart(2, "0");
  return (
    <div className="space-y-4 px-4 py-4 text-sm">
      <div>
        <div className="text-neutral-500">时长</div>
        <div className="text-xl font-semibold">{mm}:{ss}</div>
      </div>
      <VoiceGauge snapshot={snapshot} voice={voice} />
      <div>
        <div className="text-neutral-500">口头禅频率</div>
        <div className="text-xl font-semibold">{snapshot.fillerPerMinute} 次/分钟</div>
      </div>
      <div>
        <div className="mb-1 text-neutral-500">口头禅 Top</div>
        {snapshot.fillerCounts.slice(0, 5).map(([word, count]) => (
          <div key={word} className="flex justify-between">
            <span className="text-red-600">{word}</span>
            <span>{count}</span>
          </div>
        ))}
      </div>
      {snapshot.hedgeTotal > 0 && (
        <div>
          <div className="mb-1 text-neutral-500">立场模糊词</div>
          <div className="flex justify-between">
            <span className="text-violet-600">共出现</span>
            <span>{snapshot.hedgeTotal} 次</span>
          </div>
          {snapshot.hedgeCounts.slice(0, 3).map(([word, count]) => (
            <div key={word} className="flex justify-between text-neutral-500">
              <span>{word}</span>
              <span>{count}</span>
            </div>
          ))}
        </div>
      )}
      {snapshot.emotionCounts.length > 0 && (
        <div>
          <div className="mb-1 text-neutral-500">情感分布</div>
          {snapshot.emotionCounts.map(([cat, stat]) => (
            <div key={cat} className="flex justify-between">
              <span>{cat}</span>
              <span>
                {stat.count} 次 · 强度 {stat.avgIntensity}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
