import { useCallback, useEffect, useState } from "react";
import { loadSettings, saveSettings, type SaveSettingsOutcome } from "../lib/settings";
import type { Settings } from "../types";

export function useSettings() {
  const [settings, setSettings] = useState<Settings | null>(null);

  useEffect(() => {
    let cancelled = false;
    loadSettings().then((s) => {
      if (!cancelled) setSettings(s);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const update = useCallback((patch: Partial<Settings>) => {
    setSettings((prev) => (prev ? { ...prev, ...patch } : prev));
  }, []);

  /** 持久化（含 API Key → 系统凭据管理器路由）；返回 Key 存储结果供设置页提示 */
  const persist = useCallback(async (next?: Settings): Promise<SaveSettingsOutcome | null> => {
    const target = next ?? settings;
    if (!target) return null;
    setSettings(target);
    try {
      return await saveSettings(target);
    } catch {
      return null; // store 写失败等：静默（与既有行为一致）
    }
  }, [settings]);

  return { settings, update, persist };
}
