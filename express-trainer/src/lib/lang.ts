/**
 * 语言检测（纯函数）：与 Rust rules/lang.rs 同口径——
 * CJK 字符占「CJK + 拉丁字母」总数的比例（数字/标点/空白不参与），
 * ≥30% 视为中文，否则（有字母）视为英文；无任何字母视为未知（按中文处理）。
 * 目前唯一消费方：英文练习（占比 <30%）时隐藏普通话声调面板。
 */

/** CJK 占比阈值：低于该值视为英文会话（与 Rust CJK_RATIO_THRESHOLD 一致） */
export const CJK_RATIO_THRESHOLD = 0.3;

/** CJK 统一表意文字区（基本区 + 扩展 A + 兼容区，与 Rust is_cjk 同区间） */
const CJK_RE = /[㐀-䶿一-鿿豈-﫿]/g;
const LATIN_RE = /[A-Za-z]/g;

function countAll(text: string, re: RegExp): number {
  return (text.match(re) ?? []).length;
}

/** 单段文本的 CJK 占比（无字母内容时为 0） */
export function cjkRatio(text: string): number {
  const cjk = countAll(text, CJK_RE);
  const latin = countAll(text, LATIN_RE);
  const denom = cjk + latin;
  return denom === 0 ? 0 : cjk / denom;
}

/**
 * 会话整体 CJK 占比（逐字稿全部句子合并统计）。
 * 无任何字母内容（含空会话）返回 null——按中文处理，保持旧行为。
 */
export function sentencesCjkRatio(sentences: { text: string }[]): number | null {
  let cjk = 0;
  let latin = 0;
  for (const s of sentences) {
    cjk += countAll(s.text, CJK_RE);
    latin += countAll(s.text, LATIN_RE);
  }
  const denom = cjk + latin;
  return denom === 0 ? null : cjk / denom;
}
