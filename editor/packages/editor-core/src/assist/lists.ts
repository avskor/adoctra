// List levels. In AsciiDoc the level of a list item is the length of its
// marker (`*`, `**`, `***`; `.`, `..`), not its indentation.
import { EditorSelection, type ChangeSpec, type EditorState } from "@codemirror/state";
import { blockAt } from "../blocks.js";
import { parsed } from "../state.js";
import type { CommandTarget } from "./commands.js";

/** The deepest level a marker can express. */
export const MAX_LIST_LEVEL = 5;

export interface ListLine {
  indent: string;
  /** `*`…, `.`… or `-`. */
  marker: string;
  /** The checkbox of a checklist item with the space after it, or `""`. */
  checkbox: string;
  text: string;
  /** The length of everything before the text. */
  prefix: number;
}

const ITEM = /^([ \t]*)(\*+|\.+|-)([ \t]+)(\[[ x*]\][ \t]+|\[[ x*]\]$)?(.*)$/;

/** A list item's line taken apart, or `null` for any other line. */
export function listLine(text: string): ListLine | null {
  const match = ITEM.exec(text);
  if (!match) return null;
  const [, indent, marker, space, checkbox = "", rest] = match as unknown as string[];
  return {
    indent: indent!,
    marker: marker!,
    checkbox,
    text: rest!,
    prefix: indent!.length + marker!.length + space!.length + checkbox.length,
  };
}

const VERBATIM = new Set(["listing", "literal", "pass", "stem"]);

/** The list item on the line at `pos`, unless the line is verbatim text. */
export function listItemAt(state: EditorState, pos: number): ListLine | null {
  const item = listLine(state.doc.lineAt(pos).text);
  if (!item) return null;
  const around = blockAt(parsed(state).blocks, pos);
  return around && VERBATIM.has(around.kind) ? null : item;
}

/** The marker one level deeper (`1`) or higher (`-1`); `null` when there is none. */
export function shiftMarker(marker: string, delta: 1 | -1): string | null {
  // A hyphen has no levels: deeper, it becomes asterisks.
  if (marker === "-") return delta > 0 ? "**" : null;
  const length = marker.length + delta;
  return length < 1 || length > MAX_LIST_LEVEL ? null : marker[0]!.repeat(length);
}

/**
 * Move the selected list items one level deeper or higher. `false` when no
 * selected line is a list item, so a key falls through to its usual meaning.
 */
export function changeListLevel(delta: 1 | -1) {
  return ({ state, dispatch }: CommandTarget): boolean => {
    const { from, to } = state.selection.main;
    const first = state.doc.lineAt(from).number;
    const last = state.doc.lineAt(to).number;
    const changes: ChangeSpec[] = [];
    let items = 0;
    for (let number = first; number <= last; number++) {
      const line = state.doc.line(number);
      const item = listItemAt(state, line.from);
      if (!item) continue;
      items++;
      const marker = shiftMarker(item.marker, delta);
      if (marker === null) continue;
      const at = line.from + item.indent.length;
      changes.push({ from: at, to: at + item.marker.length, insert: marker });
    }
    if (items === 0) return false;
    if (changes.length > 0) {
      const changeSet = state.changes(changes);
      dispatch(
        state.update({
          changes: changeSet,
          selection: EditorSelection.range(changeSet.mapPos(from, 1), changeSet.mapPos(to, 1)),
          scrollIntoView: true,
          userEvent: delta > 0 ? "input.indent" : "delete.dedent",
        }),
      );
    }
    return true;
  };
}

/**
 * Enter at the end of a list item: start the next item of the same level,
 * or, on an item without text, go one level up and at the first level end
 * the list with a blank line.
 */
export function continueList({ state, dispatch }: CommandTarget): boolean {
  const { main } = state.selection;
  if (!main.empty || state.selection.ranges.length > 1) return false;
  const line = state.doc.lineAt(main.head);
  if (main.head !== line.to) return false;
  const item = listItemAt(state, main.head);
  if (!item) return false;
  const box = item.checkbox ? "[ ] " : "";
  if (item.text.trim() !== "") {
    const next = `\n${item.indent}${item.marker} ${box}`;
    dispatch(
      state.update({
        changes: { from: line.to, insert: next },
        selection: { anchor: line.to + next.length },
        scrollIntoView: true,
        userEvent: "input",
      }),
    );
    return true;
  }
  const marker = shiftMarker(item.marker, -1);
  // Out of the list: a blank line ends it, or the next line would still
  // belong to the last item.
  const replacement = marker === null ? "\n" : `${item.indent}${marker} ${box}`;
  dispatch(
    state.update({
      changes: { from: line.from, to: line.to, insert: replacement },
      selection: { anchor: line.from + replacement.length },
      scrollIntoView: true,
      userEvent: "input",
    }),
  );
  return true;
}

/**
 * Start a list of the other kind (numbered under bullets, bullets under
 * numbers) nested in the item at the cursor. A marker no enclosing list
 * uses nests, whatever its length; one level longer than the item's is
 * never an enclosing list's.
 */
export function nestOtherList({ state, dispatch }: CommandTarget): boolean {
  const { head } = state.selection.main;
  const item = listItemAt(state, head);
  if (!item) return false;
  const line = state.doc.lineAt(head);
  const other = item.marker[0] === "." ? "*" : ".";
  const length = Math.min((item.marker === "-" ? 1 : item.marker.length) + 1, MAX_LIST_LEVEL);
  const next = `\n${item.indent}${other.repeat(length)} `;
  dispatch(
    state.update({
      changes: { from: line.to, insert: next },
      selection: { anchor: line.to + next.length },
      scrollIntoView: true,
      userEvent: "input",
    }),
  );
  return true;
}
