// The assist commands, checked against the real parser: what a command
// writes must parse as the element it names, without diagnostics.
import { EditorSelection, EditorState } from "@codemirror/state";
import { readFileSync } from "node:fs";
import { beforeAll, describe, expect, it } from "vitest";
import * as wasm from "../../wasm/pkg/adoctra_wasm.js";
import {
  blockActions,
  defaultCommands,
  defaultToolbar,
  describeToken,
  en,
  fillTemplate,
  ru,
  searchCommands,
  slashQuery,
  changeListLevel,
  continueList,
  listLine,
  shiftMarker,
  tableTemplate,
  type AssistCommand,
} from "../src/assist/index.js";
import { activeCommands } from "../src/assist/commands.js";
import { actionBlock } from "../src/blocks.js";
import { parsedField, setParsed } from "../src/state.js";
import type { EditorViewData } from "../src/types.js";

beforeAll(() => {
  wasm.initSync({
    module: readFileSync(new URL("../../wasm/pkg/adoctra_wasm_bg.wasm", import.meta.url)),
  });
});

const parse = (text: string) => JSON.parse(wasm.editor_json(text)) as EditorViewData;
const commands = defaultCommands();
const command = (id: string) => commands.find((item) => item.id === id)!;

/** A state holding `doc` and its parse; `[` and `]` mark the selection. */
function stateOf(marked: string): EditorState {
  const anchor = marked.indexOf("[[");
  const head = marked.indexOf("]]") - 2;
  const doc = anchor < 0 ? marked : marked.replace("[[", "").replace("]]", "");
  const state = EditorState.create({
    doc,
    selection: anchor < 0 ? EditorSelection.cursor(doc.length) : EditorSelection.range(anchor, head),
    extensions: [parsedField],
  });
  return state.update({ effects: setParsed.of(parse(doc)) }).state;
}

/** Run a command on a state (on the block under the cursor, for an action). */
function run(item: AssistCommand, state: EditorState): EditorState {
  let next = state;
  const target = {
    get state() {
      return next;
    },
    dispatch: (transaction: { state: EditorState }) => void (next = transaction.state),
  };
  const block = actionBlock(state.field(parsedField).blocks, state.selection.main.head) ?? undefined;
  expect(item.run(target, block), item.id).toBe(true);
  return next;
}

const text = (state: EditorState) => state.doc.toString();

// What each inserting command must produce: a block kind or a token kind.
const EXPECT: Record<string, { kind?: string; token?: string; setup?: string; diagnostics?: true }> = {
  strong: { token: "strong" },
  emphasis: { token: "emphasis" },
  monospace: { token: "monospace" },
  mark: { token: "mark" },
  superscript: { token: "superscript" },
  subscript: { token: "subscript" },
  link: { token: "link" },
  "inline-image": { token: "inline-macro" },
  footnote: { token: "inline-macro" },
  "attribute-ref": { token: "attribute-ref", setup: ":attribute: v\n:chosen: v\n\n" },
  kbd: { token: "inline-macro", setup: ":experimental:\n\n" },
  btn: { token: "inline-macro", setup: ":experimental:\n\n" },
  menu: { token: "inline-macro", setup: ":experimental:\n\n" },
  passthrough: { kind: "paragraph" },
  "line-break": { token: "line-break" },
  title: { token: "heading" },
  "heading-1": { kind: "section" },
  "heading-2": { kind: "section", diagnostics: true },
  "heading-3": { kind: "section", diagnostics: true },
  "heading-4": { kind: "section", diagnostics: true },
  "heading-5": { kind: "section", diagnostics: true },
  ulist: { kind: "list" },
  olist: { kind: "list" },
  checklist: { kind: "list" },
  dlist: { kind: "dlist" },
  source: { kind: "listing" },
  listing: { kind: "listing" },
  literal: { kind: "literal" },
  note: { kind: "admonition" },
  tip: { kind: "admonition" },
  important: { kind: "admonition" },
  warning: { kind: "admonition" },
  caution: { kind: "admonition" },
  quote: { kind: "quote" },
  verse: { kind: "verse" },
  example: { kind: "example" },
  sidebar: { kind: "sidebar" },
  open: { kind: "open" },
  collapsible: { kind: "example" },
  table: { kind: "table" },
  stem: { kind: "stem" },
  pass: { kind: "pass" },
  image: { kind: "image" },
  video: { kind: "video" },
  // Without a workspace nothing is included; the command only has to run.
  include: { diagnostics: true },
  toc: { token: "block-macro" },
  attribute: { token: "attribute-entry" },
  comment: { token: "comment" },
  "thematic-break": { kind: "break" },
  "page-break": { kind: "break" },
};

