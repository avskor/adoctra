// The default commands: the elements of AsciiDoc an author reaches for.
import { startCompletion } from "@codemirror/autocomplete";
import { foldCode } from "@codemirror/language";
import type { Block } from "../types.js";
import type { AssistCommand, AssistContext, CommandGroup, CommandTarget } from "./commands.js";
import { insertBlock, insertInline, tableTemplate, toggleLinePrefix, toggleMark } from "./edit.js";
import { changeListLevel, nestOtherList } from "./lists.js";
import {
  addMetaLine,
  addRole,
  deleteBlock,
  duplicateBlock,
  moveBlock,
  selectBlock,
  setAdmonition,
  setStyle,
  wrapBlock,
} from "./transform.js";

const innermost = (context: AssistContext) => context.path[context.path.length - 1];
const inKind = (kind: string) => (context: AssistContext) => innermost(context)?.kind === kind;
const hasToken = (kind: string) => (context: AssistContext) => context.tokens.has(kind);

function mark(id: string, icon: string, markup: string, token: string, keywords: string[]): AssistCommand {
  return {
    id,
    group: "inline",
    icon,
    keywords,
    example: `${markup}text${markup}`,
    run: toggleMark(markup),
    isActive: hasToken(token),
  };
}

function inline(id: string, icon: string, template: string, keywords: string[], token?: string): AssistCommand {
  return {
    id,
    group: "inline",
    icon,
    keywords,
    example: plain(template),
    run: insertInline(template),
    ...(token ? { isActive: hasToken(token) } : {}),
  };
}

const HEADING = /^={1,6}[ \t]+/;
const LIST = /^[ \t]*(?:\*+|\.+|-)[ \t]+(?:\[[ x*]\][ \t]+)?/;

function prefix(
  id: string,
  group: CommandGroup,
  icon: string,
  text: string,
  existing: RegExp,
  keywords: string[],
  example = `${text}text`,
): AssistCommand {
  return {
    id,
    group,
    icon,
    keywords,
    example,
    run: toggleLinePrefix(text, existing),
    isActive: (context) => context.line.startsWith(text),
  };
}

function block(
  id: string,
  icon: string,
  template: string,
  keywords: string[],
  isActive?: (context: AssistContext) => boolean,
  group: CommandGroup = "block",
): AssistCommand {
  return {
    id,
    group,
    icon,
    keywords,
    example: plain(template),
    run: insertBlock(template),
    ...(isActive ? { isActive } : {}),
  };
}

