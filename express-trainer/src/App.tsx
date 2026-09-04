import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { hasRemoteBackend } from "./lib/settings";
import { fmtDuration } from "./lib/history";
import { SCENARIOS, scenarioMeta } from "./lib/scenarios";
import {
  buildQaPayload,
  editAnswer as editInterviewAnswer,
  interviewProgress,
  interviewRoleLabel,
  makeAnswers,
  recordAnswer,
  type QuestionCount,
} from "./lib/interview";
import { useSession, type CheckinConfig } from "./hooks/useSession";
import { useSettings } from "./hooks/useSettings";
import { useReport } from "./hooks/useReport";
import { SubtitleColumn } from "./components/SubtitleColumn";
import { FeedbackColumn } from "./components/FeedbackColumn";
import { StatsPanel } from "./components/StatsPanel";
import { SettingsView } from "./components/SettingsView";
import { SummaryView } from "./components/SummaryView";
import { ReportView } from "./components/ReportView";
import { HistoryView } from "./components/HistoryView";
import { InterviewView, type InterviewState } from "./components/InterviewView";
import { OnboardingView } from "./components/OnboardingView";
import type {
  InterviewQuestionsResult,
  ReportMode,
  Scenario,
  Settings,
  TranscriptResult,
} from "./types";
import { FILE_SPEED_MAX, speedLabel } from "./types";

type View = "live" | "summary" | "report" | "history" | "interview";

/** 从文件练习：文件选择器接受的扩展名（wav 之外尽力而为，解码失败有中文提示） */
const AUDIO_FILTERS = [
  { name: "音频文件（wav / mp3 / flac）", extensions: ["wav", "mp3", "flac", "m4a", "aac"] },
];

/** 文件练习倍速档位（与后端 FILE_SPEEDS 白名单一致；极速 = 不限速，跳过实时字幕滚动） */
const SPEED_OPTIONS = [1.0, 1.5, 2.0, FILE_SPEED_MAX] as const;

/** 已选中待开始的音频文件（probe 过，带时长与倍速选择） */
interface FilePick {
  path: string;
  name: string;
  durationMs: number;
  speed: number;
}

/** 结束会话后读本次录音路径（无录音为 null） */
async function fetchLastAudio(): Promise<string | null> {
  try {
    return await invoke<string | null>("get_last_audio");
  } catch {
    return null;
  }
}

