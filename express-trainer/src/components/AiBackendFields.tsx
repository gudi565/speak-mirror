import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BACKEND_LABELS, BACKEND_PRESETS } from "../lib/settings";
import type { AiBackend, Settings, TestConnectionResult } from "../types";

interface Props {
  settings: Settings;
  onUpdate: (patch: Partial<Settings>) => void;
  /** 「测试连接」前先持久化当前表单（Rust 侧从 store 读配置）；返回值含 Key 存储结果 */
  onSave: (next: Settings) => Promise<unknown>;
  /** 挂载后聚焦后端下拉（「1 分钟开启」横幅入口） */
  autoFocus?: boolean;
}

const BACKENDS: AiBackend[] = ["deepseek", "openai", "groq", "ollama", "custom"];

/**
 * AI 后端配置块（后端选择 / API Key / Base URL / 模型名 / 测试连接）。
 * SettingsView 与首启向导（OnboardingView 第 2 步）共用。
 */
export function AiBackendFields({ settings, onUpdate, onSave, autoFocus }: Props) {
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<TestConnectionResult | null>(null);
  const selectRef = useRef<HTMLSelectElement>(null);

  useEffect(() => {
    if (autoFocus) selectRef.current?.focus();
  }, [autoFocus]);

  const selectBackend = (backend: AiBackend) => {
    setTestResult(null);
    if (backend === "custom") {
      onUpdate({ aiBackend: backend });
      return;
    }
    const preset = BACKEND_PRESETS[backend];
    // 选预设自动填 baseURL / model（仍可手改覆盖）
    onUpdate({
      aiBackend: backend,
      baseUrl: preset.baseUrl,
      modelName: preset.model,
    });
  };

  const handleTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      await onSave(settings);
      const result = await invoke<TestConnectionResult>("test_connection");
      setTestResult(result);
    } catch (e) {
      setTestResult({ ok: false, latencyMs: 0, error: String(e) });
    } finally {
      setTesting(false);
    }
  };

  return (
    <div className="space-y-4">
      <div>
        <label className="mb-1 block text-sm font-medium text-neutral-700">AI 后端</label>
        <select
          ref={selectRef}
          value={settings.aiBackend}
          onChange={(e) => selectBackend(e.target.value as AiBackend)}
          className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
        >
          {BACKENDS.map((b) => (
            <option key={b} value={b}>
              {BACKEND_LABELS[b]}
            </option>
          ))}
        </select>
      </div>

      <div>
        <label className="mb-1 block text-sm font-medium text-neutral-700">
          API Key{settings.aiBackend === "ollama" && "（Ollama 本地服务无需填写）"}
        </label>
        <input
          type="password"
          value={settings.apiKey}
          onChange={(e) => onUpdate({ apiKey: e.target.value })}
          placeholder="sk-…（保存在本机系统凭据管理器）"
          className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
        />
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className="mb-1 block text-sm font-medium text-neutral-700">Base URL</label>
          <input
            value={settings.baseUrl}
            onChange={(e) => onUpdate({ baseUrl: e.target.value })}
            placeholder="留空使用预设默认"
            className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
          />
        </div>
        <div>
          <label className="mb-1 block text-sm font-medium text-neutral-700">模型名</label>
          <input
            value={settings.modelName}
            onChange={(e) => onUpdate({ modelName: e.target.value })}
            placeholder="留空使用预设默认"
            className="w-full rounded-lg border border-neutral-300 px-3 py-2 text-sm"
          />
        </div>
      </div>

      <div className="flex items-center gap-3">
        <button
          onClick={handleTest}
          disabled={testing}
          className="rounded-lg bg-neutral-900 px-4 py-2 text-sm font-medium text-white hover:bg-neutral-700 disabled:opacity-50"
        >
          {testing ? "测试中…" : "测试连接"}
        </button>
        {testResult && (
          <span
            className={`text-sm ${testResult.ok ? "text-green-600" : "text-red-600"}`}
            title={testResult.error ?? ""}
          >
            {testResult.ok
              ? `连接成功，延迟 ${testResult.latencyMs} ms`
              : testResult.error ?? "连接失败"}
          </span>
        )}
      </div>
    </div>
  );
}