describe("inserting commands", () => {
  // Commands on what is already there have tests of their own.
  const inserting = commands.filter(
    (item) => item.group !== "blockAction" && item.id !== "xref" && !item.id.startsWith("list-"),
  );

  it("has an expectation for every command", () => {
    expect(inserting.map((item) => item.id).sort()).toEqual(Object.keys(EXPECT).sort());
  });

  for (const item of inserting) {
    it(`${item.id} writes markup the parser reads as it`, () => {
      const expected = EXPECT[item.id]!;
      // Text goes into a line; a line prefix is put before words; a block
      // goes on an empty line, after a paragraph or around selected text.
      const starts =
        item.id === "line-break"
          ? ["Before", "[[chosen]]"]
          : item.group === "inline"
            ? ["", "Before ", "[[chosen]]"]
            : item.group === "heading" || ["ulist", "olist", "checklist"].includes(item.id)
              ? ["Words[[]]", "[[chosen]]"]
              : ["", "Before.\n\n", "Before.", "[[chosen]]"];
      for (const start of starts) {
        const result = text(run(item, stateOf((expected.setup ?? "") + start)));
        const view = parse(result);
        if (!expected.diagnostics) expect(view.diagnostics, `${item.id}: ${result}`).toEqual([]);
        if (expected.kind) {
          expect(view.blocks.map((block) => block.kind), `${item.id}: ${result}`).toContain(expected.kind);
        }
        if (expected.token) {
          expect(view.tokens.map((token) => token[2]), `${item.id}: ${result}`).toContain(expected.token);
        }
        if (start === "[[chosen]]") expect(result, item.id).toContain("chosen");
      }
    });
  }

  it("keeps a block apart from its neighbours", () => {
    expect(text(run(command("listing"), stateOf("One.\nTwo.[[]]\n\nThree.")))).toBe(
      "One.\nTwo.\n\n----\ntext\n----\n\nThree.",
    );
    expect(text(run(command("listing"), stateOf("One.\n[[]]\nThree.")))).toBe(
      "One.\n\n----\ntext\n----\n\nThree.",
    );
    expect(text(run(command("source"), stateOf("[[let x;\nlet y;]]")))).toBe(
      "[source,language]\n----\nlet x;\nlet y;\n----",
    );
  });

  it("goes after the whole table or list the cursor is in", () => {
    const rule = command("thematic-break");
    expect(text(run(rule, stateOf("|===\n|a[[]] |b\n\n|c |d\n|===\n\nEnd.")))).toBe(
      "|===\n|a |b\n\n|c |d\n|===\n\n'''\n\nEnd.",
    );
    expect(text(run(rule, stateOf("* a\n** b[[]]\n* c")))).toBe("* a\n** b\n* c\n\n'''");
    // Inside a container it stays inside.
    expect(text(run(rule, stateOf("====\nin[[]]\n====")))).toBe("====\nin\n\n'''\n\n====");
  });

  it("inserts a cross reference", () => {
    const state = run(command("xref"), stateOf("See [[]]."));
    expect(text(state)).toBe("See <<>>.");
    expect(state.selection.main.head).toBe(6);
  });

  it("writes text the snippet syntax cannot carry without fields", () => {
    expect(fillTemplate("a ${1:@x} b", "{n} $1")).toEqual({ text: "a {n} $1 b", fields: false });
    expect(fillTemplate("a ${1:@x} ${2:y}", "sel")).toEqual({ text: "a ${1:sel} ${2:y}", fields: true });
    expect(text(run(command("note"), stateOf("[[a {b} c]]")))).toBe("NOTE: a {b} c");
  });

  it("builds a table of any size the parser accepts", () => {
    const view = parse(tableTemplate(3, 4).replace(/\$\{\d+:([^}]*)\}/g, "$1"));
    expect(view.diagnostics).toEqual([]);
    expect(view.blocks.filter((block) => block.kind === "table-cell")).toHaveLength(16);
  });
});

