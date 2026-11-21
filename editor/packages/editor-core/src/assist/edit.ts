// The edits commands are made of: marks around a selection, line prefixes,
// templates with fields.
import { snippet } from "@codemirror/autocomplete";
import { EditorSelection, type ChangeSpec } from "@codemirror/state";
import { blockPath } from "../blocks.js";
import { parsed } from "../state.js";
import type { Block } from "../types.js";
import type { CommandTarget } from "./commands.js";

/**
 * Toggle `mark` (`*`, `_`, `` ` ``, `#`, `^`, `~`) around each selection
 * range. Without a selection, and outside the marks, it writes the marks
 * around a placeholder to type over.
 */
export function toggleMark(mark: string) {
  return (target: CommandTarget): boolean => {
    const { state, dispatch } = target;
    const { main } = state.selection;
    if (
      state.selection.ranges.length === 1 &&
      main.empty &&
      !(
        state.sliceDoc(main.from - mark.length, main.from) === mark &&
        state.sliceDoc(main.to, main.to + mark.length) === mark
      )
    ) {
      snippet(`${mark}\${1:text}${mark}`)(target, null, main.from, main.to);
      return true;
    }
    dispatch(
      state.update(
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
      ),
    );
    return true;
  };
}

/**
 * Toggle a line prefix (`== `, `* `, `. `) on the selected lines: a line
 * that has it loses it, any other prefix matching `existing` is replaced.
 */
export function toggleLinePrefix(prefix: string, existing: RegExp) {
  return ({ state, dispatch }: CommandTarget): boolean => {
    const { from, to } = state.selection.main;
    const first = state.doc.lineAt(from).number;
    const last = state.doc.lineAt(to).number;
    const lines = [];
    for (let number = first; number <= last; number++) {
      const line = state.doc.line(number);
      if (line.text.trim() !== "" || first === last) lines.push(line);
    }
    const remove = lines.every((line) => line.text.startsWith(prefix));
    const changes: ChangeSpec[] = lines.map((line) => {
      const current = existing.exec(line.text)?.[0].length ?? 0;
      return { from: line.from, to: line.from + current, insert: remove ? "" : prefix };
    });
    const changeSet = state.changes(changes);
    dispatch(
      state.update({
        changes: changeSet,
        // The cursor of an empty line goes after the new prefix.
        selection: EditorSelection.range(changeSet.mapPos(from, 1), changeSet.mapPos(to, 1)),
        scrollIntoView: true,
        userEvent: "input",
      }),
    );
    return true;
  };
}

const FIELD = /[$#]\{(\d+)(?::(@?)([^{}]*))?\}/g;

/**
 * A template in CodeMirror's snippet syntax (`${1:placeholder}`), where a
 * field written `${1:@placeholder}` takes the selected text. A selection
 * the snippet syntax cannot carry is written without fields.
 */
export function fillTemplate(template: string, selected: string): { text: string; fields: boolean } {
  // A field holds one line of text without the syntax's own characters.
  const safe = !/[${}#\\\n]/.test(selected);
  if (selected === "" || safe) {
    return {
      text: template.replace(FIELD, (_all, index: string, takes: string, placeholder = "") =>
        `\${${index}:${takes && selected ? selected : placeholder}}`,
      ),
      fields: true,
    };
  }
  return {
    text: template.replace(FIELD, (_all, _index, takes: string, placeholder = "") =>
      takes ? selected : placeholder,
    ),
    fields: false,
  };
}

function apply(target: CommandTarget, text: string, fields: boolean, from: number, to: number): void {
  if (fields) {
    snippet(text)(target, null, from, to);
  } else {
    target.dispatch(
      target.state.update({
        changes: { from, to, insert: text },
        selection: { anchor: from + text.length },
        scrollIntoView: true,
        userEvent: "input",
      }),
    );
  }
}

/**
 * Insert an inline template at the selection (which fills its `@` field);
 * with `after`, behind the selection, which stays.
 */
export function insertInline(template: string, after = false) {
  return (target: CommandTarget): boolean => {
    const { from, to } = target.state.selection.main;
    const { text, fields } = fillTemplate(template, after ? "" : target.state.sliceDoc(from, to));
    apply(target, text, fields, after ? to : from, to);
    return true;
  };
}

/**
 * Insert a block template on lines of its own, a blank line apart from its
 * neighbours: in place of an empty line, in place of the selected lines
 * (which fill its `@` field), or after the block the cursor is in.
 */
export function insertBlock(template: string | ((selected: string) => string)) {
  return (target: CommandTarget): boolean => {
    const { state } = target;
    const { doc } = state;
    const selection = state.selection.main;
    let from: number;
    let to: number;
    let selected = "";
    const source = (text: string) => (typeof template === "function" ? template(text) : template);
    // Only a template with a field for it takes the selected lines.
    if (!selection.empty && /[$#]\{\d+:@/.test(source(""))) {
      from = doc.lineAt(selection.from).from;
      to = doc.lineAt(selection.to).to;
      selected = doc.sliceString(from, to);
    } else {
      const line = doc.lineAt(selection.to);
      if (selection.empty && line.text.trim() === "") {
        from = line.from;
        to = line.to;
      } else {
        const block = blockToInsertAfter(blockPath(parsed(state).blocks, selection.to));
        const end = Math.min(Math.max(block?.end ?? line.to, line.to), doc.length);
        from = to = doc.lineAt(end).to;
      }
    }
    const blank = (number: number) =>
      number < 1 || number > doc.lines || doc.line(number).text.trim() === "";
    const startLine = doc.lineAt(from);
    const endLine = doc.lineAt(to);
    // After a block: a blank line first. On an emptied line: only if the
    // line above has text.
    const before =
      from === to && startLine.text.trim() !== ""
        ? "\n\n"
        : blank(startLine.number - 1)
          ? ""
          : "\n";
    const after = blank(endLine.number + 1) ? "" : "\n";
    const { text, fields } = fillTemplate(source(selected), selected);
    apply(target, before + text + after, fields, from, to);
    return true;
  };
}

/**
 * The block a new block goes after: the one around the cursor, but the
 * whole table rather than a cell and the whole list rather than an item.
 */
function blockToInsertAfter(path: readonly Block[]): Block | undefined {
  const inside = new Set(["table-cell", "list-item", "description"]);
  const lists = new Set(["list", "dlist", "colist"]);
  let index = path.length - 1;
  while (index > 0) {
    const block = path[index]!;
    if (inside.has(block.kind) || (lists.has(block.kind) && inside.has(path[index - 1]!.kind))) index--;
    else break;
  }
  return path[index];
}

/** A table of `rows` body rows and `columns` columns, with a header row. */
export function tableTemplate(rows: number, columns: number): string {
  let field = 1;
  const row = (text: string) =>
    Array.from({ length: columns }, (_, index) => `|\${${field++}:${text} ${index + 1}}`).join(" ");
  const cols = Array.from({ length: columns }, () => "1").join(",");
  const body = Array.from({ length: rows }, () => row("Cell")).join("\n");
  return `[cols="${cols}"]\n|===\n${row("Header")}\n\n${body}\n|===`;
}
