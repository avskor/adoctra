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
- Every block node keeps a `Span { source_id, start, end }` of **byte** offsets into the
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

The accepted compatibility roadmap and continuation status live in
`docs/implementation-plan.adoc`; the reference contract is in
`docs/compatibility.adoc`. Read these before compatibility work and update their
status when completing a step. The machine-readable Asciidoctor version is
`compatibility/asciidoctor-version`. Keep known differences explicit; an expected
failure is not evidence of conformance. Preserve the std-only core and existing
crate boundaries while implementing the roadmap incrementally.

The source/context API is documented in `docs/parser-api.adoc`. `Span` now also
contains `source_id`; keep it on blocks, inline nodes, list items, tokens and
diagnostics. `Document.attributes` is the header snapshot; block
`attribute_context` snapshots use copy-on-write storage and reflect block
completion. Caption numbering follows child-before-parent completion order.
Header/body attribute assignment and both unset forms are implemented.
Document.header stores source declarations (authors/revision and spans); effective
metadata stays in Document.attributes. The title remains the first level-0
Heading, with only its span referenced by the header. Header parsing is shared
with all consumers. Indexed author_N overrides reconcile contiguous keys from
author_1 at header completion, after explicit author/authors priority, comparing
against source declarations. Unchanged compound names retain their components;
empty entries are skipped, and missing derived fields/emails are not deleted.
Derived metadata may replace locked host fields without unlocking subsequent
document entries. Body author_N assignments do not recompute name fields.
Original Author records and physical spans remain unchanged. Effective author
derivation in author_values.rs removes raw XML tags
from name components while preserving formatted author values. Attributes retain
Text/Raw parts through substitutions and copy-on-write snapshots; get/iter remain
flat string APIs, raw_parts exposes boundaries. AttributeRef.parts and optional
JSON/WASM parts preserve these boundaries; WASM also exports attribute_parts.
HTML escapes Text and emits Raw unchanged. Host values are already substituted;
source whole-value pass supports none, a/attributes and c/specialcharacters/
specialchars, with ordered unique stages. Entity restoration runs inside resolved
attribute references (entity_references.rs), including Text/Raw boundaries,
backslash escapes and one-pass protection removal. It preserves entity syntax as
Raw parts, the original reference span, and untouched attribute snapshots.
Other pass stages, other post-attribute replacements, entities in source text or
across separate inline nodes, and raw whole-doctitle replacement remain work.
The tokenizer shares header parsing (author/revision tokens). Setext document titles use adjacent
processed lines, Unicode character lengths (difference <= 1), uniform =/-/~/^/+
underlines and effective level zero. ATX takes precedence. The Heading/header
title span includes both lines; inline spans cover title text only. Highlight
both lines as separate heading tokens and retain physical source fragments across
includes/selection/indent. Setext installs compat-mode before subsequent header
metadata, respecting locked overrides; its substitution effects and body setext
sections remain follow-up work. JSON and WASM expose header metadata;
WASM includes source spans. See parser-api.adoc for remaining header limitations.
Comments (`//`, matching slash fences of length >= 4, and `[comment]` paragraphs
or open blocks) are omitted contextually. ParseResult.comments retains source
ranges, exported to WASM and comment tokens. Inline scanning across removed
comment lines restores spans through a fragment map. Verbatim regions preserve
comment-looking text. The tokenizer currently parses first to reuse comment
classification; consolidating the remaining structural pass is still roadmap work.
Container bodies are bounded by their preprocessed closing fence before inner parsing,
as in the Ruby reader (a nested comment cannot hide the parent's closing fence).
Conditional preprocessing (`ifdef`/`ifndef`/`ifeval`/`endif`, nested and single-line forms,
`,`/`+`, escaping and diagnostics) uses a lazy Reader shared with header/block parsing.
The parser owns attribute mutations; conditions use attributes at read time. A container
is captured before its children are parsed, so its internal assignments do not affect
conditions in that captured body. Top-level comment bodies bypass preprocessing;
verbatim bodies do not. Reader fragments map retained text back to original offsets,
composing with the comment map. ParseResult and WASM expose `conditional_directives`
and `inactive`; tokenizer uses the parser's processed lines and trivia, with corresponding
`conditional-directive`/`inactive` kinds. `ifeval` uses the pinned Ruby comparison
and coercion rules (including arbitrary-length integers, first-operator splitting,
and the retained closing quote in string values). It always opens a block; malformed
active expressions diagnose without pushing, skipped expressions only balance nesting.
Ordinary/intrinsic attributes and escaped references are supported; set/counter side
effects remain unimplemented; basic include expansion is available with an explicit provider.
ResourceProvider/ResourceRequest/Resource and typed ResourceError now define host
resource loading. SourceMap::load resolves through an explicit provider, reuses an
identical canonical-name/text snapshot, and preserves older ids after edits. Core's
MemoryResources uses virtual slash paths; CLI owns the rooted filesystem adapter
and loads its primary file through it. WASM exports ResourceWorkspace for host
buffers and parsing/highlighting immutable snapshots by id. Neither parser nor
core loads ambient files or URLs. parse_resource/tokens_resource expand basic
include::target[] lazily through that provider, retaining one parser/attribute
context across files. Span.provenance optionally indexes ParseResult.provenance
for ranges spanning source occurrences; ordinary coordinates identify the first
physical fragment. Preserve this table with the AST for editor positions. WASM
exports provenance/include_directives; tokens use individual physical spans.
Includes have depth/byte limits and cycle diagnostics. CLI file input expands
includes within its existing root; stdin and string APIs have no provider.
Includes support lines/tag/tags selection before preprocessing, including ordered
wildcards/exclusions and physical warnings for malformed tag nesting. Preserve
separate provenance fragments for disjoint retained lines, even within one source
occurrence. Resource byte limits count the entire loaded snapshot before filtering.
Includes support opts/options=optional (suppress only NotFound, not access/encoding
errors or limits) and leveloffset. Relative/absolute leveloffset entries follow the
pinned reader's generated attribute lines and blank boundaries, including literal
retention in verbatim; generated lines have directive origins but no highlight tokens.
Heading levels and doctitle detection use the effective offset; levels above u8::MAX
clamp with a warning, and HTML uses saturating arithmetic. Preserve physical spans,
attribute snapshots and restoration semantics when extending this path.
Includes also support indent and depth. Indent runs after selection and before
preprocessing, with optional tab expansion from tabsize. Reader line fragments map
expanded tabs to their physical byte and exclude synthetic padding from highlighting.
Physical snapshots never change. Indentation growth (including intermediate tab
expansion) is preflighted and charged to the include byte budget. Depth is scoped to
an input: a child override can extend an inherited limit but never the host maximum;
returning to a parent restores its limit. Reader normalization also runs without
indent: primary and AsciiDoc inputs rstrip ASCII whitespace/NUL, while raw includes
only chomp. Unicode spaces and internal tabs survive this stage. Only primary input
condenses final empty lines; includes preserve block boundaries. Removed suffixes
and retained line endings have separate physical fragments; the tokenizer's plain
reader must not normalize processed raw content again. Verbatim tabsize substitution
outside include indentation remains an explicit XFAIL. Multiline attributes now use
one parser-owned collector for header/body: space-backslash and legacy space-plus,
marker consistency, blank/EOF termination, locked/unset consumption, and cross-source
continuations. Preprocessing sees the old context until collection finishes. Reader
records consumed attribute lines and values for tokenizer classification; preserve
physical spans and avoid independently interpreting continuation text as syntax.
Hard-break substitution after attribute expansion and empty-container attribute replay
remain explicit XFAILs (the latter also affects single-line entries).
ResourceProvider::document_path optionally supplies a slash-separated absolute
path for primary docfile/docdir/docname/docfilesuffix attributes. MemoryResources
and CLI opt in; opaque providers default to None. Resource parsing/highlighting
locks these attributes before the header, overriding caller values and preserving
them across includes. Keep metadata separate from canonical referrers, never reload
the primary snapshot, and do not infer cwd or paths in string APIs. Side-effecting
substitutions and safe-mode compatibility are follow-up work; see the
implementation plan and parser API. Include parameter lists now follow the pinned
AttributeList grammar, including ignored positional/unknown fields, quote recovery,
duplicate/None rules and options merging/spacing. Encoding remains unsupported by
the UTF-8 provider contract. Ordinary/intrinsic attribute substitution runs once
before parsing the list, with both reference escapes and attribute-missing policies.
Single quotes do not run inline substitutions here: the pinned reader omits
sub_result. Missing-attribute warnings map to physical include directives; preserve
this mapping through selection/indentation. Targets now share this single-pass
substitution grammar, with the pinned reader's
missing-reference rules: warn uses drop-line and warns instead of loading an empty
result unless optional; drop-line skips missing targets before parameter parsing.
Blank expanded targets never call the provider and are handled before encoding/depth
validation. Diagnostics retain physical directive spans after selection/indent.
Partial output still retains the original directive rather than Ruby's path-bearing
fallback. set/counter side effects remain follow-up work.

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
(`assign_captions`, in `resolve.rs`, walking in block-completion order, children first)
numbers every **titled** `Example` block — an untitled example is skipped and does
*not* advance the counter, matching asciidoctor — and stores the resolved prefix on
`Block.caption` (`Option<String>`, a ready-to-render `"Example 1. "` like
asciidoctor's `@caption`). The prefix word comes from the `example-caption`
attribute in the block's completion snapshot (default `Example`, overridable with
`:example-caption: …`). An unset prefix suppresses the caption and counter increment. The HTML
backend prepends it inside the `<div class="title">` (`#{caption}#{title}`), and
JSON/WASM serialize a `caption` field (the string, or `null`). Only example blocks
are numbered: `listing-caption` stays unset (a titled listing/literal keeps its bare
title) and `figure-caption`/`table-caption` are moot until image/table blocks exist.
Snapshotted by `golden/examples/caption_numbering.adoc` and the golden oracle no
longer strips the counter. **Quote and verse blocks** are the next type-changing
styles. A `____` delimited block defaults to `BlockKind::Quote { attribution,
blocks }` — a parsed *container* like `====` (body is blocks; inline subs, nested
blocks, anchors and `<<id>>` apply, the resolve pass recurses in) — and a `[quote]`
style on a paragraph wraps it the same single-child way. A `[verse]` style turns
either form into `BlockKind::Verse { attribution, inlines }`, which is **verbatim**
(source line breaks preserved) yet still inline-substituted: the new
`scan_inlines_with(…, fold_newlines=false)` keeps the newlines (the regular
`scan_inlines` folds them to spaces) while `*bold*` still formats, and the HTML
backend renders the body in a `<pre>`. The content model is chosen at the `____`
fence from the pending style threaded into `parse_block`. Both kinds carry an
optional `Attribution { author, citetitle }` (each an inline `Vec`, asciidoctor
applies subs) folded by `apply_style_fixup` from the positional attributes
(`[quote, author, work]`, consumed off `attrs.positional`); an empty leading slot
is preserved so `[quote, , Work]` is citetitle-only, matching asciidoctor —
`parse_block_attr_list` now keeps empty positional slots past the style word (its
attribution inline spans are best-effort, based at the block start). An admonition
style on a `____` block (`[NOTE]` on `____`) still wins, moving the quote's children
into an `Admonition`. HTML renders the **modern HTML5 idiom** — `<figure
class="quoteblock"><blockquote>…</blockquote><figcaption>&#8212; author
<cite>work</cite></figcaption></figure>` and `<figure class="verseblock"><pre>…
</pre>…</figure>` (id/roles on the `<figure>`, the em dash only with an author, a
citetitle-only attribution rendering just the `<cite>`) — *not* asciidoctor's
`<div class="quoteblock">` / `<div class="attribution">` layout; `adoctra.css`
ships `.quoteblock`/`.verseblock` rules. JSON/WASM serialize `name:"quote"` /
`"verse"` with an `attribution` object (`author`/`citetitle` inline arrays or
`null`) plus the nested `blocks` / `inlines` field; the tokenizer emits a
`quote-delimiter` token for the `____` fence (the body keeps its normal tokens, as
for `====`). Captions do not apply (only example blocks are numbered), but the
caption pass recurses into a quote so a titled `Example` *inside* a `____` block
still numbers. Snapshotted by `golden/quotes/` and `golden/verses/`. **Sidebar
blocks** are the next container, a near-exact mirror of the example block: a `****`
delimited block (and the `[sidebar]` style on a paragraph) yields
`BlockKind::Sidebar { blocks }` — body parsed *as blocks* via `collect_blocks`, so
inline subs, nested blocks, anchors and `<<id>>` all apply, the resolve pass
recurses in, and a titled `Example` *inside* a `****` block still numbers. The
whole-line `****` check sits before list detection, so it is distinct from the
`**` unordered marker run (a `**** item` line stays a 4-level list item). Unlike a
`====` example, an uppercase admonition style on a `****` block is **inert**:
asciidoctor leaves `[NOTE]` on `****` a plain sidebar, so `apply_style_fixup` never
folds a sidebar into an `Admonition` (the style stays on `Block.attrs.style`). The
HTML backend renders the **modern semantic** `<aside class="sidebarblock">` (a
sidebar *is* an aside) wrapping the children — *not* asciidoctor's nested `<div
class="sidebarblock"><div class="content">` — with id/roles on the `<aside>` and the
`.Title` just inside it; `adoctra.css` ships a `.sidebarblock` rule. JSON/WASM
serialize `name:"sidebar"` with a nested `blocks` array, and the tokenizer emits a
`sidebar-delimiter` token for the `****` fence (the body keeps its normal tokens, as
for `====`). Snapshotted by `golden/sidebars/`. **Open blocks** are the last
delimiter-container, and the *generic* one: a `****`-style mirror, a `--` delimited
block (and the `[open]` style on a paragraph) yields `BlockKind::Open { blocks }` —
body parsed *as blocks* via `collect_blocks`, so inline subs, nested blocks, anchors
and `<<id>>` all apply, the resolve pass recurses in, and a titled `Example` inside a
`--` block still numbers. The whole-line check is exactly two hyphens (`trimmed == "--"`),
so it is distinct from the inline `--` (em dash, scanned *after* block detection inside
paragraph text) and from `---`/`----` (the latter already a listing fence). What sets the
open block apart is **masquerading**: a *container* style applied to a `--` block
"becomes" that kind, folded by `apply_style_fixup` *after* the fence has produced an
`Open` — `[quote]` → `Quote` (the positional attribution fold is reused, so
`[quote, author, work]` on `--` works), `[example]` → `Example`, `[sidebar]` →
`Sidebar`, and an uppercase admonition style (`[NOTE]`, …) → `Admonition` (unlike a
`****` sidebar, where the admonition style is inert — on a `--` block it folds, matching
asciidoctor). The **verbatim** masquerades (`[source]`/`[literal]`/`[verse]` on `--`) are
**also wired**, via a *content-model switch at the `--` fence* (mirroring how `____` picks
Verse over Quote): `[source]` → `Listing` (the `source` style and folded `language` ride on
`attrs` and drive the highlight rendering), `[literal]` → `Literal` — both verbatim with no
inline subs — and `[verse]` → `Verse` — verbatim line breaks but inline subs applied, with
the positional attribution (`[verse, author, work]`) folded in exactly as for `____`. Unlike
the *container* masquerades these never reach `apply_style_fixup` as an `Open`: the fence
produces the verbatim kind directly (a shared `capture_verbatim_body` helper does the raw
body capture for both the `____` verse branch and the `--` masquerades), so the backends and
tokenizer need no change — they already render `Listing`/`Literal`/`Verse`.
The HTML backend renders a single `<div class="openblock">` wrapping the children (the
no-wrapper convention; asciidoctor nests `openblock`/`content` divs), `adoctra.css` ships
a minimal transparent `.openblock` rule, JSON/WASM serialize `name:"open"` with a nested
`blocks` array, and the tokenizer emits an `open-delimiter` token for the `--` fence (the
body keeps its normal tokens, as for `====`). Snapshotted by `golden/opens/`, with the
masquerade rendering pinned by `open_masquerade.adoc` fixtures dropped into
`golden/{quotes,examples,sidebars,admonitions,blocks,literals,verses}/` (each reuses that
kind's golden test — the last three for the verbatim `[source]`/`[literal]`/`[verse]` forms).
**Passthrough blocks** complete the delimited-block family and are the first
*truly raw* block: a `++++` delimited block (and the `[pass]` style on a paragraph,
plus `[pass]` as a fourth **verbatim** masquerade on `--`, switched at the `--`
fence beside `[source]`/`[literal]`/`[verse]`) yields `BlockKind::Passthrough {
text }` — verbatim like `Literal`/`Listing` (raw `text`, no inline subs, so
`*bold*` and `{attr}` stay literal) but rendered **completely raw** by the HTML
backend: `out.push_str(text)` with **no escaping and no wrapper element at all**
(asciidoctor's `:pass` context), so even a block id/role is dropped on output —
the *only* block where id/roles do not surface. The body capture reuses the shared
`capture_verbatim_body` helper; the `[pass]` paragraph form mirrors `[literal]`
(raw source slice in `apply_style_fixup`). JSON/WASM serialize `name:"pass"` with
the raw `text` (id/roles kept in the model for completeness — only the HTML render
drops them), and the tokenizer emits a `pass-delimiter`/`pass-text` pair, treating
`++++` as a verbatim region like `----`/`....` (a `----`/`....` line inside a
passthrough is passthrough text). Snapshotted by `golden/passthrough/` (the
`masquerade.adoc` fixture pins `[pass]` on `--`). Other styles
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
`golden/examples/`, block-title fixtures under `golden/titles/`, quote/verse
fixtures under `golden/quotes/` and `golden/verses/`, open-block fixtures under
`golden/opens/`, and passthrough fixtures under `golden/passthrough/`).

Scaffold only: DOCX backend; the extension API exists but `parse`/`convert` do
not invoke it yet.

Not yet implemented (common asks — treat as roadmap, do not assume present):
broader language coverage (tables, footnotes, includes,
block/inline macros). The full block-attribute list is now *parsed* (style, named
attributes, `%options`, id/role folding), but most of it has nothing to render
against yet: a few **styles that change a block's type** (`[stem]`/`[latexmath]` →
math, …) remain unimplemented — `[source,<lang>]`, `[literal]`, `[pass]`,
`[example]`, `[sidebar]`, `[quote]`/`[verse]`, `[open]` and the admonition styles
are wired. The **passthrough block** (`++++` / `[pass]`) is done — verbatim and
emitted *completely raw* (no escaping, no wrapper, id/roles dropped). The **open
block** (`--` / `[open]`) is done, including *both* its *container* masquerades
(`[quote]`/`[example]`/`[sidebar]`/`[NOTE]` on `--`) and its **verbatim**
masquerades (`[source]`/`[literal]`/`[verse]`/`[pass]` on `--`, switched at the
`--` fence into a `Listing`/`Literal`/`Verse`/`Passthrough`). Also unimplemented is
**rendering of options** (`%hardbreaks`, `%nowrap`, …), which are captured but
not yet acted on. For
**admonitions**, the *paragraph* forms (`[NOTE]` style and `NOTE: ` label) and the
**delimited form** (`[NOTE]` on a `====` block) are done. **Block titles**
(`.Title`) are parsed and rendered on every block (on `Block.title`); still missing
here: admonitions on list items. **Caption numbering** is done for **example
blocks** (`Example N.` via the `example-caption` attribute, on `Block.caption`); the
`Figure N.` / `Table N.` counters await image/table blocks. The unset forms
`:!example-caption:` / `:example-caption!:` disable example captions and their
counter increments; there is still no custom per-block `caption=` attribute
override. Also: **indented/offset
literal** (leading-space lines) and delimiters longer than four chars (the same
single-length limitation applies to `====`/`____`: a nested same-length `====` /
`____` closes the outer block); **anchors on individual list items**, TOC generation,
incremental/block reparse, and a pull/event API over the AST.

When asked to "add a feature," check this list first and say plainly whether it
is new work or wiring up an existing seam.

## Reference

A GitLab project, `fdc-marketplace`, is the practical reference for broader
AI-assisted development conventions (code review, CLAUDE.md style, plugins, RAG
document management with SVN).