describe("toggles", () => {
  it("wraps and unwraps a selection", () => {
    const bold = run(command("strong"), stateOf("a [[b]] c"));
    expect(text(bold)).toBe("a *b* c");
    expect(text(run(command("strong"), bold))).toBe("a b c");
  });

  it("sets, changes and removes a line prefix", () => {
    const section = run(command("heading-1"), stateOf("Title[[]]"));
    expect(text(section)).toBe("== Title");
    const deeper = run(command("heading-2"), section);
    expect(text(deeper)).toBe("=== Title");
    expect(text(run(command("heading-2"), deeper))).toBe("Title");
    expect(text(run(command("olist"), stateOf("[[* a\n* b\n\n* [ ] c]]")))).toBe(". a\n. b\n\n. c");
  });

  it("reports the commands the cursor is in", () => {
    const active = (marked: string) => activeCommands(commands, stateOf(marked));
    expect(active("== S\n\na *b[[]]old* c")).toContain("strong");
    expect(active("== S[[]]\n\ntext")).toContain("heading-1");
    expect(active("[source,sh]\n----\nx[[]]\n----")).toEqual(["source"]);
    expect(active("* item[[]]")).toContain("ulist");
    expect(active("|===\n|a[[]]\n|===")).toContain("table");
    expect(active("TIP: t[[]]")).toEqual(["tip"]);
  });
});

