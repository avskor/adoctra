// Changes to an existing block: its style, its metadata lines, its place.
import type { ChangeSpec } from "@codemirror/state";
import { blockStart } from "../blocks.js";
import { parsed } from "../state.js";
import type { Block } from "../types.js";
import type { CommandTarget } from "./commands.js";

const LABEL = /^(NOTE|TIP|IMPORTANT|WARNING|CAUTION):[ \t]+/;
const ATTRIBUTE_LINE = /^\[(?!\[)(.*)\]\s*$/;

function change(target: CommandTarget, changes: ChangeSpec, select?: { anchor: number; head?: number }) {
  target.dispatch(
    target.state.update({
      changes,
      ...(select ? { selection: select } : {}),
      scrollIntoView: true,
      userEvent: "input",
    }),
  );
  return true;
}

/** The block's attribute list line (`[…]`, not an anchor), if it has one. */
function attributeLine(target: CommandTarget, block: Block) {
  const { doc } = target.state;
  if (block.meta_start === undefined) return null;
  const last = doc.lineAt(block.start).number - 1;
  for (let number = doc.lineAt(block.meta_start).number; number <= last; number++) {
    const line = doc.line(number);
    const match = ATTRIBUTE_LINE.exec(line.text);
    if (match) return { line, content: match[1]! };
  }
  return null;
}

/**
 * Set the block's style (the first positional attribute), keeping the rest
 * of its attribute list; `null` removes the style.
 */
export function setStyle(target: CommandTarget, block: Block, style: string | null): boolean {
  const found = attributeLine(target, block);
  if (!found) {
    return style === null
      ? true
      : change(target, { from: block.start, insert: `[${style}]\n` });
  }
  const { line, content } = found;
  const comma = content.indexOf(",");
  const first = comma < 0 ? content : content.slice(0, comma);
  const rest = comma < 0 ? "" : content.slice(comma);
  let next: string;
  if (first.includes("=")) {
    // A named attribute comes first: the style goes before it.
    next = style === null ? content : `${style},${content}`;
  } else {
    // `style#id.role%option`: replace the word, keep the shorthand.
    const shorthand = /^[^#.%]*(.*)$/.exec(first)![1]!;
    next = `${style ?? ""}${shorthand}${rest}`.replace(/^,\s*/, "");
  }
  return next === ""
    ? change(target, { from: line.from, to: Math.min(line.to + 1, target.state.doc.length) })
    : change(target, { from: line.from, to: line.to, insert: `[${next}]` });
}

/**
 * Make the block an admonition of `kind` (`NOTE`, …) or, with `null`, a
 * plain block again. A `NOTE: text` paragraph changes its label.
 */
export function setAdmonition(target: CommandTarget, block: Block, kind: string | null): boolean {
  const { doc } = target.state;
  const line = doc.lineAt(block.start);
  const label = LABEL.exec(line.text);
  if (label) {
    return change(target, {
      from: line.from,
      to: line.from + label[0].length,
      insert: kind === null ? "" : `${kind}: `,
    });
  }
  if (kind !== null && block.kind === "paragraph" && block.meta_start === undefined) {
    return change(target, { from: block.start, insert: `${kind}: ` });
  }
  return setStyle(target, block, kind);
}

/** Put the block between `delimiter` lines, with an optional style line. */
export function wrapBlock(target: CommandTarget, block: Block, delimiter: string, style?: string) {
  const end = target.state.doc.lineAt(Math.min(block.end, target.state.doc.length)).to;
  return change(target, [
    { from: block.start, insert: `${style ? `[${style}]\n` : ""}${delimiter}\n` },
    { from: end, insert: `\n${delimiter}` },
  ]);
}

/** Add a metadata line above the block and select its placeholder. */
export function addMetaLine(target: CommandTarget, block: Block, before: string, placeholder: string, after: string) {
  const at = blockStart(block);
  return change(
    target,
    { from: at, insert: `${before}${placeholder}${after}\n` },
    { anchor: at + before.length, head: at + before.length + placeholder.length },
  );
}

/** Add a role to the block's attribute list, or a `[.role]` line. */
export function addRole(target: CommandTarget, block: Block, role = "role"): boolean {
  const found = attributeLine(target, block);
  if (!found) return addMetaLine(target, block, "[.", role, "]");
  const { line, content } = found;
  const comma = content.indexOf(",");
  const at = line.from + 1 + (comma < 0 ? content.length : comma);
  return change(target, { from: at, insert: `.${role}` }, { anchor: at + 1, head: at + 1 + role.length });
}

function whole(target: CommandTarget, block: Block) {
  const { doc } = target.state;
  return {
    from: doc.lineAt(blockStart(block)).from,
    to: doc.lineAt(Math.min(block.end, doc.length)).to,
  };
}

export function selectBlock(target: CommandTarget, block: Block): boolean {
  const { from, to } = whole(target, block);
  target.dispatch(target.state.update({ selection: { anchor: from, head: to }, scrollIntoView: true }));
  return true;
}

export function deleteBlock(target: CommandTarget, block: Block): boolean {
  const { doc } = target.state;
  const { from, to } = whole(target, block);
  // Take the blank lines after it too (or before it, at the end).
  let end = to;
  while (end < doc.length && doc.lineAt(end + 1).text.trim() === "") end = doc.lineAt(end + 1).to;
  if (end < doc.length) end += 1;
  let start = from;
  if (end === doc.length) {
    while (start > 0 && doc.lineAt(start - 1).text.trim() === "") start = doc.lineAt(start - 1).from;
    if (start > 0) start -= 1;
  }
  return change(target, { from: start, to: end }, { anchor: start });
}

export function duplicateBlock(target: CommandTarget, block: Block): boolean {
  const { from, to } = whole(target, block);
  const text = target.state.sliceDoc(from, to);
  return change(target, { from: to, insert: `\n\n${text}` }, { anchor: to + 2 });
}

/** The block's sibling before (`-1`) or after (`1`) it under the same parent. */
function sibling(blocks: readonly Block[], block: Block, direction: -1 | 1): Block | null {
  const index = blocks.findIndex(
    (other) => other.start === block.start && other.end === block.end && other.depth === block.depth,
  );
  if (index < 0) return null;
  for (let at = index + direction; at >= 0 && at < blocks.length; at += direction) {
    const other = blocks[at]!;
    if (other.depth < block.depth) return null;
    if (other.depth === block.depth) return other;
  }
  return null;
}

/** Swap the block with its neighbour. */
export function moveBlock(target: CommandTarget, block: Block, direction: -1 | 1): boolean {
  const other = sibling(parsed(target.state).blocks, block, direction);
  if (!other) return false;
  const [first, second] = direction < 0 ? [other, block] : [block, other];
  const a = whole(target, first);
  const b = whole(target, second);
  if (a.to >= b.from) return false;
  const textA = target.state.sliceDoc(a.from, a.to);
  const textB = target.state.sliceDoc(b.from, b.to);
  const between = target.state.sliceDoc(a.to, b.from);
  const cursor = direction < 0 ? a.from : a.from + textB.length + between.length;
  return change(target, { from: a.from, to: b.to, insert: textB + between + textA }, { anchor: cursor });
}
