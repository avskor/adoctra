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

const ADMONITION: Record<string, string> = {
  note: "NOTE",
  tip: "TIP",
  important: "IMPORTANT",
  warning: "WARNING",
  caution: "CAUTION",
};

/** A short label of a block's type, for a gutter or a breadcrumb. */
export function blockLabel(block: Block): string {
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
    case "list-item":
      return "•";
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
    if (block.kind === "table-cell") continue;
    if (start >= from && (found === null || block.depth < found.depth)) found = block;
  }
  return found;
}
