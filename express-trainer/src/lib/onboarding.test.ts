import { describe, expect, it } from "vitest";
import { downloadFileLabel, formatBytes, progressPercent } from "./onboarding";

describe("progressPercent", () => {
  it("returns null for unknown total (0 / NaN)", () => {
    expect(progressPercent(123, 0)).toBeNull();
    expect(progressPercent(123, Number.NaN)).toBeNull();
    expect(progressPercent(Number.NaN, 100)).toBeNull();
  });

  it("computes percent and caps at 100", () => {
    expect(progressPercent(0, 200)).toBe(0);
    expect(progressPercent(100, 200)).toBe(50);
    expect(progressPercent(150, 200)).toBe(75);
    // 续传瞬间 received 可能超过估计 total → 封顶
    expect(progressPercent(300, 200)).toBe(100);
  });
});

describe("formatBytes", () => {
  it("formats common model sizes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    // Silero VAD 实际大小
    expect(formatBytes(643_854)).toBe("628.8 KB");
    // Paraformer 归档约 1GB（未到 GB 阈值，显示 MB）
    expect(formatBytes(1_047_319_737)).toBe("998.8 MB");
    expect(formatBytes(2 * 1024 ** 3)).toBe("2.0 GB");
  });

  it("degrades gracefully for invalid input", () => {
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe("—");
  });
});

describe("downloadFileLabel", () => {
  it("maps known download file names and passes through unknown ones", () => {
    expect(downloadFileLabel("silero_vad.onnx")).toBe("断句模型（Silero VAD）");
    expect(downloadFileLabel("paraformer.tar.bz2")).toContain("Paraformer");
    expect(downloadFileLabel("encoder.int8.onnx")).toContain("int8");
    expect(downloadFileLabel("whatever.bin")).toBe("whatever.bin");
  });
});
