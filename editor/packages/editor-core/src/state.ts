// The parse outcome inside the CodeMirror state: token decorations and the
// block list, kept in step with edits until the next outcome arrives.
import { StateEffect, StateField, type EditorState, type Range } from "@codemirror/state";
import { Decoration, EditorView, type DecorationSet } from "@codemirror/view";
import type { Block, EditorViewData } from "./types.js";

export const setParsed = StateEffect.define<EditorViewData>();

export interface Parsed {
  /** The last view, with ranges moved through later edits. */
  view: EditorViewData | null;
  blocks: readonly Block[];
  tokens: DecorationSet;
}

const marks = new Map<string, Decoration>();
function mark(kind: string): Decoration {
  let found = marks.get(kind);
  if (!found) marks.set(kind, (found = Decoration.mark({ class: `adoc-${kind}` })));
  return found;
}

function tokenSet(view: EditorViewData, length: number): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  for (const [start, end, kind] of view.tokens) {
    if (start < end && end <= length) ranges.push(mark(kind).range(start, end));
  }
  // Tokens come in source order per kind of scan, not globally.
  return Decoration.set(ranges, true);
}

export const parsedField = StateField.define<Parsed>({
  create: () => ({ view: null, blocks: [], tokens: Decoration.none }),
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setParsed)) {
        const view = effect.value;
        return { view, blocks: view.blocks, tokens: tokenSet(view, tr.newDoc.length) };
      }
    }
    if (!tr.docChanged) return value;
    const map = (block: Block): Block => ({
      ...block,
      start: tr.changes.mapPos(block.start, 1),
      end: Math.max(tr.changes.mapPos(block.end, 1), tr.changes.mapPos(block.start, 1)),
      ...(block.meta_start === undefined
        ? {}
        : { meta_start: tr.changes.mapPos(block.meta_start, -1) }),
    });
    return {
      view: value.view,
      blocks: value.blocks.map(map),
      tokens: value.tokens.map(tr.changes),
    };
  },
  provide: (field) => EditorView.decorations.from(field, (value) => value.tokens),
});

export function parsed(state: EditorState): Parsed {
  return state.field(parsedField);
}
