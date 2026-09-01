/** 中栏字幕（定稿为主、预览为辅）纯函数：流式预览行的显示判定 */

/**
 * 是否显示「识别中…」流式预览行：
 * - 设置关闭（showLivePreview=false）→ 完全不显示
 * - partial 为空白（尚无语音 / 会话结束清空）→ 不显示
 * - 与最近定稿句重复 → 不显示：SenseVoice 定稿后，流式引擎的 partial 常滞后
 *   重吐刚定稿的那句（整段包含在定稿文本里、没有新内容），照旧显示会与上方
 *   定稿句闪现同样文字，观感差；partial 长出定稿句之外的新内容时才显示
 */
export function shouldShowPartial(
  partial: string,
  lastFinalText: string | null,
  showLivePreview: boolean,
): boolean {
  if (!showLivePreview) return false;
  const preview = partial.trim();
  if (preview.length === 0) return false;
  if (lastFinalText !== null && lastFinalText.trim().includes(preview)) return false;
  return true;
}
