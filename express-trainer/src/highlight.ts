export interface HighlightPart {
  text: string;
  isFiller: boolean;
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * 按口头禅词表切分文本做红色标注。
 * 词表先按长度降序（最长优先），保证「然后就是」不被拆成「然后」+「就是」；
 * 词条做正则转义，用户自定义词里的特殊字符不会破坏正则。
 */
export function highlightFillers(text: string, fillers: string[]): HighlightPart[] {
  const words = [...new Set(fillers.filter((w) => w.length > 0))].sort(
    (a, b) => b.length - a.length,
  );
  if (words.length === 0) return [{ text, isFiller: false }];
  const pattern = new RegExp(`(${words.map(escapeRegExp).join("|")})`, "g");
  return text
    .split(pattern)
    .filter((s) => s.length > 0)
    .map((s) => ({ text: s, isFiller: words.includes(s) }));
}
