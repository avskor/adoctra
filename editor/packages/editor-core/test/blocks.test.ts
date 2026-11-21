import { describe, expect, it } from "vitest";
import { actionBlock, blockAncestors, blockAt, blockLabel, blockPath, blockStart, blockStartingIn } from "../src/blocks.js";
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
    // A list is labelled by its items.
    expect(blockStartingIn(blocks, 42, 45)).toBe(blocks[5]);
    expect(blockStartingIn(blocks, 46, 50)).toBe(blocks[7]);
    expect(blockStartingIn(blocks, 5, 5)).toBeNull();
    const cell: Block = { start: 90, end: 95, depth: 1, kind: "table-cell" };
    expect(blockStartingIn([...blocks, cell], 90, 95)).toBeNull();
    const preamble: Block[] = [
      { start: 5, end: 9, depth: 0, kind: "preamble" },
      { start: 5, end: 9, depth: 1, kind: "paragraph" },
    ];
    expect(blockStartingIn(preamble, 5, 9)?.kind).toBe("paragraph");
  });

  it("takes a container made of one paragraph as the block to act on", () => {
    const note: Block[] = [
      { start: 0, end: 30, depth: 0, kind: "section", level: 1 },
      { start: 5, end: 12, depth: 1, kind: "admonition", admonition: "note" },
      { start: 11, end: 12, depth: 2, kind: "paragraph" },
      { start: 14, end: 30, depth: 1, kind: "example" },
      { start: 19, end: 25, depth: 2, kind: "paragraph" },
      { start: 27, end: 30, depth: 1, kind: "paragraph" },
    ];
    expect(actionBlock(note, 12)?.kind).toBe("admonition");
    expect(actionBlock(note, 20)?.kind).toBe("paragraph");
    // The last paragraph of a section is not the section.
    expect(actionBlock(note, 28)?.kind).toBe("paragraph");
    expect(actionBlock(blocks, 50)?.kind).toBe("list-item");
    expect(actionBlock([], 3)).toBeNull();
  });

  it("shows the level of a list item", () => {
    const label = (index: number) => blockLabel(blocks[index]!, blockAncestors(blocks, blocks[index]!));
    expect(blockAncestors(blocks, blocks[7]!).map((b) => b.kind)).toEqual([
      "section", "list", "list-item", "list",
    ]);
    expect(label(5)).toBe("•");
    // A numbered list inside a bulleted one: the second level.
    expect(label(7)).toBe("1.1");
    const bullets: Block[] = [
      { start: 0, end: 20, depth: 0, kind: "list", ordered: false },
      { start: 0, end: 20, depth: 1, kind: "list-item" },
      { start: 4, end: 20, depth: 2, kind: "list", ordered: false },
      { start: 4, end: 20, depth: 3, kind: "list-item" },
      { start: 9, end: 20, depth: 4, kind: "list", ordered: false },
      { start: 9, end: 20, depth: 5, kind: "list-item" },
    ];
    expect([1, 3, 5].map((i) => blockLabel(bullets[i]!, blockAncestors(bullets, bullets[i]!)))).toEqual([
      "•", "••", "•••",
    ]);
    expect(blockAncestors(bullets, { start: 0, end: 1, depth: 0, kind: "paragraph" })).toEqual([]);
  });

  it("names block types", () => {
    expect(blocks.map(blockLabel)).toEqual(["H2", "H2", "¶", "rust", "ul", "•", "ol", "•"]);
    expect(blockLabel({ start: 0, end: 1, depth: 0, kind: "admonition", admonition: "tip" })).toBe("TIP");
    expect(blockLabel({ start: 0, end: 1, depth: 0, kind: "table" })).toBe("table");
  });
});
