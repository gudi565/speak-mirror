/** 首启引导（M4）纯函数：下载进度显示换算，供 OnboardingView 使用 */

export interface DownloadProgressState {
  received: number;
  total: number;
}

/** 下载百分比（0–100）；total 未知（0）→ null（调用方显示不确定进度） */
export function progressPercent(received: number, total: number): number | null {
  if (!Number.isFinite(received) || !Number.isFinite(total) || total <= 0) return null;
  return Math.min(100, (received / total) * 100);
}

/** 字节数人类可读化：B 取整，KB/MB/GB 保留一位小数 */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  const units = ["B", "KB", "MB", "GB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

/** 下载文件的中文名（OnboardingView 展示用） */
export const DOWNLOAD_FILE_LABELS: Record<string, string> = {
  "silero_vad.onnx": "断句模型（Silero VAD）",
  "paraformer.tar.bz2": "识别模型压缩包（Paraformer）",
  "tokens.txt": "模型词表",
  "encoder.int8.onnx": "识别模型编码器（int8）",
  "decoder.int8.onnx": "识别模型解码器（int8）",
  "sense-voice.tar.bz2": "高精引擎压缩包（SenseVoice）",
  "model.int8.onnx": "高精引擎（int8）",
};

export function downloadFileLabel(file: string): string {
  return DOWNLOAD_FILE_LABELS[file] ?? file;
}