export default function App() {
  const { settings, update, persist } = useSettings();
  const report = useReport();

  const [view, setView] = useState<View>("live");
  const [settingsOpen, setSettingsOpen] = useState(false);
  // 历史落盘失败提示（Rust 只发一次 history_error；主流程不受影响）
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [scenario, setScenario] = useState<Scenario>("free");
  const [topic, setTopic] = useState("");
  // 报告模式（默认 quick：用户痛点是报告慢；模拟面试固定走完整版）
  const [reportMode, setReportMode] = useState<ReportMode>("quick");
  // 开始界面的可选「计划时长」（分钟）：AI 周期快评判断「该收结论」的依据之一
  const [plannedMin, setPlannedMin] = useState("");
  // 已忽略的反馈事件 uid（本会话内不再显示；仅影响 UI 不影响统计）
  const [dismissed, setDismissed] = useState<Set<number>>(new Set());
  // 报告导出需要知道生成时用的逐字稿（可能是用户修正稿）
  const [reportTranscript, setReportTranscript] = useState<string | null>(null);
  // 从文件练习：已选中待开始的文件、选择/探测错误、进行中会话的文件名标记
  const [filePick, setFilePick] = useState<FilePick | null>(null);
  const [pickError, setPickError] = useState<string | null>(null);
  const [activeFileName, setActiveFileName] = useState<string | null>(null);
  const [activeSpeed, setActiveSpeed] = useState(1.0);
  // 本次会话录音路径（结束后供总结页逐句回放；进行中为 null）
  const [lastAudio, setLastAudio] = useState<string | null>(null);
  // 模拟面试流程状态（null = 未在面试模式；答题会话复用 useSession 管线）
  const [interview, setInterview] = useState<InterviewState | null>(null);
  const [interviewStarting, setInterviewStarting] = useState(false);
  const [interviewStartError, setInterviewStartError] = useState<string | null>(null);

  // AI 周期快评：开关开 且 配置了远端后端 才真正生效（默认关闭）。
  // 面试答题期间以当前题目为主题锚点
  const interviewRef = useRef<InterviewState | null>(null);
  interviewRef.current = interview;
  const checkinTopic =
    interview && view === "live"
      ? (interview.questions[interview.current]?.question ?? topic)
      : topic;
  const checkinConfig: CheckinConfig | undefined =
    settings && settings.realtimeCheckinEnabled
      ? {
          enabled: hasRemoteBackend(settings),
          intervalSec: settings.realtimeCheckinIntervalSec,
          topic: checkinTopic,
        }
      : undefined;
  const { running, partial, sentences, events, snapshot, voice, toneFlags, error, pending, fillerWords, start, startFromFile, stop } =
    useSession(checkinConfig);

  // 设置加载后，场景选择器落到默认场景
  useEffect(() => {
    if (settings) setScenario(settings.scenario);
  }, [settings?.scenario]); // eslint-disable-line react-hooks/exhaustive-deps

  // 历史落盘失败提示（一次性横幅，可手动关闭）
  useEffect(() => {
    let un: (() => void) | undefined;
    listen<{ message: string }>("history_error", (e) => setHistoryError(e.payload.message)).then(
      (f) => {
        un = f;
      },
    );
    return () => un?.();
  }, []);

  const handleStart = async () => {
    report.reset();
    setDismissed(new Set());
    setView("live");
    setActiveFileName(null);
    setActiveSpeed(1.0);
    setLastAudio(null);
    const plannedSec = Number(plannedMin) > 0 ? Math.round(Number(plannedMin) * 60) : null;
    await start(plannedSec);
  };

  /** 停止收尾：拿最终快照进入总结页，并读取本次录音路径（回放入口） */
  const finishSession = async () => {
    await stop();
    setLastAudio(await fetchLastAudio());
    setView("summary");
  };

  const handleStop = finishSession;

  // -------------------------------------------------------------------------
  // 模拟面试（前端驱动逐题复用现有会话管线）
  // -------------------------------------------------------------------------

  const handleStartInterview = async (
    role: string,
    count: QuestionCount,
    jd: string,
  ) => {
    setInterviewStarting(true);
    setInterviewStartError(null);
    try {
      const r = await invoke<InterviewQuestionsResult>(
        "generate_interview_questions",
        { role, count, jd: jd.trim() || null },
      );
      if (r.questions.length === 0) {
        setInterviewStartError("出题失败：题库返回为空，请重试");
        return;
      }
      setInterview({
        role,
        source: r.source,
        questions: r.questions,
        answers: makeAnswers(r.questions.length),
        current: 0,
        phase: "question",
      });
      setView("interview");
    } catch (e) {
      setInterviewStartError(String(e));
    } finally {
      setInterviewStarting(false);
    }
  };

  /** 开始回答当前题：进 live 界面答题（顶部题目横幅 + 结束回答） */
  const startInterviewAnswer = async () => {
    if (!interview || pending) return;
    report.reset();
    setDismissed(new Set());
    setLastAudio(null);
    setView("live");
    await start(null);
  };

  const handleRestartInterviewAnswer = async () => {
    setInterview((prev) =>
      prev ? { ...prev, answers: editInterviewAnswer(prev.answers, prev.current, "") } : prev,
    );
    await startInterviewAnswer();
  };

  /** 结束当前题回答：终稿从后端取（避免与 sentence_final 事件竞态），累积进答案 */
  const finishInterviewAnswer = async () => {
    if (!interview) return;
    const snap = await stop();
    let text = "";
    try {
      const r = await invoke<TranscriptResult>("get_transcript");
      text = r.sentences.map((s) => s.text).join("\n");
    } catch {
      text = ""; // 取稿失败按未作答处理，用户可在汇总页手填
    }
    setInterview((prev) =>
      prev
        ? {
            ...prev,
            answers: recordAnswer(prev.answers, prev.current, text, snap?.durationMs ?? 0),
          }
        : prev,
    );
    setView("interview");
  };

  /** 下一题（保留当前回答）；跳过 = 清空当前回答再前进；最后一题均进汇总 */
  const interviewAdvance = (skip: boolean) => {
    setInterview((prev) => {
      if (!prev) return prev;
      let next = prev;
      if (skip) {
        const texts = [...prev.answers.texts];
        const durations = [...prev.answers.durations];
        texts[prev.current] = "";
        durations[prev.current] = 0;
        next = { ...prev, answers: { texts, durations } };
      }
      const progress = interviewProgress(next.current, next.questions.length);
      if (progress.nextIndex === null) return { ...next, phase: "summary" };
      return { ...next, current: progress.nextIndex, phase: "question" };
    });
  };

  const handleFinishInterviewEarly = () => {
    setInterview((prev) => (prev ? { ...prev, phase: "summary" } : prev));
  };

  /** 结束整场面试（放弃 / 汇总页关闭）：回普通练习 */
  const handleExitInterview = () => {
    setInterview(null);
    setView("live");
  };

  const handleEditInterviewAnswer = (index: number, text: string) => {
    setInterview((prev) =>
      prev ? { ...prev, answers: editInterviewAnswer(prev.answers, index, text) } : prev,
    );
  };

  const handleGenerateInterviewReport = () => {
    if (!interview) return;
    const qa = buildQaPayload(interview.questions, interview.answers);
    if (!qa) return;
    setReportTranscript(null);
    setView("report");
    report.generate({
      scenario: "mockInterview",
      topic: interviewRoleLabel(interview.role),
      transcript: null,
      qa,
    });
  };

  // 从文件练习：选文件 → 探测时长（失败立刻给中文错误）→ 确认后开始
  const handlePickFile = async () => {
    setPickError(null);
    let path: string | null = null;
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        filters: AUDIO_FILTERS,
      });
      path = typeof picked === "string" ? picked : null;
    } catch (e) {
      setPickError(`打开文件选择器失败：${String(e)}`);
      return;
    }
    if (!path) return; // 用户取消
    try {
      const meta = await invoke<{ durationMs: number }>("probe_audio_file", { path });
      const name = path.split(/[\\/]/).pop() ?? path;
      // 默认极速（不限速）：无麦克风用户不必等素材实时时长，统计仍按内容时间
      setFilePick({ path, name, durationMs: meta.durationMs, speed: FILE_SPEED_MAX });
    } catch (e) {
      // 文件不存在 / 格式不支持 / 解码失败（后端给中文可操作提示）
      setPickError(String(e));
      setFilePick(null);
    }
  };

  const handleStartFromFile = async () => {
    if (!filePick || pending) return;
    report.reset();
    setDismissed(new Set());
    setPickError(null);
    setView("live");
    setLastAudio(null);
    const plannedSec = Number(plannedMin) > 0 ? Math.round(Number(plannedMin) * 60) : null;
    await startFromFile(filePick.path, plannedSec, filePick.speed);
    setActiveFileName(filePick.name);
    setActiveSpeed(filePick.speed);
  };

  // 文件播放到头：后端发 session_finished（终稿已 flush），前端按「停止」收尾——
  // stop_session 对已结束的线程会直接返回快照，然后进入总结页（面试模式回答题页）
  const runningRef = useRef(running);
  runningRef.current = running;
  useEffect(() => {
    let un: (() => void) | undefined;
    listen("session_finished", () => {
      if (!runningRef.current) return; // 已手动停止（stop 也走同一收尾）
      if (interviewRef.current) void finishInterviewAnswer();
      else void finishSession();
    }).then((f) => {
      un = f;
    });
    return () => un?.();
  }, [stop]);

  const handleDismiss = (uid: number) => {
    setDismissed((prev) => new Set(prev).add(uid));
  };

  const visibleEvents = events.filter(
    (e) => e.uid === undefined || !dismissed.has(e.uid),
  );

  const handleGenerate = (transcript: string | null) => {
    setReportTranscript(transcript);
    setView("report");
    report.generate({ scenario, topic, transcript, mode: reportMode });
  };

  // 首启引导（M4）：未完成时以整屏向导替代主界面；完成/跳过时写 onboardingDone
  const finishOnboarding = async () => {
    if (!settings) return;
    await persist({ ...settings, onboardingDone: true });
  };

  // 设置保存（含 AI 快评开关）：把开关重新打开时同时恢复被自动停用的快评
  const persistSettings = async (next?: Settings) => {
    const outcome = await persist(next);
    if (next?.realtimeCheckinEnabled) {
      try {
        await invoke("reset_checkin");
      } catch {
        /* 非 Tauri 环境忽略 */
      }
    }
    return outcome;
  };

  if (settings && !settings.onboardingDone) {
    return (
      <OnboardingView
        settings={settings}
        onUpdate={update}
        onSave={persist}
        onFinish={finishOnboarding}
      />
    );
  }

  return (
    <div className="flex h-screen flex-col bg-neutral-50 text-neutral-900">
      <header className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-neutral-200 bg-white px-6 py-3">
        <div className="flex min-w-0 flex-wrap items-center gap-3">
          <h1 className="shrink-0 text-base font-semibold">SpeakMirror 表达镜</h1>
          {view === "live" && running && activeFileName && (
            <span
              title={`从文件练习：${
                activeSpeed === FILE_SPEED_MAX
                  ? "极速处理（不限速喂入，跳过实时字幕滚动）"
                  : `按 ${activeSpeed} 倍速实时分析`
              }`}
              className="max-w-56 truncate rounded-full bg-neutral-100 px-2.5 py-1 text-xs text-neutral-500"
            >
              🎧 {activeFileName} · {speedLabel(activeSpeed)}
            </span>
          )}
          {view === "live" && interview && (
            <span
              title={interview.questions[interview.current]?.intent}
              className="max-w-72 truncate rounded-full bg-indigo-50 px-2.5 py-1 text-xs text-indigo-700"
            >
              🎤 模拟面试 · 第 {interview.current + 1}/{interview.questions.length} 题
            </span>
          )}
          {view === "live" && !interview && (
            <>
              <select
                value={scenario}
                disabled={running}
                onChange={(e) => setScenario(e.target.value as Scenario)}
                className="rounded-lg border border-neutral-300 px-2 py-1.5 text-sm disabled:opacity-50"
              >
                {SCENARIOS.map((s) => (
                  <option key={s.value} value={s.value}>
                    {s.label}
                  </option>
                ))}
              </select>
              {/* 开始前的可选主题：喂给 AI 周期快评与报告 */}
              <input
                value={topic}
                disabled={running}
                onChange={(e) => setTopic(e.target.value)}
                placeholder={scenarioMeta(scenario).topicPlaceholder}
                className="w-44 min-w-0 max-w-full rounded-lg border border-neutral-300 px-2 py-1.5 text-sm disabled:opacity-50"
              />
              {/* 计划时长（分钟，可选）：快评判断「该收结论」用 */}
              <input
                value={plannedMin}
                disabled={running}
                onChange={(e) => setPlannedMin(e.target.value.replace(/[^\d.]/g, ""))}
                placeholder="计划分钟（可选）"
                title="计划时长：快评在进度超 80% 且无收束迹象时提醒收结论"
                className="w-28 min-w-0 rounded-lg border border-neutral-300 px-2 py-1.5 text-sm disabled:opacity-50"
              />
            </>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {view === "live" && !running && !interview && (
            <button
              onClick={handlePickFile}
              disabled={pending}
              title="选一段本地音频（wav/mp3/flac），按 1.0–2.0 倍速或极速（不限速）走完全相同的实时分析"
              className="shrink-0 rounded-lg border border-neutral-300 px-4 py-2 text-sm font-medium text-neutral-700 hover:bg-neutral-100 disabled:opacity-50"
            >
              从文件练习
            </button>
          )}
          {view === "live" && interview && !running && (
            <button
              onClick={() => setView("interview")}
              className="rounded-lg border border-indigo-200 px-4 py-2 text-sm font-medium text-indigo-700 hover:bg-indigo-50"
            >
              返回面试
            </button>
          )}
          {view === "live" && !interview && (
            <button
              disabled={pending}
              onClick={running ? handleStop : handleStart}
              className={`shrink-0 rounded-lg px-5 py-2 text-sm font-medium text-white disabled:opacity-50 ${
                running ? "bg-red-500 hover:bg-red-600" : "bg-neutral-900 hover:bg-neutral-700"
              }`}
            >
              {running ? "结束练习" : "开始练习"}
            </button>
          )}
          {view === "live" && interview && running && (
            <button
              disabled={pending}
              onClick={finishInterviewAnswer}
              title="结束本题回答，进入下一题"
              className="rounded-lg bg-red-500 px-5 py-2 text-sm font-medium text-white hover:bg-red-600 disabled:opacity-50"
            >
              结束回答
            </button>
          )}
          {/* 模拟面试入口：练习进行中隐藏（避免误触离开实时界面） */}
          {!(view === "live" && running) && (
            <button
              onClick={() => setView("interview")}
              title="逐题口述的模拟面试：出题、作答、逐题 STAR 报告"
              className={`rounded-lg px-4 py-2 text-sm font-medium ${
                view === "interview"
                  ? "bg-neutral-900 text-white"
                  : "border border-neutral-300 text-neutral-700 hover:bg-neutral-100"
              }`}
            >
              模拟面试
            </button>
          )}
          {/* 成长档案入口：练习进行中隐藏（避免误触离开实时界面） */}
          {!(view === "live" && running) && (
            <button
              onClick={() => setView("history")}
              className={`rounded-lg px-4 py-2 text-sm font-medium ${
                view === "history"
                  ? "bg-neutral-900 text-white"
                  : "border border-neutral-300 text-neutral-700 hover:bg-neutral-100"
              }`}
            >
              历史
            </button>
          )}
          <button
            onClick={() => setSettingsOpen(true)}
            title="设置"
            className="rounded-lg px-3 py-2 text-lg leading-none text-neutral-500 hover:bg-neutral-100"
          >
            ⚙️
          </button>
        </div>
      </header>
      {/* 面试答题横幅：进入 live 界面回答当前题时显示题目 */}
      {view === "live" && interview && (
        <div className="flex items-center gap-3 border-b border-indigo-100 bg-indigo-50 px-6 py-2 text-sm text-indigo-900">
          <span className="shrink-0 rounded bg-indigo-600 px-2 py-0.5 text-xs font-medium text-white">
            第 {interview.current + 1} / {interview.questions.length} 题
          </span>
          <span
            className="min-w-0 flex-1 truncate font-medium"
            title={interview.questions[interview.current]?.question}
          >
            {interview.questions[interview.current]?.question}
          </span>
          <span
            className="hidden max-w-64 shrink-0 truncate text-xs text-indigo-400 md:inline"
            title={interview.questions[interview.current]?.intent}
          >
            考察点：{interview.questions[interview.current]?.intent}
          </span>
        </div>
      )}
      {(error ?? pickError) && view === "live" && (
        <div className="border-b border-red-200 bg-red-50 px-6 py-2 text-sm text-red-700">
          {error ?? pickError}
        </div>
      )}
      {view === "live" && !running && !interview && filePick && !error && (
        <div className="flex items-center justify-between gap-4 border-b border-blue-100 bg-blue-50 px-6 py-2 text-sm text-blue-900">
          <span className="min-w-0 truncate">
            🎧 已选择 <span className="font-medium">{filePick.name}</span>
            （{fmtDuration(filePick.durationMs)}）·{" "}
            <select
              value={filePick.speed}
              onChange={(e) =>
                setFilePick({ ...filePick, speed: Number(e.target.value) })
              }
              className="rounded border border-blue-200 bg-white px-1 py-0.5 text-xs"
            >
              {SPEED_OPTIONS.map((s) => (
                <option key={s} value={s}>
                  {s === FILE_SPEED_MAX ? "极速（不限速）" : `${s} 倍速`}
                </option>
              ))}
            </select>{" "}
            {filePick.speed === FILE_SPEED_MAX
              ? "极速处理（只出终稿句，时间戳与统计按素材内容时间）"
              : "实时分析（时间戳与统计按素材内容时间）"}
          </span>
          <span className="flex shrink-0 items-center gap-2">
            <button
              onClick={handleStartFromFile}
              disabled={pending}
              className="rounded-lg bg-neutral-900 px-3 py-1.5 text-xs font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
            >
              开始文件练习
            </button>
            <button
              onClick={() => setFilePick(null)}
              title="取消"
              className="rounded px-2 py-1 text-blue-700 hover:bg-blue-100"
            >
              ✕
            </button>
          </span>
        </div>
      )}
      {historyError && (
        <div className="flex items-center justify-between border-b border-amber-200 bg-amber-50 px-6 py-2 text-sm text-amber-800">
          <span>{historyError}（不影响本次练习与报告）</span>
          <button
            onClick={() => setHistoryError(null)}
            className="rounded px-2 py-0.5 text-amber-700 hover:bg-amber-100"
          >
            ✕
          </button>
        </div>
      )}

      {view === "live" && (
        <main className="grid min-h-0 flex-1 grid-cols-[260px_1fr_300px]">
          <aside className="border-r border-neutral-200 bg-white">
            <div className="border-b border-neutral-100 px-4 py-2 text-xs font-medium text-neutral-400">
              表达分析
            </div>
            <StatsPanel snapshot={snapshot} voice={voice} />
          </aside>
          <section className="min-w-0">
            <SubtitleColumn
              sentences={sentences}
              partial={partial}
              fillers={fillerWords}
              showLivePreview={settings?.showLivePreview ?? true}
            />
          </section>
          <aside className="border-l border-neutral-200 bg-neutral-50">
            <div className="border-b border-neutral-200 px-4 py-2 text-xs font-medium text-neutral-400">
              实时反馈
            </div>
            <FeedbackColumn events={visibleEvents} onDismiss={handleDismiss} />
          </aside>
        </main>
      )}

      {view === "summary" && (
        <main className="min-h-0 flex-1 overflow-y-auto">
          <SummaryView
            sentences={sentences}
            snapshot={snapshot}
            settings={settings}
            scenario={scenario}
            onScenarioChange={setScenario}
            topic={topic}
            onTopicChange={setTopic}
            reportMode={reportMode}
            onReportModeChange={setReportMode}
            onGenerate={handleGenerate}
            onRestart={handleStart}
            generating={report.status === "streaming"}
            audioPath={lastAudio}
            toneFlags={toneFlags}
          />
        </main>
      )}

      {view === "report" && (
        <main className="flex min-h-0 flex-1 flex-col overflow-hidden">
          <ReportView
            status={report.status}
            text={report.text}
            error={report.error}
            scenario={interview ? "mockInterview" : scenario}
            topic={interview ? interviewRoleLabel(interview.role) : topic}
            transcript={reportTranscript}
            mode={interview ? "full" : reportMode}
            onRetry={report.retry}
            onRestart={interview ? handleExitInterview : handleStart}
          />
        </main>
      )}

      {view === "history" && (
        <main className="min-h-0 flex-1 overflow-y-auto">
          <HistoryView
            settings={settings}
            onBack={() => setView(interview ? "interview" : "live")}
          />
        </main>
      )}

      {view === "interview" && (
        <main className="min-h-0 flex-1 overflow-y-auto">
          <InterviewView
            settings={settings}
            interview={interview}
            starting={interviewStarting}
            startError={interviewStartError}
            sessionError={error}
            generating={report.status === "streaming"}
            onStart={handleStartInterview}
            onStartAnswer={startInterviewAnswer}
            onRestartAnswer={handleRestartInterviewAnswer}
            onNext={() => interviewAdvance(false)}
            onSkip={() => interviewAdvance(true)}
            onFinishEarly={handleFinishInterviewEarly}
            onExit={handleExitInterview}
            onEditAnswer={handleEditInterviewAnswer}
            onGenerateReport={handleGenerateInterviewReport}
          />
        </main>
      )}

      {settingsOpen && settings && (
        <SettingsView
          settings={settings}
          onUpdate={update}
          onSave={persistSettings}
          onClose={() => setSettingsOpen(false)}
        />
      )}
    </div>
  );
}
