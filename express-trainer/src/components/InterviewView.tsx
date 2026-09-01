import { useMemo, useState } from "react";
import {
  INTERVIEW_ROLES,
  QUESTION_COUNTS,
  buildQaPayload,
  interviewProgress,
  interviewRoleLabel,
  summarizeAnswers,
  type InterviewAnswers,
  type QuestionCount,
} from "../lib/interview";
import { hasRemoteBackend } from "../lib/settings";
import type { InterviewQuestion, Settings } from "../types";

/** App 持有的面试流程状态（视图路由与答题会话由 App 驱动） */
export interface InterviewState {
  role: string;
  source: "ai" | "offline";
  questions: InterviewQuestion[];
  answers: InterviewAnswers;
  /** 当前题下标（0 起始；phase=summary 时停留在最后一题） */
  current: number;
  phase: "question" | "summary";
}

interface Props {
  settings: Settings | null;
  interview: InterviewState | null;
  /** 出题请求进行中（按钮禁用 + 出题中…） */
  starting: boolean;
  /** 出题失败信息 */
  startError: string | null;
  /** 会话启动错误（开始回答失败时展示） */
  sessionError: string | null;
  /** 报告生成中（streaming） */
  generating: boolean;
  onStart: (role: string, count: QuestionCount, jd: string) => void;
  onStartAnswer: () => void;
  /** 重新回答当前题（清掉已记录回答再进 live） */
  onRestartAnswer: () => void;
  /** 前进到下一题（保留当前回答）；最后一题时进汇总 */
  onNext: () => void;
  /** 跳过当前题（回答置空并前进） */
  onSkip: () => void;
  /** 直接进入汇总页（提前结束面试） */
  onFinishEarly: () => void;
  /** 结束整场（回普通练习） */
  onExit: () => void;
  onEditAnswer: (index: number, text: string) => void;
  onGenerateReport: () => void;
}

export function InterviewView(props: Props) {
  const { interview } = props;
  if (!interview) return <InterviewConfig {...props} />;
  if (interview.phase === "summary")
    return <InterviewSummary {...props} interview={interview} />;
  return <InterviewQuestionPage {...props} interview={interview} />;
}

// ---------------------------------------------------------------------------
// 配置页
// ---------------------------------------------------------------------------