describe("list levels", () => {
  const act = (id: string, marked: string) => text(run(command(id), stateOf(marked)));
  const depths = (source: string) =>
    parse(source)
      .blocks.filter((block) => block.kind === "list-item")
      .map((block) => block.depth);

  it("moves an item a level deeper and back by the length of its marker", () => {
    const nested = act("list-indent", "* a\n* b[[]]\n* c");
    expect(nested).toBe("* a\n** b\n* c");
    // The parser nests the item: two levels of depth per list.
    expect(depths("* a\n* b\n* c")).toEqual([1, 1, 1]);
    expect(depths(nested)).toEqual([1, 3, 1]);
    expect(parse(nested).diagnostics).toEqual([]);
    expect(act("list-outdent", "* a\n** b[[]]\n* c")).toBe("* a\n* b\n* c");
    expect(act("list-indent", ". a\n. b[[]]")).toBe(". a\n.. b");
    expect(depths(". a\n.. b")).toEqual([1, 3]);
  });

  it("moves every selected item and keeps checkboxes", () => {
    expect(act("list-indent", "* a\n[[* [ ] b\n\ntext\n* [x] c]]")).toBe("* a\n** [ ] b\n\ntext\n** [x] c");
    expect(act("list-outdent", "[[*** a\n** b\n* c]]")).toBe("** a\n* b\n* c");
  });

  it("stops at the first and the last level, and turns a hyphen into asterisks", () => {
    expect(act("list-outdent", "* a[[]]")).toBe("* a");
    expect(act("list-indent", "***** a[[]]")).toBe("***** a");
    expect(act("list-indent", "- a\n- b[[]]")).toBe("- a\n** b");
    expect(depths("- a\n** b")).toEqual([1, 3]);
    expect(shiftMarker("-", -1)).toBeNull();
    expect(shiftMarker("..", 1)).toBe("...");
  });

  it("leaves other lines to the key's usual meaning", () => {
    const state = stateOf("plain[[]]");
    let touched = false;
    const target = { state, dispatch: () => void (touched = true) };
    expect(changeListLevel(1)(target)).toBe(false);
    expect(continueList(target)).toBe(false);
    // A line that only looks like an item, inside a listing.
    const listing = stateOf("----\n* x[[]]\n----");
    expect(changeListLevel(1)({ state: listing, dispatch: () => void (touched = true) })).toBe(false);
    expect(continueList({ state: listing, dispatch: () => void (touched = true) })).toBe(false);
    expect(touched).toBe(false);
    // A block title and a literal fence are not items.
    expect(listLine(".Title")).toBeNull();
    expect(listLine("....")).toBeNull();
    expect(listLine("--")).toBeNull();
    expect(listLine("  ** [x] done")).toEqual({ indent: "  ", marker: "**", checkbox: "[x] ", text: "done", prefix: 9 });
  });

  it("continues a list on Enter and leaves it on an empty item", () => {
    const enter = (marked: string) => {
      let state = stateOf(marked);
      expect(continueList({ state, dispatch: (tr) => void (state = tr.state) })).toBe(true);
      return `${state.sliceDoc(0, state.selection.main.head)}|${state.sliceDoc(state.selection.main.head)}`;
    };
    expect(enter("* a\n** b[[]]")).toBe("* a\n** b\n** |");
    expect(enter(". a[[]]\n. z")).toBe(". a\n. |\n. z");
    expect(enter("* [x] done[[]]")).toBe("* [x] done\n* [ ] |");
    // An item without text goes a level up, then ends the list: what is
    // typed next is a paragraph, not more of the last item.
    expect(enter("* a\n** [[]]")).toBe("* a\n* |");
    expect(enter("* a\n* [[]]")).toBe("* a\n\n|");
    expect(enter("* a\n- [[]]")).toBe("* a\n\n|");
    expect(parse("* a\n\nafter").blocks.map((block) => [block.depth, block.kind])).toEqual([
      [0, "list"],
      [1, "list-item"],
      [0, "paragraph"],
    ]);
    // In the middle of an item Enter breaks the line as usual.
    const middle = stateOf("* ab[[]]cd");
    expect(continueList({ state: middle, dispatch: () => {} })).toBe(false);
  });

  it("nests a list of the other kind under an item", () => {
    const mixed = act("list-nest-other", "* a[[]]\n* z");
    expect(mixed).toBe("* a\n.. \n* z");
    const view = parse("* a\n.. one\n.. two\n* z");
    expect(view.diagnostics).toEqual([]);
    expect(view.blocks.filter((block) => block.kind === "list").map((block) => [block.depth, block.ordered])).toEqual([
      [0, false],
      [2, true],
    ]);
    // Under a numbered item nested in bullets, asterisks must not return
    // to the outer list.
    expect(act("list-nest-other", "* a\n. b[[]]")).toBe("* a\n. b\n** ");
    expect(depths("* a\n. b\n** c")).toEqual([1, 3, 5]);
  });

  it("offers a list item its level actions", () => {
    expect(act("item-indent", "* a\n* b[[]]")).toBe("* a\n** b");
    expect(act("item-outdent", "* a\n** b[[]]")).toBe("* a\n* b");
  });
});

