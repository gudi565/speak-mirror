import { useCallback, useEffect, useMemo, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { fetchSessionDetail, fetchSessionSummaries, fmtDuration, goalStatus, goalY, overallScore, scoreSeries, scoreTone, removeSession, sourceBadge, sourceLabel, sparklinePoints } from "../lib/history";
import { scenarioLabel } from "../lib/scenarios";
import { playGlyph, playbackAvailable } from "../lib/playback";
import { reportModeLabel } from "../lib/report";
import { useSentencePlayer } from "../hooks/useSentencePlayer";
import { ToneFlagsPanel } from "./ToneFlagsPanel";
import type { SessionRecord, SessionSummary, Settings } from "../types";

interface Props {
  settings: Settings | null;
  onBack: () => void;
}

const CHART_W = 260;
const CHART_H = 72;
/** 手绘 SVG 折线的颜色（dataviz 纪律：线用弱化灰、当前点用验证过的强调蓝、文字用文本色） */
const LINE = "#a3a3a3";
const ACCENT = "#2a78d6";
const INK = "#171717";
const GOAL = "#525252";

function shortDate(date: string): string {
  // "2026-08-30 10:00:00" → "08-30"
  return date.length >= 10 ? date.slice(5, 10) : date;
}

interface TrendChartProps {
  title: string;
  unit: string;
  values: number[];
  dates: string[];
  goal?: number | null;
  goalLabel?: string;
  /** 数值保留小数位 */
  digits?: number;
}

function TrendChart({ title, unit, values, dates, goal, goalLabel, digits = 1 }: TrendChartProps) {
  const pts = sparklinePoints(values, CHART_W, CHART_H);
  const latest = values[values.length - 1] ?? null;
  const path = pts.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(" ");
  const gy = goal != null ? goalY(values, goal, CHART_H) : null;
  const fmt = (v: number) => (digits === 0 ? String(Math.round(v)) : v.toFixed(digits));
  return (
    <div className="rounded-xl border border-neutral-200 bg-white p-3">
      <div className="mb-1 flex items-baseline justify-between">
        <span className="text-xs text-neutral-500">{title}</span>
        {latest != null && (
          <span className="text-sm font-semibold tabular-nums" style={{ color: INK }}>
            {fmt(latest)}
            <span className="ml-0.5 text-xs font-normal text-neutral-400">{unit}</span>
          </span>
        )}
      </div>
      <svg
        viewBox={`0 0 ${CHART_W} ${CHART_H}`}
        className="h-[72px] w-full"
        role="img"
        aria-label={`${title}趋势，共 ${values.length} 次练习`}
      >
        {/* 底部弱化基线（recessive grid） */}
        <line x1={0} x2={CHART_W} y1={CHART_H - 1} y2={CHART_H - 1} stroke="#e5e5e5" strokeWidth={1} />
        {gy != null && (
          <>
            <line
              x1={0}
              x2={CHART_W}
              y1={gy}
              y2={gy}
              stroke={GOAL}
              strokeWidth={1}
              strokeDasharray="4 3"
            />
            <text x={CHART_W - 2} y={gy - 4} textAnchor="end" fontSize={10} fill={GOAL}>
              {goalLabel ?? "目标"}
            </text>
          </>
        )}
        {pts.length > 1 && (
          <polyline points={path} fill="none" stroke={LINE} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
        )}
        {pts.map((p, i) => (
          <g key={i}>
            <circle cx={p.x} cy={p.y} r={8} fill="transparent">
              <title>{`${dates[i]}：${fmt(values[i])} ${unit}`}</title>
            </circle>
            {i === pts.length - 1 ? (
              <circle cx={p.x} cy={p.y} r={4} fill={ACCENT} stroke="#ffffff" strokeWidth={2} />
            ) : (
              <circle cx={p.x} cy={p.y} r={2.5} fill={LINE} />
            )}
          </g>
        ))}
      </svg>
      {dates.length > 1 && (
        <div className="flex justify-between text-[10px] text-neutral-400">
          <span>{shortDate(dates[0])}</span>
          <span>{shortDate(dates[dates.length - 1])}</span>
        </div>
      )}
    </div>
  );
}

function GoalBanner({ summaries, settings }: { summaries: SessionSummary[]; settings: Settings | null }) {
  const goal = settings?.fillerGoalPerMin ?? null;
  const latest = summaries[0];
  if (goal == null || !latest) return null;
  const status = goalStatus(latest.fillerPerMinute, goal);
  return (
    <div className="flex items-center justify-between rounded-xl border border-neutral-200 bg-white px-4 py-3 text-sm">
      <span className="text-neutral-500">
        口头禅目标 <span className="font-semibold text-neutral-900">{goal} 次/分钟</span>
        （可在设置中调整）
      </span>
      {status.met ? (
        <span className="font-medium text-green-700">
          已达标：最近一次 {latest.fillerPerMinute}，低于目标 {-status.diff}
        </span>
      ) : (
        <span className="font-medium text-amber-700">
          还差 {status.diff} 次/分钟：最近一次 {latest.fillerPerMinute}
        </span>
      )}
    </div>
  );
}

export function HistoryView({ settings, onBack }: Props) {
  const [summaries, setSummaries] = useState<SessionSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionRecord | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const player = useSentencePlayer();

  const refresh = useCallback(async () => {
    try {
      setSummaries(await fetchSessionSummaries());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const openDetail = async (id: string) => {
    try {
      setDetail(await fetchSessionDetail(id));
      setDetailError(null);
    } catch (e) {
      setDetailError(String(e));
    }
  };

  const handleDelete = async () => {
    if (!detail || deleting) return;
    if (!window.confirm("删除这条历史记录？对应的练习与报告将一并删除。")) return;
    setDeleting(true);
    try {
      await removeSession(detail.id);
      setDetail(null);
      await refresh();
    } catch (e) {
      setDetailError(String(e));
    } finally {
      setDeleting(false);
    }
  };

  // 趋势曲线按时间正序（旧 → 新）
  const chrono = useMemo(() => [...(summaries ?? [])].reverse(), [summaries]);
  const dates = chrono.map((s) => s.date);
  // 报告总分趋势（对无评分记录跳点：本地降级报告与旧记录无 scores）
  const scoreTrend = useMemo(() => scoreSeries(chrono), [chrono]);

  // 详情：重渲染该次报告 Markdown
  if (detail) {
    return (
      <div className="mx-auto max-w-3xl px-6 py-6">
        <div className="flex items-center justify-between">
          <div>
            <h2 className="text-lg font-semibold">
              {sourceBadge(detail.source)}{" "}
              {scenarioLabel(detail.scenario)} · {detail.date}
            </h2>
            {detail.topic && <p className="mt-0.5 text-sm text-neutral-500">{detail.topic}</p>}
            {detail.source === "file" && detail.fileName && (
              <p className="mt-0.5 text-xs text-neutral-400">来源文件：{detail.fileName}</p>
            )}
          </div>
          <div className="flex items-center gap-2 text-sm">
            <button
              onClick={() => setDetail(null)}
              className="rounded-lg border border-neutral-300 px-3 py-2 text-neutral-700 hover:bg-neutral-100"
            >
              返回列表
            </button>
            <button
              onClick={handleDelete}
              disabled={deleting}
              className="rounded-lg border border-red-200 px-3 py-2 text-red-600 hover:bg-red-50 disabled:opacity-50"
            >
              删除
            </button>
          </div>
        </div>
        {detailError && <p className="mt-2 text-sm text-red-600">{detailError}</p>}
        <div className="mt-3 grid grid-cols-2 gap-3 rounded-xl border border-neutral-200 bg-white p-4 text-sm sm:grid-cols-4">
          <div>
            <div className="text-neutral-500">时长</div>
            <div className="font-semibold">{fmtDuration(detail.snapshot.durationMs)}</div>
          </div>
          <div>
            <div className="text-neutral-500">口头禅</div>
            <div className="font-semibold">{detail.snapshot.fillerPerMinute} 次/分钟</div>
          </div>
          <div>
            <div className="text-neutral-500">语速</div>
            <div className="font-semibold">
              {detail.snapshot.speechRate > 0 ? `${detail.snapshot.speechRate} 字/分钟` : "—"}
            </div>
          </div>
          <div>
            <div className="text-neutral-500">失控停顿</div>
            <div className="font-semibold">
              {detail.snapshot.voice
                ? `${detail.snapshot.voice.runawayPauseCount} 次`
                : "—"}
            </div>
          </div>
        </div>

        {/* 报告评分（有 SCORE 标记的记录）：总分徽章 + 各维度分 */}
        {detail.scores && overallScore(detail) != null && (
          <div className="mt-3 rounded-xl border border-neutral-200 bg-white p-4">
            <div className="flex items-center gap-2">
              <span className="text-sm text-neutral-500">报告总分</span>
              <span
                className={`inline-block rounded-full px-2.5 py-0.5 text-sm font-semibold tabular-nums ${scoreTone(overallScore(detail)!)}`}
              >
                {overallScore(detail)}
              </span>
              <span className="text-xs text-neutral-400">/ 100</span>
            </div>
            {Object.entries(detail.scores)
              .filter(([k]) => k !== "overall")
              .length > 0 && (
              <div className="mt-2 flex flex-wrap gap-1.5">
                {Object.entries(detail.scores)
                  .filter(([k]) => k !== "overall")
                  .map(([key, value]) => (
                    <span
                      key={key}
                      className="rounded border border-neutral-200 bg-neutral-50 px-2 py-0.5 text-xs text-neutral-600"
                      title={`${key}：${value} / 100`}
                    >
                      {key} <span className="font-semibold tabular-nums">{value}</span>
                    </span>
                  ))}
              </div>
            )}
          </div>
        )}

        {/* 声调提示（该次会话的离线分析结果有标记时展示） */}
        {detail.snapshot.toneFlags && detail.snapshot.toneFlags.length > 0 && (
          <div className="mt-4">
            <div className="mb-1 flex items-baseline justify-between">
              <span className="text-sm font-medium text-neutral-700">声调提示</span>
              <span className="text-xs text-neutral-400">词典对照的启发判断，仅供参考</span>
            </div>
            <ToneFlagsPanel
              flags={detail.snapshot.toneFlags}
              sentences={detail.transcript}
              audioPath={detail.audioFile ?? null}
              playingId={player.playingId}
              onToggle={(path, id, startMs, endMs) => {
                void player.toggle(path, id, startMs, endMs);
              }}
            />
            {player.error && <p className="mt-1 text-xs text-red-600">{player.error}</p>}
          </div>
        )}

        {/* 逐句回放（该次会话有录音时） */}
        {playbackAvailable(detail.audioFile ?? null, detail.transcript.length) && (
          <div className="mt-4">
            <div className="mb-1 flex items-baseline justify-between">
              <span className="text-sm font-medium text-neutral-700">逐句回放</span>
              <span className="text-xs text-neutral-400">录音仅保存在本机</span>
            </div>
            <div className="max-h-64 space-y-1 overflow-y-auto rounded-xl border border-neutral-200 bg-white px-3 py-2">
              {detail.transcript.map((s) => (
                <div key={s.id} className="flex items-start gap-2 text-sm leading-6">
                  <button
                    onClick={() =>
                      detail.audioFile && player.toggle(detail.audioFile, s.id, s.startMs, s.endMs)
                    }
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

        <article className="report-md mt-4 rounded-xl border border-neutral-200 bg-white px-6 py-5 text-sm leading-7">
          <Markdown remarkPlugins={[remarkGfm]}>{detail.report}</Markdown>
        </article>
        <p className="mt-2 text-xs text-neutral-400">
          报告模式：{reportModeLabel(detail.reportMode)} · 后端：
          {detail.aiBackendUsed === "local" ? "本地降级" : detail.aiBackendUsed}
        </p>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-3xl px-6 py-6">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">成长档案</h2>
        <button
          onClick={onBack}
          className="rounded-lg border border-neutral-300 px-4 py-2 text-sm text-neutral-700 hover:bg-neutral-100"
        >
          返回练习
        </button>
      </div>

      {error && (
        <p className="mt-3 rounded-lg border border-red-200 bg-red-50 px-4 py-2 text-sm text-red-700">
          历史记录读取失败：{error}
        </p>
      )}

      {summaries != null && summaries.length === 0 ? (
        <div className="mt-10 rounded-xl border border-dashed border-neutral-300 bg-white px-6 py-12 text-center">
          <p className="text-sm font-medium text-neutral-700">还没有历史记录</p>
          <p className="mt-2 text-sm text-neutral-500">
            完成一次练习并生成报告后，这里会自动保存你的会话，
            并给出口头禅频率、语速、失控停顿三条趋势曲线与目标追踪。
          </p>
          <button
            onClick={onBack}
            className="mt-5 rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700"
          >
            去练一次
          </button>
        </div>
      ) : (
        summaries != null && (
          <div className="mt-4 space-y-4">
            <GoalBanner summaries={summaries} settings={settings} />

            {summaries.length >= 2 ? (
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
                <TrendChart
                  title="口头禅 / 分钟"
                  unit="次"
                  values={chrono.map((s) => s.fillerPerMinute)}
                  dates={dates}
                  goal={settings?.fillerGoalPerMin ?? null}
                />
                <TrendChart
                  title="语速"
                  unit="字/分"
                  values={chrono.map((s) => s.speechRate)}
                  dates={dates}
                  digits={0}
                />
                <TrendChart
                  title="失控停顿"
                  unit="次"
                  values={chrono.map((s) => s.runawayPauseCount)}
                  dates={dates}
                  digits={0}
                />
                {/* 报告总分：只画有评分的记录（无评分跳点；全部无评分时隐藏） */}
                {scoreTrend.values.length > 0 && (
                  <TrendChart
                    title="报告总分"
                    unit="分"
                    values={scoreTrend.values}
                    dates={scoreTrend.dates}
                    digits={0}
                  />
                )}
              </div>
            ) : (
              <p className="rounded-xl border border-dashed border-neutral-300 bg-white px-4 py-3 text-sm text-neutral-500">
                已有 1 次记录：再完成一次练习，趋势曲线就会出现在这里。
              </p>
            )}

            <div className="overflow-hidden rounded-xl border border-neutral-200 bg-white">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-neutral-200 text-left text-xs text-neutral-400">
                    <th className="px-4 py-2.5 font-medium">日期</th>
                    <th className="px-4 py-2.5 font-medium">场景</th>
                    <th className="px-4 py-2.5 font-medium">总分</th>
                    <th className="px-4 py-2.5 font-medium">时长</th>
                    <th className="px-4 py-2.5 font-medium">口头禅/分钟</th>
                    <th className="px-4 py-2.5 font-medium">语速</th>
                    <th className="px-4 py-2.5 font-medium">失控停顿</th>
                  </tr>
                </thead>
                <tbody>
                  {summaries.map((s) => {
                    const overall = overallScore(s);
                    return (
                      <tr
                        key={s.id}
                        onClick={() => openDetail(s.id)}
                        className="cursor-pointer border-b border-neutral-100 last:border-0 hover:bg-neutral-50"
                      >
                        <td className="px-4 py-2.5 text-neutral-500">{s.date}</td>
                        <td className="px-4 py-2.5">
                          <span title={sourceLabel(s.source)}>{sourceBadge(s.source)}</span>{" "}
                          {scenarioLabel(s.scenario)}
                          {s.topic && (
                            <span className="ml-1 text-neutral-400">· {s.topic}</span>
                          )}
                          {s.fileName && (
                            <span className="ml-1 max-w-40 truncate align-middle text-neutral-400" title={s.fileName}>
                              · {s.fileName}
                            </span>
                          )}
                        </td>
                        <td className="px-4 py-2.5">
                          {overall != null ? (
                            <span
                              title="报告综合评分（0–100）"
                              className={`inline-block rounded-full px-2 py-0.5 text-xs font-semibold tabular-nums ${scoreTone(overall)}`}
                            >
                              {overall}
                            </span>
                          ) : (
                            <span className="text-xs text-neutral-300">—</span>
                          )}
                        </td>
                        <td className="px-4 py-2.5 tabular-nums">{fmtDuration(s.durationMs)}</td>
                        <td className="px-4 py-2.5 tabular-nums">{s.fillerPerMinute} 次</td>
                        <td className="px-4 py-2.5 tabular-nums">
                          {s.speechRate > 0 ? `${s.speechRate} 字/分` : "—"}
                        </td>
                        <td className="px-4 py-2.5 tabular-nums">{s.runawayPauseCount} 次</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </div>
        )
      )}
    </div>
  );
}
