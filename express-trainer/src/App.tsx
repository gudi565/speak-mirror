import { useSession } from "./hooks/useSession";
import { SubtitleColumn } from "./components/SubtitleColumn";
import { FeedbackColumn } from "./components/FeedbackColumn";
import { StatsPanel } from "./components/StatsPanel";

export default function App() {
  const { running, partial, sentences, events, snapshot, error, fillerWords, start, stop } =
    useSession();

  return (
    <div className="flex h-screen flex-col bg-neutral-50 text-neutral-900">
      <header className="flex items-center justify-between border-b border-neutral-200 bg-white px-6 py-3">
        <h1 className="text-base font-semibold">表达训练系统</h1>
        <button
          onClick={running ? stop : start}
          className={`rounded-lg px-5 py-2 text-sm font-medium text-white ${
            running ? "bg-red-500 hover:bg-red-600" : "bg-neutral-900 hover:bg-neutral-700"
          }`}
        >
          {running ? "结束练习" : "开始练习"}
        </button>
      </header>
      {error && (
        <div className="border-b border-red-200 bg-red-50 px-6 py-2 text-sm text-red-700">
          {error}
        </div>
      )}
      <main className="grid min-h-0 flex-1 grid-cols-[260px_1fr_300px]">
        <aside className="border-r border-neutral-200 bg-white">
          <div className="border-b border-neutral-100 px-4 py-2 text-xs font-medium text-neutral-400">
            表达分析
          </div>
          <StatsPanel snapshot={snapshot} />
        </aside>
        <section className="min-w-0">
          <SubtitleColumn sentences={sentences} partial={partial} fillers={fillerWords} />
        </section>
        <aside className="border-l border-neutral-200 bg-neutral-50">
          <div className="border-b border-neutral-200 px-4 py-2 text-xs font-medium text-neutral-400">
            实时反馈
          </div>
          <FeedbackColumn events={events} />
        </aside>
      </main>
    </div>
  );
}
