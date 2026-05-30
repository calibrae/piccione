// Compose-side text formatting → Signal bodyRanges.
//
// Users type markdown-ish markers; we strip them and emit explicit
// {start,length,style} ranges over the cleaned (UTF-16) text. The wire format
// is bodyRanges, which every Signal client renders — so the input convention
// here is purely our UX; recipients see proper bold/italic/etc.

export interface FmtRange {
  start: number;
  length: number;
  style: string;
}

// Longer markers first so `**` wins over `*`.
const MARKERS: { mark: string; style: string }[] = [
  { mark: "**", style: "bold" },
  { mark: "~~", style: "strikethrough" },
  { mark: "||", style: "spoiler" },
  { mark: "`", style: "monospace" },
  { mark: "*", style: "italic" },
];

export function parseFormatting(input: string): { text: string; ranges: FmtRange[] } {
  let out = "";
  const ranges: FmtRange[] = [];
  let i = 0;
  outer: while (i < input.length) {
    for (const { mark, style } of MARKERS) {
      if (input.startsWith(mark, i)) {
        const close = input.indexOf(mark, i + mark.length);
        // Require a non-empty body and a matching close.
        if (close > i + mark.length) {
          const inner = input.slice(i + mark.length, close);
          // Don't treat a marker as formatting if the inner itself is just
          // markers/whitespace.
          if (inner.trim().length > 0) {
            const start = out.length;
            out += inner;
            ranges.push({ start, length: inner.length, style });
            i = close + mark.length;
            continue outer;
          }
        }
      }
    }
    out += input[i];
    i += 1;
  }
  return { text: out, ranges };
}