function InterviewConfig({
  settings,
  starting,
  startError,
  onStart,
}: Props) {
  const [role, setRole] = useState("general");
  const [count, setCount] = useState<QuestionCount>(3);
  const [jd, setJd] = useState("");

  const isCustom = role === "custom";
  const remoteReady = settings ? hasRemoteBackend(settings) : false;
  const jdMissing = isCustom && jd.trim().length === 0;

  return (
    <div className="mx-auto max-w-2xl px-6 py-10">
      <h2 className="text-lg font-semibold">模拟面试</h2>
      <p className="mt-1 text-sm text-neutral-500">
        选择岗位方向与题数，逐题口述回答（复用实时分析管线），结束后生成逐题
        STAR 点评的面试报告。
      </p>

      <div className="mt-6 space-y-5 rounded-xl border border-neutral-200 bg-white p-5">
        <div>
          <label className="mb-1 block text-sm font-medium text-neutral-700">岗位方向</label>
          <select
            value={role}
            onChange={(e) => setRole(e.target.value)}
            disabled={starting}
            className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm disabled:opacity-50"
          >
            {INTERVIEW_ROLES.map((r) => (
              <option key={r.value} value={r.value}>
                {r.label}
              </option>
            ))}
          </select>
          <p className="mt-1 text-xs text-neutral-400">
            {INTERVIEW_ROLES.find((r) => r.value === role)?.hint}
          </p>
        </div>

        <div>
          <label className="mb-1 block text-sm font-medium text-neutral-700">题数</label>
          <div className="flex gap-2">
            {QUESTION_COUNTS.map((c) => (
              <button
                key={c}
                onClick={() => setCount(c)}
                disabled={starting}
                className={`rounded-lg px-4 py-2 text-sm font-medium ${
                  count === c
                    ? "bg-neutral-900 text-white"
                    : "border border-neutral-300 text-neutral-700 hover:bg-neutral-100"
                } disabled:opacity-50`}
              >
                {c} 题
              </button>
            ))}
          </div>
        </div>

        <div>
          <label className="mb-1 block text-sm font-medium text-neutral-700">
            职位描述 JD{isCustom ? "（自定义岗位必填）" : "（可选，AI 会按 JD 出题）"}
          </label>
          <textarea
            value={jd}
            onChange={(e) => setJd(e.target.value)}
            disabled={starting}
            rows={5}
            placeholder="粘贴招聘 JD 或岗位要求，出题会更贴合目标岗位"
            className="w-full resize-y rounded-lg border border-neutral-300 px-3 py-2 text-sm disabled:opacity-50"
          />
        </div>

        {startError && (
          <p className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
            {startError}
          </p>
        )}

        <div className="flex items-center gap-3">
          <button
            onClick={() => onStart(role, count, jd)}
            disabled={starting || jdMissing}
            className="rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
          >
            {starting ? "出题中…" : "开始面试"}
          </button>
          {!remoteReady && (
            <span className="text-xs text-neutral-400">
              未配置 AI，将使用内置题库出题（可在设置中配置）
            </span>
          )}
          {jdMissing && (
            <span className="text-xs text-amber-600">自定义岗位需要先填写 JD</span>
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// 答题页（当前题；回答进行中由 App 切到 live 视图，这里只承载题面与流转）
// ---------------------------------------------------------------------------

function InterviewQuestionPage({
  interview,
  sessionError,
  onStartAnswer,
  onRestartAnswer,
  onNext,
  onSkip,
  onFinishEarly,
  onExit,
}: Props & { interview: InterviewState }) {
  const { questions, answers, current, source, role } = interview;
  const progress = interviewProgress(current, questions.length);
  const q = questions[current];
  const answered = (answers.texts[current] ?? "").trim().length > 0;

  return (
    <div className="mx-auto max-w-2xl px-6 py-8">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">模拟面试 · {interviewRoleLabel(role)}</h2>
        <div className="flex items-center gap-2 text-sm">
          <button
            onClick={onFinishEarly}
            className="rounded-lg border border-neutral-300 px-3 py-2 text-neutral-700 hover:bg-neutral-100"
          >
            提前结束面试
          </button>
          <button
            onClick={onExit}
            title="放弃本场面试，回到普通练习"
            className="rounded-lg px-3 py-2 text-neutral-500 hover:bg-neutral-100"
          >
            放弃
          </button>
        </div>
      </div>
      {source === "offline" && (
        <p className="mt-1 text-xs text-neutral-400">未配置 AI，使用内置题库</p>
      )}

      <div className="mt-5 rounded-xl border border-neutral-200 bg-white p-5">
        <div className="text-xs font-medium text-neutral-400">{progress.label}</div>
        <p className="mt-2 text-lg font-semibold leading-8">{q.question}</p>
        <p className="mt-2 text-sm text-neutral-500">考察点：{q.intent}</p>

        <div className="mt-5 flex flex-wrap items-center gap-3">
          {answered ? (
            <button
              onClick={onRestartAnswer}
              className="rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700"
            >
              重新回答本题
            </button>
          ) : (
            <button
              onClick={onStartAnswer}
              className="rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700"
            >
              开始回答
            </button>
          )}
          <button
            onClick={onNext}
            className="rounded-lg border border-neutral-300 px-4 py-2.5 text-sm text-neutral-700 hover:bg-neutral-100"
          >
            {progress.isLast ? "进入汇总" : "下一题"}
          </button>
          <button
            onClick={onSkip}
            className="rounded-lg border border-neutral-200 px-4 py-2.5 text-sm text-neutral-500 hover:bg-neutral-100"
          >
            跳过本题
          </button>
        </div>
        {answered && (
          <div className="mt-4">
            <div className="mb-1 text-xs font-medium text-neutral-400">
              本题回答（{(answers.texts[current] ?? "").trim().replace(/\s/g, "").length} 字）
            </div>
            <div className="max-h-40 overflow-y-auto whitespace-pre-wrap rounded-lg border border-neutral-100 bg-neutral-50 px-3 py-2 text-sm leading-6 text-neutral-600">
              {answers.texts[current]}
            </div>
          </div>
        )}
        {sessionError && (
          <p className="mt-3 text-sm text-red-600">开始回答失败：{sessionError}</p>
        )}
      </div>

      {/* 题目进度点 */}
      <div className="mt-4 flex items-center gap-1.5">
        {questions.map((_, i) => (
          <span
            key={i}
            title={`第 ${i + 1} 题`}
            className={`h-2 w-6 rounded-full ${
              i < current || (i === current && answered)
                ? "bg-neutral-800"
                : i === current
                  ? "bg-blue-400"
                  : "bg-neutral-200"
            }`}
          />
        ))}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// 汇总页：逐题 Q&A 预览（可编辑修正）+ 生成面试报告
// ---------------------------------------------------------------------------

function InterviewSummary({
  interview,
  generating,
  onEditAnswer,
  onGenerateReport,
  onExit,
}: Props & { interview: InterviewState }) {
  const { questions, answers, source, role } = interview;
  const stats = useMemo(() => summarizeAnswers(answers), [answers]);
  const qa = useMemo(() => buildQaPayload(questions, answers), [questions, answers]);

  return (
    <div className="mx-auto max-w-3xl px-6 py-8">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold">模拟面试汇总 · {interviewRoleLabel(role)}</h2>
        <button
          onClick={onExit}
          className="rounded-lg border border-neutral-300 px-3 py-2 text-sm text-neutral-700 hover:bg-neutral-100"
        >
          结束本场
        </button>
      </div>
      {source === "offline" && (
        <p className="mt-1 text-xs text-neutral-400">本场使用内置题库（未配置 AI）</p>
      )}
      <p className="mt-1 text-sm text-neutral-500">
        可先修正各题转写中的错字（修正后的逐字稿作为报告输入），再生成报告。
        作答 {stats.answered} 题 / 跳过 {stats.skipped} 题，共 {stats.totalChars} 字。
      </p>

      <div className="mt-5 space-y-4">
        {questions.map((q, i) => {
          const text = answers.texts[i] ?? "";
          const answered = text.trim().length > 0;
          return (
            <div key={i} className="rounded-xl border border-neutral-200 bg-white p-4">
              <div className="text-sm font-semibold leading-7">
                第 {i + 1} 题 · {q.question}
              </div>
              <div className="mt-0.5 text-xs text-neutral-400">考察点：{q.intent}</div>
              <textarea
                value={text}
                onChange={(e) => onEditAnswer(i, e.target.value)}
                rows={Math.min(Math.max(text.split("\n").length + 1, 3), 12)}
                placeholder={
                  answered
                    ? undefined
                    : "未识别到回答（可手动输入逐字稿，或保持空白按跳过计）"
                }
                className="mt-2 w-full resize-y rounded-lg border border-neutral-200 bg-neutral-50 px-3 py-2 text-sm leading-6"
              />
            </div>
          );
        })}
      </div>

      <div className="mt-6 flex items-center gap-3">
        <button
          onClick={onGenerateReport}
          disabled={generating || qa === null}
          className="rounded-lg bg-neutral-900 px-5 py-2.5 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
        >
          {generating ? "生成中…" : qa ? "生成面试报告" : "没有可分析的作答"}
        </button>
        <span className="text-xs text-neutral-400">
          报告含逐题 STAR 点评与全场数据；未配置 AI 时生成本地降级报告
        </span>
      </div>
    </div>
  );
}