describe("block actions", () => {
  const act = (id: string, marked: string) => text(run(command(id), stateOf(marked)));

  it("turns a paragraph into an admonition and back", () => {
    expect(act("to-note", "Text[[]] here.")).toBe("NOTE: Text here.");
    expect(act("to-tip", "NOTE: Text[[]].")).toBe("TIP: Text.");
    expect(act("to-paragraph", "NOTE: Text[[]].")).toBe("Text.");
    // A paragraph with metadata lines takes a style instead of a label.
    expect(act("to-warning", ".T\n[.r]\nText[[]].")).toBe(".T\n[WARNING.r]\nText.");
    expect(act("to-paragraph", "[NOTE]\nText[[]].")).toBe("Text.");
    for (const kind of ["note", "tip", "important", "warning", "caution"]) {
      const view = parse(act(`to-${kind}`, "Text[[]]."));
      expect(view.blocks[0]).toMatchObject({ kind: "admonition", admonition: kind });
    }
  });

  it("wraps a paragraph in a delimited block", () => {
    expect(act("to-quote", "One.\n\nSaid[[]] so.\n\nThree.")).toBe("One.\n\n____\nSaid so.\n____\n\nThree.");
    for (const [id, kind] of [["to-quote", "quote"], ["to-example", "example"], ["to-sidebar", "sidebar"], ["to-listing", "listing"]]) {
      const view = parse(act(id!, "Line one\nline[[]] two"));
      expect(view.diagnostics).toEqual([]);
      expect(view.blocks[0]!.kind).toBe(kind);
    }
  });

  it("adds metadata lines above the block", () => {
    const titled = run(command("add-title"), stateOf("----\ncode[[]]\n----"));
    expect(text(titled)).toBe(".Title\n----\ncode\n----");
    expect(titled.sliceDoc(titled.selection.main.from, titled.selection.main.to)).toBe("Title");
    expect(act("add-anchor", ".T\n----\ncode[[]]\n----")).toBe("[[id]]\n.T\n----\ncode\n----");
    expect(act("add-role", "Text[[]].")).toBe("[.role]\nText.");
    expect(act("add-role", "[quote,A]\nText[[]].")).toBe("[quote.role,A]\nText.");
    expect(act("to-source", "[%nowrap]\n----\ncode[[]]\n----")).toBe("[source%nowrap]\n----\ncode\n----");
    expect(act("to-source", "[subs=quotes]\n----\ncode[[]]\n----")).toBe("[source,subs=quotes]\n----\ncode\n----");
    expect(parse(act("add-anchor", "Text[[]].")).anchors).toEqual([{ id: "id", start: 7, end: 12 }]);
  });

  it("moves, duplicates and deletes a block with its metadata", () => {
    const doc = "One.\n\n.T\nTwo[[]].\n\nThree.";
    expect(act("move-up", doc)).toBe(".T\nTwo.\n\nOne.\n\nThree.");
    expect(act("move-down", doc)).toBe("One.\n\nThree.\n\n.T\nTwo.");
    expect(act("duplicate", doc)).toBe("One.\n\n.T\nTwo.\n\n.T\nTwo.\n\nThree.");
    expect(act("delete", doc)).toBe("One.\n\nThree.");
    expect(act("delete", "One.\n\nLast[[]].")).toBe("One.");
    const selected = run(command("select"), stateOf(doc));
    expect(selected.sliceDoc(selected.selection.main.from, selected.selection.main.to)).toBe(".T\nTwo.");
  });

  it("offers a block the actions that fit it", () => {
    const ids = (marked: string) => {
      const state = stateOf(marked);
      const block = actionBlock(state.field(parsedField).blocks, state.selection.main.head)!;
      return blockActions(commands, block).map((item) => item.id);
    };
    expect(ids("Text[[]].")).toEqual(expect.arrayContaining(["to-note", "to-quote", "add-title", "delete"]));
    expect(ids("Text[[]].")).not.toContain("to-paragraph");
    expect(ids("NOTE: n[[]]")).toEqual(expect.arrayContaining(["to-paragraph", "to-tip"]));
    expect(ids("NOTE: n[[]]")).not.toContain("to-note");
    expect(ids("----\nx[[]]\n----")).toContain("to-source");
    expect(ids("----\nx[[]]\n----")).not.toContain("to-quote");
  });
});

