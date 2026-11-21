// Folding from the block list: sections and multi-line blocks.
import { codeFolding, foldGutter, foldService } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { parsed, parsedField } from "../state.js";

const FOLDABLE = new Set([
  "section",
  "listing",
  "literal",
  "pass",
  "stem",
  "example",
  "sidebar",
  "quote",
  "verse",
  "open",
  "admonition",
  "table",
  "list",
  "dlist",
]);

export function folding(): Extension {
  return [
    parsedField,
    codeFolding(),
    foldGutter(),
    foldService.of((state, lineStart, lineEnd) => {
      let range: { from: number; to: number } | null = null;
      for (const block of parsed(state).blocks) {
        if (block.start > lineEnd) break;
        // The outermost foldable block starting on the line wins.
        if (block.start >= lineStart && block.end > lineEnd && FOLDABLE.has(block.kind)) {
          range = { from: lineEnd, to: Math.min(block.end, state.doc.length) };
          break;
        }
      }
      return range;
    }),
  ];
}
