// Formatting commands: wrap the selection in constrained marks.
import { EditorSelection, type Extension } from "@codemirror/state";
import { keymap, type Command } from "@codemirror/view";

/** Toggle `mark` (`*`, `_`, `` ` ``, `#`) around each selection range. */
export function toggleMark(mark: string): Command {
  return (view) => {
    const { state } = view;
    view.dispatch(
      state.changeByRange((range) => {
        const size = mark.length;
        const before = state.sliceDoc(range.from - size, range.from);
        const after = state.sliceDoc(range.to, range.to + size);
        if (before === mark && after === mark) {
          return {
            changes: [
              { from: range.from - size, to: range.from },
              { from: range.to, to: range.to + size },
            ],
            range: EditorSelection.range(range.from - size, range.to - size),
          };
        }
        return {
          changes: [
            { from: range.from, insert: mark },
            { from: range.to, insert: mark },
          ],
          range: EditorSelection.range(range.from + size, range.to + size),
        };
      }),
      { scrollIntoView: true, userEvent: "input" },
    );
    return true;
  };
}

export const toggleStrong = toggleMark("*");
export const toggleEmphasis = toggleMark("_");
export const toggleMonospace = toggleMark("`");

export function formattingKeymap(): Extension {
  return keymap.of([
    { key: "Mod-b", run: toggleStrong },
    { key: "Mod-i", run: toggleEmphasis },
    { key: "Mod-`", run: toggleMonospace },
  ]);
}