describe("slash menu", () => {
  it("opens only for a slash alone on its line", () => {
    expect(slashQuery("/", 1)).toEqual({ from: 0, query: "" });
    expect(slashQuery("  /tab", 6)).toEqual({ from: 2, query: "tab" });
    expect(slashQuery("/таб", 4)).toEqual({ from: 0, query: "таб" });
    expect(slashQuery("//", 2)).toBeNull();
    expect(slashQuery("// note", 7)).toBeNull();
    expect(slashQuery("text /tab", 9)).toBeNull();
    expect(slashQuery("/tab rest", 4)).toBeNull();
    expect(slashQuery("and/or", 6)).toBeNull();
  });

  it("finds commands by label in either language and by syntax words", () => {
    const first = (query: string, messages = ru) => searchCommands(commands, messages, query)[0]?.id;
    expect(first("таб")).toBe("table");
    expect(first("table")).toBe("table");
    expect(first("cols")).toBe("table");
    expect(first("код")).toBe("monospace");
    expect(first("исх")).toBe("source");
    expect(first("bold", en)).toBe("strong");
    expect(first("h2")).toBe("heading-1");
    expect(searchCommands(commands, ru, "").length).toBe(
      commands.filter((item) => item.group !== "blockAction").length,
    );
    expect(searchCommands(commands, ru, "zzz")).toEqual([]);
    expect(searchCommands(commands, ru, "удал")).toEqual([]);
  });
});

describe("messages", () => {
  it("label every command, menu and group in both languages", () => {
    for (const messages of [en, ru]) {
      for (const item of commands) expect(messages.commands[item.id], item.id).toBeTruthy();
      for (const item of defaultToolbar) {
        if (typeof item === "object" && "menu" in item) {
          expect(messages.menus[item.menu], item.menu).toBeTruthy();
          for (const id of item.items) expect(command(id), id).toBeTruthy();
        } else if (typeof item === "string" && item !== "|") {
          expect(command(item), item).toBeTruthy();
        }
      }
    }
    expect(Object.keys(ru.commands).sort()).toEqual(Object.keys(en.commands).sort());
    expect(Object.keys(ru.tokens).sort()).toEqual(Object.keys(en.tokens).sort());
    expect(Object.keys(ru.styles).sort()).toEqual(Object.keys(en.styles).sort());
    expect(Object.keys(ru.attributes).sort()).toEqual(Object.keys(en.attributes).sort());
  });

  it("explain every token kind the parser writes", () => {
    const kinds = new Set(
      parse(
        "= T\nA <a@b.c>\nv1, 2026-01-01: r\n:k: v\n\n// c\n\n[[i]]\n.T\n[source]\n----\nx <1>\n----\n<1> c\n\n" +
          "*b* _i_ `m` #h# ^s^ ~s~ {k} https://a.b[x] <<i>> kbd:[x] +\nz\n\n* l\n\nNOTE: n\n\nimage::a.png[]\n\n'''\n\n<<<\n\n" +
          "....\nl\n....\n\n++++\np\n++++\n\n====\ne\n====\n\n____\nq\n____\n\n****\ns\n****\n\n--\no\n--\n\n|===\n|c\n|===\n",
      ).tokens.map((token) => token[2]),
    );
    expect(kinds.size).toBeGreaterThan(25);
    // Verbatim text needs no explanation.
    for (const kind of kinds) {
      if (!kind.endsWith("-text")) expect(en.tokens[kind], kind).toBeTruthy();
    }
  });

  it("explain the style and options of an attribute list", () => {
    expect(describeToken(ru, "block-attribute-list", "[source%linenums,rust]")).toBe(
      `source — ${ru.styles.source}\n%linenums — ${ru.styles["%linenums"]}\n${ru.tokens["block-attribute-list"]}`,
    );
    expect(describeToken(en, "attribute-entry", ":toc: left")).toContain(en.attributes.toc);
    expect(describeToken(en, "strong", "*b*")).toBe(en.tokens.strong);
    expect(describeToken(en, "unknown-kind", "x")).toBeNull();
  });
});
