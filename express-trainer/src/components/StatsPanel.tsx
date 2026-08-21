import type { SessionSnapshot } from "../types";

export function StatsPanel({ snapshot }: { snapshot: SessionSnapshot | null }) {
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
      {snapshot.emotionCounts.length > 0 && (
        <div>
          <div className="mb-1 text-neutral-500">情感分布</div>
          {snapshot.emotionCounts.map(([cat, count]) => (
            <div key={cat} className="flex justify-between">
              <span>{cat}</span>
              <span>{count}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
