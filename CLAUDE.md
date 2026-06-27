# CLAUDE.md

Guidance for Claude Code (and any AI assistant) working in this repository.

## What this is

`adoctra` is a Rust toolchain for the AsciiDoc markup language: a parser, output
backends, a CLI, and WASM bindings for an editor frontend.

> AsciiDoc is a registered trademark of the Eclipse Foundation, Inc. This is an
> independent processor and is not affiliated with or endorsed by the Eclipse
> Foundation or the Asciidoctor project. Never use `asciidoc` or `asciidoctor`
> as a crate, binary, or brand name; describe compatibility in prose only.

## Workspace layout

Cargo workspace (`crates/*`), virtual manifest at the root.

- `adoctra-core` — parser and document model. **Backend-agnostic.** Owns the
  `Backend` and extension (`TreeProcessor`/`Registry`) traits.
- `adoctra-html` — HTML5 backend.
- `adoctra-docx` — DOCX backend. **Scaffold/stub** (returns `Unsupported`);
  wire up `docx-rs` here. Do not delete the traversal seam.
- `adoctra-json` — JSON/ASG backend; intended TCK adapter.
- `adoctra` — umbrella crate. Re-exports core; backends behind feature flags
  (`html` default, plus `docx`, `json`, `all-backends`).
- `adoctra-cli` — the `adoctra` binary (native only).
- `adoctra-wasm` — editor-facing WASM bindings (`parse_json`, `tokens_json`).
  Depends on `adoctra-core` **only** — never on backends.

Dependency direction is one-way: `cli -> adoctra -> {core, backends}`, each
backend depends only on `core`. Keep it that way. `core` must never depend on a
backend, and `adoctra-wasm` must never pull in a backend.

## Three consumers, one parser

The model serves three audiences; keep their needs in mind when changing core:

1. **Backends (converters)** want the full AST for traversal (e.g. building a
   TOC by walking heading nodes).
2. **The editor (via WASM)** wants: `parse` to be *error-tolerant* (never `Err`
   on bad syntax — return a partial `Document` plus `diagnostics`), nodes to
   carry source **spans** (byte offsets), and a separate **token** stream for
   highlighting.
3. **The CLI** wants one-shot parse + convert.

### Hard rules for `adoctra-core`

- `parse(&str) -> ParseResult` must **never panic and never return `Err`** for
  malformed input. Syntax problems are `Diagnostic`s (data), not failures.
  `Error`/`Result` are reserved for real failures (I/O in backends).
- Every block node keeps a `Span { start, end }` of **byte** offsets into the
  original source. Add spans to any new node types too.
- Keep output deterministic (e.g. `Attributes` is a `BTreeMap`) so snapshot and
  TCK tests stay stable.
- The tokenizer (`tokens`) must stay consistent with the parser's view of the
  source; prefer driving editor highlighting from it over a duplicated JS
  grammar.

## Conventions

- Edition is currently `2021` / `resolver = "2"` to match the local toolchain.
  Bump to `2024` / `resolver = "3"` / `rust-version = "1.85"` only deliberately,
  and update `rust-toolchain.toml` and CLAUDE.md together.
- Use workspace inheritance: `version.workspace = true`, etc. New third-party
  deps go in the root `[workspace.dependencies]` and are referenced with
  `{ workspace = true }`.
- Keep the scaffold **dependency-light**. The core/backends are intentionally
  `std`-only right now; the `thiserror`/`docx-rs`/`clap`/`serde` lines in the
  root manifest are commented placeholders. Add a dependency only when it earns
  its place, and prefer `cargo add` so versions stay current.
- WASM glue stays behind `#[cfg(target_arch = "wasm32")]`; the JSON
  serialization layer in `adoctra-wasm` must remain pure Rust so it compiles and
  unit-tests on the native target.
- Run `cargo fmt` and keep `cargo clippy --workspace --all-features` clean.
- **Test coverage is mandatory** for every change — see "Test coverage is
  mandatory" under "Build, test, run" for what that requires.

## Build, test, run

```sh
cargo build --workspace --all-features
cargo test  --workspace --all-features
cargo fmt --all
cargo clippy --workspace --all-features

# CLI
cargo run -p adoctra-cli -- examples/sample.adoc --to html --stdout
cargo run -p adoctra-cli -- examples/sample.adoc --to json --stdout

# WASM (run on a toolchain with the target installed)
rustup target add wasm32-unknown-unknown
wasm-pack build crates/adoctra-wasm --target web   # or --target bundler

# See the JSON an editor frontend receives, on the native target:
cargo run --example editor_demo -p adoctra-wasm
```

### Test coverage is mandatory

