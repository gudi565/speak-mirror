export interface HighlightPart {
  text: string;
  isFiller: boolean;
}

export function highlightFillers(text: string, fillers: string[]): HighlightPart[] {
  if (fillers.length === 0) return [{ text, isFiller: false }];
  const pattern = new RegExp(`(${fillers.join("|")})`, "g");
  return text
    .split(pattern)
    .filter((s) => s.length > 0)
    .map((s) => ({ text: s, isFiller: fillers.includes(s) }));
}
