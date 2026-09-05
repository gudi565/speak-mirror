export interface HighlightPart {
  text: string;
  isFiller: boolean;
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** 词内字符（等效正则 \w 的 ASCII 部分）：英文字母 / 数字 / 下划线。
 *  CJK 不算——中文本无词边界，且保证混合句「然后 like 这个」里的 like
 *  两侧（CJK 字符）不构成边界阻挡。与 Rust WordMatcher 同口径。 */
function isAsciiWordChar(c: string): boolean {
  return /[A-Za-z0-9_]/.test(c);
}

/** 纯 ASCII 词条（含撇号如 don't、含空格短语如 you know）：按词边界匹配 */
function isAsciiEntry(w: string): boolean {
  return /^[\x21-\x7E]+$/.test(w) && /[A-Za-z0-9]/.test(w);
}

/** ASCII 词条的边界化模式：首尾是词内字符的一侧加 \b（等效 Rust 侧判断） */
function boundaryPattern(w: string): string {
  const lead = isAsciiWordChar(w[0]) ? "\\b" : "";
  const tail = isAsciiWordChar(w[w.length - 1]) ? "\\b" : "";
  return `${lead}${escapeRegExp(w)}${tail}`;
}

/**
 * 按口头禅词表切分文本做红色标注。
 * 词表先按长度降序（最长优先），保证「然后就是」不被拆成「然后」+「就是」、
 * 「sort of」先于「so」；词条做正则转义，用户自定义词里的特殊字符不会破坏正则。
 * 英文词条（纯 ASCII）按词边界匹配且大小写不敏感：「like」不再命中
 * 「likely」，句首「Like」照常标红；含 CJK 的词条保持子串匹配。
 */
export function highlightFillers(text: string, fillers: string[]): HighlightPart[] {
  const words = [...new Set(fillers.filter((w) => w.length > 0))].sort(
    (a, b) => b.length - a.length,
  );
  if (words.length === 0) return [{ text, isFiller: false }];
  const pattern = new RegExp(
    `(${words.map((w) => (isAsciiEntry(w) ? boundaryPattern(w) : escapeRegExp(w))).join("|")})`,
    "gi",
  );
  // 英文匹配大小写不敏感：命中文本与词表比对也走小写集合
  const lookup = new Set(words.map((w) => w.toLowerCase()));
  return text
    .split(pattern)
    .filter((s) => s.length > 0)
    .map((s) => ({ text: s, isFiller: lookup.has(s.toLowerCase()) }));
}