/** A template as the text it writes, placeholders in place. */
function plain(template: string): string {
  return template.replace(/[$#]\{\d+(?::@?([^{}]*))?\}/g, (_all, placeholder = "") => placeholder);
}

const ADMONITIONS = ["note", "tip", "important", "warning", "caution"] as const;

function action(
  id: string,
  icon: string,
  appliesTo: (block: Block) => boolean,
  run: AssistCommand["run"],
): AssistCommand {
  return { id, group: "blockAction", icon, keywords: [], run, appliesTo };
}

const PARAGRAPH_LIKE = new Set(["paragraph", "admonition"]);
const STYLED = new Set(["paragraph", "listing", "literal", "example", "sidebar", "quote", "verse", "open", "admonition", "table", "list", "dlist", "image"]);
const isParagraph = (target: Block) => target.kind === "paragraph";
const notInline = (target: Block) => target.kind !== "list-item" && target.kind !== "description" && target.kind !== "table-cell";
const titled = (target: Block) => STYLED.has(target.kind);
const required = (target: Block | undefined): target is Block => target !== undefined;
const isItem = (target: Block) => target.kind === "list-item";

/** Run a cursor command with the cursor on the block's first line. */
function atBlock(target: CommandTarget, block: Block, run: (target: CommandTarget) => boolean): boolean {
  target.dispatch(target.state.update({ selection: { anchor: block.start } }));
  return run(target);
}

/** The default command set, in the order menus list it. */
export function defaultCommands(): AssistCommand[] {
  return [
    // ---- inline --------------------------------------------------------
    mark("strong", "B", "*", "strong", ["bold", "strong"]),
    mark("emphasis", "I", "_", "emphasis", ["italic", "emphasis"]),
    mark("monospace", "</>", "`", "monospace", ["code", "monospace", "literal"]),
    mark("mark", "✎", "#", "mark", ["highlight", "mark"]),
    mark("superscript", "x²", "^", "superscript", ["superscript", "sup"]),
    mark("subscript", "x₂", "~", "subscript", ["subscript", "sub"]),
    inline("link", "🔗", "https://${1:example.org}[${2:@link text}]", ["link", "url", "href"], "link"),
    {
      id: "xref",
      group: "inline",
      icon: "⇢",
      keywords: ["xref", "cross reference", "anchor"],
      example: "<<section-id>>",
      run(target, _block, view) {
        const { from, to } = target.state.selection.main;
        target.dispatch(
          target.state.update({
            changes: { from, to, insert: "<<>>" },
            selection: { anchor: from + 2 },
            userEvent: "input",
          }),
        );
        if (view) startCompletion(view);
        return true;
      },
      isActive: hasToken("xref"),
    },
    inline("inline-image", "▣", "image:${1:path.png}[${2:@alt text}]", ["image", "inline image", "icon"]),
    inline("footnote", "†", "footnote:[${1:@note text}]", ["footnote", "note"]),
    inline("attribute-ref", "{}", "{${1:@attribute}}", ["attribute", "variable", "reference"], "attribute-ref"),
    inline("kbd", "⌨", "kbd:[${1:@Ctrl+C}]", ["kbd", "keyboard", "shortcut", "experimental"]),
    inline("btn", "▢", "btn:[${1:@OK}]", ["btn", "button", "experimental"]),
    inline("menu", "☰", "menu:${1:File}[${2:@Save}]", ["menu", "experimental"]),
    inline("passthrough", "+", "+${1:@text}+", ["passthrough", "literal text", "escape"]),
    {
      id: "line-break",
      group: "inline",
      icon: "↵",
      keywords: ["line break", "hard break", "br"],
      example: "first line +\nsecond line",
      run: insertInline(" +\n", true),
    },

    // ---- headings ------------------------------------------------------
    prefix("title", "heading", "H0", "= ", HEADING, ["title", "document title", "h1"]),
    prefix("heading-1", "heading", "H1", "== ", HEADING, ["section", "heading", "h2"]),
    prefix("heading-2", "heading", "H2", "=== ", HEADING, ["subsection", "heading", "h3"]),
    prefix("heading-3", "heading", "H3", "==== ", HEADING, ["heading", "h4"]),
    prefix("heading-4", "heading", "H4", "===== ", HEADING, ["heading", "h5"]),
    prefix("heading-5", "heading", "H5", "====== ", HEADING, ["heading", "h6"]),

    // ---- lists ---------------------------------------------------------
    // The samples show that a longer marker is a deeper level.
    prefix("ulist", "list", "•", "* ", LIST, ["bullet", "unordered list", "ul", "nested"], "* item\n** nested item\n* item"),
    prefix("olist", "list", "1.", ". ", LIST, ["numbered", "ordered list", "ol", "nested"], ". item\n.. nested item\n. item"),
    prefix("checklist", "list", "☑", "* [ ] ", LIST, ["checklist", "todo", "task"], "* [ ] to do\n* [x] done\n** [ ] nested"),
    {
      id: "list-indent",
      group: "list",
      icon: "⇥",
      keywords: ["indent", "nested list", "sublist", "level", "deeper", "tab"],
      example: "* item\n** one level deeper",
      run: changeListLevel(1),
    },
    {
      id: "list-outdent",
      group: "list",
      icon: "⇤",
      keywords: ["outdent", "unindent", "level", "higher", "shift-tab"],
      example: "** item\n* one level higher",
      run: changeListLevel(-1),
    },
    {
      id: "list-nest-other",
      group: "list",
      icon: "⊢",
      keywords: ["nested list", "mixed list", "numbered inside bullets", "sublist"],
      example: "* item\n.. numbered list inside\n.. second",
      run: nestOtherList,
    },
    block("dlist", "∷", "${1:@Term}:: ${2:Description}", ["description list", "definition", "dl", "term"], inKind("dlist"), "list"),

    // ---- blocks --------------------------------------------------------
    block("source", "{ }", "[source,${1:language}]\n----\n${2:@code}\n----", ["source", "code block", "listing", "highlight"], (context) => {
      const current = innermost(context);
      return current?.kind === "listing" && current.style === "source";
    }),
    block("listing", "≡", "----\n${1:@text}\n----", ["listing", "verbatim", "preformatted"], (context) => {
      const current = innermost(context);
      return current?.kind === "listing" && current.style !== "source";
    }),
    block("literal", "…", "....\n${1:@text}\n....", ["literal", "verbatim"], inKind("literal")),
    ...ADMONITIONS.map((kind) =>
      block(
        kind,
        kind === "note" ? "ℹ" : kind === "tip" ? "💡" : kind === "caution" ? "🔥" : kind === "warning" ? "⚠" : "❗",
        `${kind.toUpperCase()}: \${1:@text}`,
        ["admonition", kind, "callout"],
        (context) => context.path.some((item) => item.admonition === kind),
      ),
    ),
    block("quote", "❝", "[quote,${1:Author},${2:Source}]\n____\n${3:@Quote}\n____", ["quote", "blockquote", "citation"], inKind("quote")),
    block("verse", "♪", "[verse,${1:Author}]\n____\n${2:@Verse}\n____", ["verse", "poem"], inKind("verse")),
    block("example", "E", "====\n${1:@Example}\n====", ["example"], inKind("example")),
    block("sidebar", "▌", "****\n${1:@Sidebar}\n****", ["sidebar", "aside"], inKind("sidebar")),
    block("open", "⧉", "--\n${1:@Content}\n--", ["open block", "container"], inKind("open")),
    block("collapsible", "▸", ".${1:Details}\n[%collapsible]\n====\n${2:@Hidden content}\n====", ["collapsible", "details", "spoiler", "summary"]),
    {
      ...block("table", "▦", tableTemplate(2, 2), ["table", "grid", "cols"], (context) =>
        context.path.some((item) => item.kind === "table"),
      ),
    },
    block("stem", "∑", "[stem]\n++++\n${1:@sqrt(4) = 2}\n++++", ["stem", "math", "formula", "latex", "asciimath"], inKind("stem")),
    block("pass", "<>", "++++\n${1:@<div>raw HTML</div>}\n++++", ["passthrough", "raw html"], inKind("pass")),

    // ---- insert --------------------------------------------------------
    block("image", "🖼", "image::${1:path.png}[${2:alt text}]", ["image", "picture", "figure"], inKind("image"), "insert"),
    block("video", "▶", "video::${1:file.mp4}[]", ["video", "youtube", "vimeo"], inKind("video"), "insert"),
    block("include", "⤵", "include::${1:file.adoc}[]", ["include", "partial", "file"], undefined, "insert"),
    block("toc", "☷", "toc::[]", ["toc", "table of contents"], inKind("toc"), "insert"),
    block("attribute", ":", ":${1:name}: ${2:value}", ["attribute", "variable", "setting", "entry"], undefined, "insert"),
    block("comment", "//", "// ${1:@comment}", ["comment", "hidden"], hasToken("comment"), "insert"),
    block("thematic-break", "—", "'''", ["horizontal rule", "thematic break", "line", "hr"], undefined, "insert"),
    block("page-break", "⤓", "<<<", ["page break"], undefined, "insert"),

    // ---- block actions ---------------------------------------------------
    action("to-paragraph", "¶", (target) => target.kind === "admonition", (target, item) =>
      required(item) && setAdmonition(target, item, null),
    ),
    ...ADMONITIONS.map((kind) =>
      action(
        `to-${kind}`,
        "!",
        (target) => PARAGRAPH_LIKE.has(target.kind) && target.admonition !== kind,
        (target, item) => required(item) && setAdmonition(target, item, kind.toUpperCase()),
      ),
    ),
    action("to-quote", "❝", isParagraph, (target, item) => required(item) && wrapBlock(target, item, "____")),
    action("to-example", "E", isParagraph, (target, item) => required(item) && wrapBlock(target, item, "====")),
    action("to-sidebar", "▌", isParagraph, (target, item) => required(item) && wrapBlock(target, item, "****")),
    action("to-listing", "≡", isParagraph, (target, item) => required(item) && wrapBlock(target, item, "----")),
    action("to-source", "{ }", (target) => target.kind === "listing" || target.kind === "literal", (target, item) =>
      required(item) && setStyle(target, item, "source"),
    ),
    action("add-title", "T", titled, (target, item) => required(item) && addMetaLine(target, item, ".", "Title", "")),
    action("add-anchor", "#", notInline, (target, item) => required(item) && addMetaLine(target, item, "[[", "id", "]]")),
    action("add-role", ".", titled, (target, item) => required(item) && addRole(target, item)),
    action("item-indent", "⇥", isItem, (target, item) => required(item) && atBlock(target, item, changeListLevel(1))),
    action("item-outdent", "⇤", isItem, (target, item) => required(item) && atBlock(target, item, changeListLevel(-1))),
    action("select", "⬚", () => true, (target, item) => required(item) && selectBlock(target, item)),
    action("fold", "▾", (target) => target.end > target.start, (target, item, view) => {
      if (!required(item) || !view) return false;
      target.dispatch(target.state.update({ selection: { anchor: item.start } }));
      return foldCode(view);
    }),
    action("move-up", "↑", notInline, (target, item) => required(item) && moveBlock(target, item, -1)),
    action("move-down", "↓", notInline, (target, item) => required(item) && moveBlock(target, item, 1)),
    action("duplicate", "⧉", notInline, (target, item) => required(item) && duplicateBlock(target, item)),
    action("delete", "✕", () => true, (target, item) => required(item) && deleteBlock(target, item)),
  ];
}

/** The toolbar's default layout: command ids, `|` separators and menus. */
export type ToolbarItem = string | { menu: string; icon: string; items: string[] } | { table: true };

export const defaultToolbar: ToolbarItem[] = [
  "strong",
  "emphasis",
  "monospace",
  "|",
  { menu: "headings", icon: "H", items: ["title", "heading-1", "heading-2", "heading-3", "heading-4", "heading-5"] },
  "ulist",
  "olist",
  "checklist",
  "list-outdent",
  "list-indent",
  "|",
  "link",
  "xref",
  "image",
  { table: true },
  "source",
  "|",
  { menu: "admonitions", icon: "ℹ", items: ["note", "tip", "important", "warning", "caution"] },
  { menu: "blocks", icon: "▤", items: ["listing", "literal", "quote", "verse", "example", "sidebar", "open", "collapsible", "stem", "pass"] },
  {
    menu: "more",
    icon: "⋯",
    items: [
      "mark", "superscript", "subscript", "footnote", "inline-image", "attribute-ref", "kbd", "btn", "menu",
      "passthrough", "line-break", "list-nest-other", "dlist", "video", "include", "toc", "attribute", "comment", "thematic-break", "page-break",
    ],
  },
];
