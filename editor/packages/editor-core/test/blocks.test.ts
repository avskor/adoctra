import { describe, expect, it } from "vitest";
import { blockAt, blockLabel, blockPath, blockStart, blockStartingIn } from "../src/blocks.js";
import type { Block } from "../src/types.js";

// == S / para / [source,rust] ---- / * a ** b
const blocks: Block[] = [
  { start: 0, end: 80, depth: 0, kind: "section", level: 1 },
  { start: 0, end: 4, depth: 1, kind: "heading", level: 1 },
  { start: 6, end: 10, depth: 1, kind: "paragraph" },
  { start: 26, end: 40, depth: 1, kind: "listing", meta_start: 12, style: "source", language: "rust" },
  { start: 42, end: 80, depth: 1, kind: "list", ordered: false },
  { start: 42, end: 80, depth: 2, kind: "list-item" },
  { start: 46, end: 80, depth: 3, kind: "list", ordered: true },
  { start: 46, end: 80, depth: 4, kind: "list-item" },
];

describe("block queries", () => {
  it("finds the chain of blocks around an offset", () => {
    expect(blockPath(blocks, 7).map((b) => b.kind)).toEqual(["section", "paragraph"]);
    expect(blockPath(blocks, 50).map((b) => b.kind)).toEqual([
      "section", "list", "list-item", "list", "list-item",
    ]);
    // Between blocks only the section holds the offset.
    expect(blockPath(blocks, 11).map((b) => b.kind)).toEqual(["section"]);
    expect(blockPath(blocks, 200)).toEqual([]);
    expect(blockAt([], 0)).toBeNull();
  });

  it("counts metadata lines as part of the block", () => {
    expect(blockAt(blocks, 15)?.kind).toBe("listing");
    expect(blockStart(blocks[3]!)).toBe(12);
    expect(blockStartingIn(blocks, 12, 25)?.kind).toBe("listing");
    expect(blockStartingIn(blocks, 26, 30)).toBeNull();
  });

  it("labels a line with the outermost block starting on it", () => {
    expect(blockStartingIn(blocks, 0, 4)?.kind).toBe("section");
    expect(blockStartingIn(blocks, 42, 45)?.kind).toBe("list");
    expect(blockStartingIn(blocks, 46, 50)?.kind).toBe("list");
    expect(blockStartingIn(blocks, 5, 5)).toBeNull();
    const cell: Block = { start: 90, end: 95, depth: 1, kind: "table-cell" };
    expect(blockStartingIn([...blocks, cell], 90, 95)).toBeNull();
  });

  it("names block types", () => {
    expect(blocks.map(blockLabel)).toEqual(["H2", "H2", "¶", "rust", "ul", "•", "ol", "•"]);
    expect(blockLabel({ start: 0, end: 1, depth: 0, kind: "admonition", admonition: "tip" })).toBe("TIP");
    expect(blockLabel({ start: 0, end: 1, depth: 0, kind: "table" })).toBe("table");
  });
});
