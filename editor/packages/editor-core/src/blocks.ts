// Queries over the flat block list of the editor view.
import type { Block } from "./types.js";

/** Where a block starts for the reader: its metadata lines, else itself. */
export function blockStart(block: Block): number {
  return block.meta_start ?? block.start;
}

/**
 * The blocks containing `pos`, outermost first. The list is in document
 * order with depths, so one pass keeps the chain of open ancestors.
 */
export function blockPath(blocks: readonly Block[], pos: number): Block[] {
  const path: Block[] = [];
  for (const block of blocks) {
    if (blockStart(block) > pos) break;
    if (pos <= block.end) {
      path.length = Math.min(path.length, block.depth);
      path.push(block);
    }
  }
  return path;
}

/** The innermost block at `pos`. */
export function blockAt(blocks: readonly Block[], pos: number): Block | null {
  const path = blockPath(blocks, pos);
  return path[path.length - 1] ?? null;
}

/**
 * The block at `pos` an action applies to: the innermost one, but the
 * container rather than the paragraph that is all of its content (a
 * `NOTE: text` line, a `[quote]` paragraph).
 */
export function actionBlock(blocks: readonly Block[], pos: number): Block | null {
  const path = blockPath(blocks, pos);
  let index = path.length - 1;
  // A delimited container ends after its closing line; one made of a
  // paragraph ends where the paragraph does.
  while (
    index > 0 &&
    path[index]!.kind === "paragraph" &&
    path[index - 1]!.end === path[index]!.end &&
    !STRUCTURAL.has(path[index - 1]!.kind)
  ) {
    index--;
  }
  return path[index] ?? null;
}

const STRUCTURAL = new Set(["section", "preamble", "list-item", "description", "table-cell"]);

const ADMONITION: Record<string, string> = {
  note: "NOTE",
  tip: "TIP",
  important: "IMPORTANT",
  warning: "WARNING",
  caution: "CAUTION",
};

/** The blocks `block` is nested in, outermost first. */
export function blockAncestors(blocks: readonly Block[], block: Block): Block[] {
  const open: Block[] = [];
  for (const other of blocks) {
    if (other === block) return open.slice(0, block.depth);
    open.length = Math.min(open.length, other.depth);
    open.push(other);
  }
  return [];
}

/**
 * A short label of a block's type, for a gutter or a breadcrumb. With its
 * `ancestors`, a list item shows its level: `•`, `••`; `1.`, `1.1`.
 */
export function blockLabel(block: Block, ancestors: readonly Block[] = []): string {
  switch (block.kind) {
    case "section":
    case "heading":
      return block.style === "discrete" || block.style === "float"
        ? "heading"
        : `H${(block.level ?? 0) + 1}`;
    case "paragraph":
      return "¶";
    case "listing":
      return block.language ?? (block.style === "source" ? "source" : "listing");
    case "admonition":
      return ADMONITION[block.admonition ?? ""] ?? "admonition";
    case "list":
      return block.ordered ? "ol" : "ul";
    case "dlist":
      return "dl";
    case "colist":
      return "callouts";
    case "list-item": {
      // (`blocks.map(blockLabel)` passes an index here.)
      const lists = (Array.isArray(ancestors) ? (ancestors as readonly Block[]) : []).filter(
        (item) => item.kind === "list" || item.kind === "colist",
      );
      const level = Math.max(lists.length, 1);
      const own = lists[lists.length - 1];
      if (own?.kind === "colist") return "<1>";
      if (own?.ordered) return level === 1 ? "1." : `1${".1".repeat(level - 1)}`;
      return "•".repeat(Math.min(level, 5));
    }
    case "description":
      return "dd";
    case "table-cell":
      return "cell";
    case "open":
      return block.style && block.style !== "open" ? block.style : "open";
    default:
      return block.kind;
  }
}

/** The symbol name of a block's type (`adoc-block-<name>` CSS classes). */
export function blockClass(block: Block): string {
  return block.kind === "admonition" && block.admonition
    ? `admonition adoc-block-${block.admonition}`
    : block.kind;
}

/**
 * The block a gutter labels on the line `from..to`: the outermost one that
 * starts there (a section rather than its heading, a list rather than its
 * first item).
 */
export function blockStartingIn(
  blocks: readonly Block[],
  from: number,
  to: number,
): Block | null {
  let found: Block | null = null;
  for (const block of blocks) {
    const start = blockStart(block);
    if (start > to) break;
    // A table's cells are not labelled: a row of them would only be noise.
    // The preamble is not either: it has no markup of its own, and its
    // first block is what the author sees on the line.
    if (block.kind === "table-cell" || block.kind === "preamble") continue;
    if (start >= from && (found === null || block.depth < found.depth)) found = block;
  }
  // A list is labelled by its items, each with its level.
  if (found?.kind === "list" || found?.kind === "colist") {
    const first = blocks[blocks.indexOf(found) + 1];
    if (first?.kind === "list-item" && first.start <= to) return first;
  }
  return found;
}