Every change ships with tests **in the same commit** — this is a hard rule, not
a preference. A feature or fix without tests is *incomplete*, not "to be tested
later".

- **New functionality must be covered.** Core parser/inline changes assert both
  the resulting AST shape *and* byte [`Span`]s (see the test modules in
  `crates/adoctra-core/src/{parser,inline,tokenizer}.rs`).
- **Every new `Inline`/`Block` node type is tested in each consumer that renders
  it** — HTML, JSON *and* WASM — not only in core. Resolution done once in core
  (e.g. `{attr}`) is asserted in core; each backend asserts how it renders the
  resolved node (including fallbacks like a missing attribute).
- **Bug fixes get a regression test** that fails before the fix and passes
  after.
- **Keep the golden compatibility layer in step.** For any new inline/block
  construct, add an input under `crates/adoctra-html/tests/golden/` and
  regenerate the asciidoctor reference with `regenerate.sh`.
- `cargo test --workspace --all-features` must stay green, and `cargo clippy
  --workspace --all-features` / `cargo fmt --all` must stay clean, before the
  change is considered done.

## Current state (so you don't assume more than exists)

Working: parser for header attributes, headings, listing blocks, literal blocks
(`....` and `[literal]`), admonitions (paragraph form), paragraphs,
lists (unordered `*`/`-`, ordered `.` and explicit-numbered `1.` — the latter a
marker identity *distinct* from `.` (so a `1.` after a `.` list nests), with
digit values ignored (numbering is positional, matching asciidoctor); **nested**
via marker runs `**`/`...` and
across kinds, asciidoctor's "unique marker → deeper level" rule; a list starts
at column 0 but deeper levels may be **indented** — nesting is decided by marker
*identity*, not indent, so an indented same-marker line is a sibling;
**multi-line** item text (wrapped plain lines fold into the item) and `+`
**continuations** attaching blocks (paragraph, listing) to the item in source
order; a blank line ends the list), and
inline `*strong*` / `_emphasis_`, `` `monospace` `` / `#mark#` (constrained)
and `^superscript^` / `~subscript~` (unconstrained); **links** — the
`link:target[text]` / `mailto:target[text]` macro (brackets required), the URL
macro `https://h[text]` and **bare autolinks** for `http(s)`/`ftp`/`irc` (with
asciidoctor's left-boundary rule and trailing-punctuation trimming), an empty
or omitted text rendered `class="bare"`, and inline markup inside link text;
**cross-references** — the `<<id>>` / `<<id,text>>` angle form and the
`xref:id[text]` macro, rendered as an internal `<a href="#id">` link, with inline
markup parsed inside the reftext; a **post-parse resolve pass**
(`adoctra-core/src/resolve.rs`, the first pass over the *whole* document — needed
because a reference may point forward) builds a registry of **auto-generated
section ids** (asciidoctor's `sectids`: lowercase, non-alphanumerics dropped or
folded to `_`, `_`-prefixed — `== Hello World` → `_hello_world`, with `_2`/`_3`
deduplication) and resolves each reference: a textless `<<id>>` / `xref:id[]` takes
the section's title as display text (inline formatting preserved), an explicit
reftext is kept verbatim, and an unknown id emits a `Warning` diagnostic and
renders the `[id]` *unresolved* fallback (asciidoctor-compatible). **Explicit
anchors** join the same registry: a *block anchor* (a whole-line `[[id]]` /
`[[id,reftext]]`) attaches its id and inline `reftext` to the next block,
overriding a section's auto id; an *inline anchor* (`[[id]]` / `[[id,reftext]]`
sharing its line with text, or the `anchor:id[reftext]` macro) renders as an empty
`<a id>` at that point, its `reftext` feeding a textless `<<id>>`; a known id with
no natural text falls back to `[id]` *without* a diagnostic and a duplicate id
warns once (first definition wins). **Block attribute lists** share that
seam: a whole-line `[…]` (anything but the `[[id]]` anchor or an empty `[]`) is
parsed as the full `[style, #id.role, name=value, %option]` list — matching
asciidoctor, which consumes *any* such line (even `[draft]` / `[Note to self]`),
so a malformed id/role token is skipped rather than rejecting the line. The
shorthand `#id`/`.role` and the longhand `id=`/`role=` both fold into `Block.id`
(last-wins) and `Block.roles` (accumulated; a `role=` value splits on spaces); the
first positional is the block **style**, `name=value` pairs land in
`Block.attrs.named` (a `BTreeMap`), `%option` flags in `Block.attrs.options`. Two
styles are wired to rendering. `source`: `[source,<lang>]` folds the language
into a `language` attribute *in core*, and the HTML backend renders the listing as
asciidoctor's highlight form `<pre class="highlight"><code class="language-LANG"
data-lang="LANG">` (no language ⇒ a bare `<code>`). `literal`: `[literal]` on a
paragraph turns it into a verbatim **literal block** (`BlockKind::Literal`, raw
source text, no inline subs) — the first style that changes a block's *type*; the
delimited form `....` … `....` (mirror of the `----` listing) yields the same kind.
The HTML backend renders both as `<pre class="literalblock">` (asciidoctor's
`literalblock` class moved onto the inner `<pre>`, per adoctra's no-wrapper
convention); the tokenizer emits `literal-delimiter`/`literal-text`. The
uppercase admonition styles (`NOTE`/`TIP`/`IMPORTANT`/`WARNING`/`CAUTION`) are the
next type-changing styles: `[NOTE]` on a paragraph **and** the label prefix
`NOTE: …` on a paragraph's first line both yield a `BlockKind::Admonition { kind,
blocks }` — like asciidoctor the body is a parsed *container* of child blocks, so
the paragraph and label forms wrap their inlines in a single child paragraph (an
`AdmonitionKind` enum; both forms case-sensitive, matching asciidoctor —
`[note]`/`note:` stay paragraphs). The HTML backend renders a **modern, semantic**
`<aside class="admonition" data-type="TYPE" aria-label="…">` with a
`.admonition-title` and the rendered child blocks — *not* asciidoctor's
table/icon-font layout; id/roles hang on the `<aside>`. This is also where adoctra first **ships a
reference stylesheet**: `adoctra-html` exposes `STYLESHEET` (`assets/adoctra.css`,
a modern theme — custom properties, `color-mix()`, logical properties, SVG-mask
icons) and `convert()` inlines it into a `<head>` so output is styled out of the
box. The tokenizer emits an `admonition-label` token for the `NOTE:` prefix (the
`[NOTE]` style is just a `block-attribute-list`). **Example blocks** are the
first **container** block kind besides a list: a `====` … `====` delimited block
(and the `[example]` style on a paragraph) yields `BlockKind::Example { blocks }`.
Unlike the verbatim `----`/`....` blocks, the body is parsed *as blocks* — the
top-level block collector was refactored into a shared `collect_blocks(stop)`
that both the document body and a container body call, so nested blocks, inline
subs, block metadata (`[[id]]`/`[.role]`/`[source]`) and the type-changing style
fixups (`[literal]`/`[NOTE]`/`[example]`) all work at any depth, and the resolve
pass recurses into a container so anchors/ids inside register and `<<id>>`
resolves across the whole document. A paragraph now ends at any block-delimiter
line (`----`/`....`/`====`) so a container's closing fence is never folded into
the body. The HTML backend renders a single `<div class="exampleblock">` wrapping
the rendered children (asciidoctor nests `exampleblock`/`content` divs; adoctra's
no-wrapper convention emits one element carrying the id/roles), `adoctra.css`
ships an `.exampleblock` rule, JSON/WASM serialize `name:"example"` with a nested
`blocks` array, and the tokenizer emits `example-delimiter` (the body, *not*
verbatim, keeps its normal tokens). Building directly on that container, an
uppercase admonition style on a `====` block is now the **delimited admonition**
form: `apply_style_fixup` moves the parsed `Example` children into
`BlockKind::Admonition { kind, blocks }`, so an admonition can hold several blocks
(paragraphs, a listing, …); HTML renders the same `<aside>` with the children
inside (no `exampleblock` wrapper), JSON/WASM serialize `name:"admonition"` with
the nested `blocks` array, the resolve pass recurses into it, and `[note]`
(lowercase) stays a plain example. A **block title** — a `.Title` line preceding
*any* block (asciidoctor's `BlockTitleRx`: a leading `.` then a non-space,
non-`.` char, so it is *distinct* from the ordered-list marker `. `, the `....`
delimiter and the `..`/`...` nested markers) — is the first block metadata that is
neither id, role nor attribute: it rides the same `PendingMeta` seam in
`collect_blocks` (so it floats across blank lines and attaches at any depth), its
text is parsed inlines (inline subs apply, e.g. `.A *bold* caption`), a second
`.Title` wins, and it lives on `Block.title` (`Option<Vec<Inline>>`) — *not* inside
`BlockKind`, so a `.Title` before a `[NOTE]`/`[example]` block stays on the
resulting container, not the wrapped child. The HTML backend renders it as a
`<div class="title">` immediately before the block's element — just *inside* the
wrapper for the container blocks (after the `.admonition-title` label in an
`<aside>`, as the first child of an `exampleblock`); `adoctra.css` ships a `.title`
rule; JSON/WASM serialize a `title` field (the inlines, or `null`); the tokenizer
emits one `block-title` token per `.Title` line (its inline markup *not*
sub-tokenized, like the block anchor's reftext). Building on titles, asciidoctor's
**caption numbering** is wired for example blocks: a third resolve phase
(`assign_captions`, in `resolve.rs`, walking the whole document in source order)
numbers every **titled** `Example` block — an untitled example is skipped and does
*not* advance the counter, matching asciidoctor — and stores the resolved prefix on
`Block.caption` (`Option<String>`, a ready-to-render `"Example 1. "` like
asciidoctor's `@caption`). The prefix word comes from the `example-caption`
attribute (default `Example`, overridable with `:example-caption: …`). The HTML
backend prepends it inside the `<div class="title">` (`#{caption}#{title}`), and
JSON/WASM serialize a `caption` field (the string, or `null`). Only example blocks
are numbered: `listing-caption` stays unset (a titled listing/literal keeps its bare
title) and `figure-caption`/`table-caption` are moot until image/table blocks exist.
Snapshotted by `golden/examples/caption_numbering.adoc` and the golden oracle no
longer strips the counter. Other styles
are parsed and carried on `Block.attrs.style` but do not yet change a block's
*type*. The id
lives on the block (`Block.id`, `None` for the level-0 doctitle and plain blocks)
and roles on `Block.roles` — the HTML backend emits the id on `hN`/`p`/`pre`/`ol`/
`ul` and roles as a `class` (appended to a listing's base class; asciidoctor hangs
both on a wrapping `<div>`, adoctra emits no wrapper so they go on the inner
element), inline anchors as `<a id="…"></a>`, and JSON/WASM expose id, roles,
style, named attributes and options; the tokenizer emits one `block-attribute-list`
token per such line; a single inline scanner
(`adoctra-core/src/inline.rs`) shared by the parser and tokenizer; inline nodes
carry byte spans; attribute references `{name}` are resolved once in core
against the document attributes; typographic replacements (`(C)`/`(R)`/`(TM)`,
spaced/word `--` em dashes, `...` ellipsis, the `'` apostrophe, and the
`->`/`=>`/`<-`/`<=` arrows, with `\` escaping) are resolved once in core into
the `Text` value (the HTML backend emits numeric entities, JSON the literal
char); HTML and JSON backends; editor layer
(error-tolerant `parse`, spans, diagnostics, tokenizer); WASM `parse_json` /
`tokens_json`; golden compatibility tests against asciidoctor
(`adoctra-html/tests/golden/`, list fixtures under `golden/lists/`, source-block
fixtures under `golden/blocks/`, literal-block fixtures under `golden/literals/`,
admonition fixtures under `golden/admonitions/`, example-block fixtures under
`golden/examples/`, and block-title fixtures under `golden/titles/`).

Scaffold only: DOCX backend; the extension API exists but `parse`/`convert` do
not invoke it yet.

Not yet implemented (common asks — treat as roadmap, do not assume present):
broader language coverage (tables, footnotes, includes, conditionals,
block/inline macros). The full block-attribute list is now *parsed* (style, named
attributes, `%options`, id/role folding), but most of it has nothing to render
against yet: most **styles that change a block's type** (`[quote]`/`[verse]` →
quote, `[sidebar]`, …) are unimplemented — only `[source,<lang>]`, `[literal]`,
`[example]` and the admonition styles are wired — as is **rendering of options**
(`%hardbreaks`, `%nowrap`, …), which are captured but not yet acted on. For
**admonitions**, the *paragraph* forms (`[NOTE]` style and `NOTE: ` label) and the
**delimited form** (`[NOTE]` on a `====` block) are done. **Block titles**
(`.Title`) are parsed and rendered on every block (on `Block.title`); still missing
here: admonitions on list items. **Caption numbering** is done for **example
blocks** (`Example N.` via the `example-caption` attribute, on `Block.caption`); the
`Figure N.` / `Table N.` counters await image/table blocks, the unset form
`:!example-caption:` awaits **attribute-unset** support (`parse_attribute_entry`
does not yet parse `:!name:`), and there is no custom per-block `caption=` attribute
override. Also: **indented/offset
literal** (leading-space lines) and delimiters longer than four chars (the same
single-length limitation applies to `====`: a nested same-length `====` closes the
outer block); **anchors on individual list items**, TOC generation,
incremental/block reparse, and a pull/event API over the AST.

When asked to "add a feature," check this list first and say plainly whether it
is new work or wiring up an existing seam.

## Reference

A GitLab project, `fdc-marketplace`, is the practical reference for broader
AI-assisted development conventions (code review, CLAUDE.md style, plugins, RAG
document management with SVN).
