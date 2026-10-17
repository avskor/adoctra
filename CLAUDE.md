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
Text/Raw and Open/Close(AttributeFormat) parts through substitutions and copy-on-write
snapshots; get/iter remain flat text APIs without semantic markers, raw_parts exposes
all boundaries. AttributeRef.parts and optional
JSON/WASM parts preserve these boundaries; WASM also exports attribute_parts.
HTML escapes Text and emits Raw unchanged. Host values are already substituted;
source whole-value pass supports none, a/attributes, c/specialcharacters/
specialchars, r/replacements, q/quotes with roles/id and link m/macros, with
ordered unique stages. Pass replacements
reuse the lexical stream but preserve generated entity spellings as Raw, so a
later c stage can protect them. Plain inline replacements still use Unicode.
A shared lexical replacement stream
(replacements.rs / substitution_text.rs) processes adjacent Text/AttributeRef
nodes after expansion, preserving protection, ownership and source spans. It
applies the pinned replacement stages in order, with entity restoration last;
entities are not decoded or rescanned. Attribute references keep their token span
even if an earlier node consumes their entire value (Some("")). Replacements
starting in Text span the consumed source construct. Inline::Raw carries restored
source entity syntax; HTML emits it directly and JSON/WASM expose name:"raw".
Definitions and attribute snapshots remain untouched. Formatting/macro nodes bound
runs and their children are processed separately. Whole-value pass quotes use
attribute_quotes.rs and semantic boundaries for strong/emphasis/monospace/mark/
superscript/subscript. Their temporary reference spelling preserves later c/a/r
stages without storing generated tags as Raw. JSON/WASM parts use kind:open/close
and a format name; HTML chooses the tags. Quotes handle single/double markers,
escapes, typographic quotation marks and formatting attributes (roles/id).
OpenWithAttributes carries lexical id/role values (including entity protection);
AttributeFormat::Span represents unquoted spans and metadata on typographic quotes.
JSON/WASM export optional id/role fields on open parts. Late a/r stages preserve
metadata; c protects the whole lexical representation. Replacements reconstruct
only tracked generated boundaries, never arbitrary raw tags. Quote attribute lists
expand references eagerly; ordinary inline a-stage now supports intrinsics and
both escape positions. Definitions and selected pass a-stages now share the
lexical resolver in inline_attributes.rs. Whole-value m/macros supports link:,
mailto:, URL and email through attribute_macros.rs. OpenLink(AttributeLink) /
CloseLink preserve semantic targets, id/role/title/window/rel and label parts;
JSON/WASM use open-link/close-link. Fields keep lexical entity protection just
like quote metadata. Link params support AttributeList, URI-encoded mail subject/body,
windows and relation options; single-quoted params rerun substitutions with a
shared 32-level pass/link recursion bound and diagnostics for cyclic host values. Late a/r/c stages, author
derivation and snapshots retain their semantics. Implicit xref labels strip
generated attribute link boundaries from the clone without mutating headings.
Post-attribute link macros now share the lexical matcher across adjacent
Text/Raw/resolved AttributeRef nodes after replacements. Generated content starting
in source text uses Inline::Substituted { value, parts, span }; parts retain neutral
boundaries, and JSON/WASM use name:"substituted". Attribute-owned results stay in
AttributeRef; consumed references remain empty with their original spans/tokens.
Source-owned results cover consumed constructs, including source provenance across
includes. Definitions/snapshots do not change. Direct links now use this same late
matcher, including target/parameter substitutions, escapes and formatted labels.
Replacements and macros retain one fragment map until AST write-back, preserving
precise link spans after length-changing replacements. Source quotes are flattened
temporarily into semantic boundaries and surviving pairs are rebuilt into their
original nodes. Syntax highlighting retains consumed quote/attribute tokens and
uses link spans reported by the matcher. Ordinary source quotes and whole-value
pass:q now share attribute_quotes.rs with ordered stages, single/double markers, escaping,
roles/id and typographic quotes. inline_quotes.rs maps the result to source edits,
rebuilds undecorated AST quote nodes, and preserves decorated semantic boundaries
in Substituted. Source typographic characters remain Unicode Text. Metadata-
consumed attribute references remain empty nodes with physical spans/tokens.
Eligible closers are indexed for incomplete editor input. Source quotes use the
block's effective compat-mode; attribute-entry pass:q uses the cached mode from
host overrides/context creation and header completion. inline_pass.rs extracts
protected regions before quotes and restores them after the outer substitutions.
It supports +/++/+++, $$, compat backticks, roles/id, escapes, legacy x- and inline
pass: with c/q/a/r/m/p, normal/n, verbatim/v, specialchars and none. The result uses
Substituted Text/Raw and neutral boundaries; private collision-free slots never
escape unchanged to the final AST, link targets or metadata. Pinned Ruby mailto
subject/body encoding retains percent-encoded slots; preserve its explicit regression.
Attribute expansions do not
restart extraction. Physical spans include the consumed delimiters and compose
through includes. Tokens use pass-text or monospace and omit protected inner
syntax; the existing line-based tokenizer limitation still affects multiline
constructs. pass_subs.rs shares group expansion, ordered deduplication, -name
removals and warnings for invalid stages with whole-value attribute pass. Ruby
rejects basic; verbatim/v is special-character protection without callouts.
Selecting macros extracts nested passthroughs before every chosen stage; reserve
all outer slots before nested evaluation. Limit nesting to 32 with a warning and
literal raw remainder. Diagnostics use physical selector spans or enclosing
macro/attribute spans after transformations, mapped through reader/comment provenance.
AttributePart::LineBreak represents p/normal hard breaks; HTML renders br and
JSON/WASM use kind:line-break. Later c can protect the lexical spelling. Preserve
LF/CRLF and hardbreaks-option behavior inside pass.
inline_attributes.rs runs the ordinary a-stage over the lexical quote result,
with Unicode Word reference names, leading/trailing escapes, case-insensitive
intrinsics, document/host override priority, and skip/warn/drop/drop-line policies.
Declarations now preserve the same Word characters, including combining marks.
Generated values are not rescanned. Intrinsics retain Raw entity spelling and
ordinary AttributeRef ownership. Removed refs remain Some("") with their tokens;
unknown retained refs remain None and still participate in late link matching.
Drop filters the transformed lines, including generated newlines, and removes
only lines made empty by missing values; drop-line removes any affected line.
Warnings (warn) and Info diagnostics (drop-line) use physical reference spans,
including selected/indented includes. A retained failed include can also warn
about its literal missing reference. Quote metadata retains semantic boundaries
through late expansion. Eager metadata now uses the same lexical a resolver
and emits diagnostics with mapped origins.
WASM preserves source reference nodes and diagnostics; JSON keeps its existing
resolved-text projection with optional parts. Stem and other macro families
remain incomplete.
The shared parts entry point runs a over the complete Text/Raw/semantic stream,
including late quote/link metadata and references spanning adjacent parts.
Normal definitions start with special-character protection; selected pass stages
retain their requested c/q/a/r/m/p order. Definitions use the current missing
policy before assignment, even when assigning attribute-missing itself. Empty
results are assigned, not unset. Locked definitions are skipped before evaluation.
Definition diagnostics cover the entire physical declaration (including joined
lines); selected or nested pass diagnostics use the enclosing physical construct.
Reader provenance composes these origins across selected/indented includes.
String-only header helpers retain skip behavior without fabricated diagnostics;
eager quote metadata and repeated single-quoted link parameters now share
the ordinary missing policy and diagnostic sink. Source definitions preserve intrinsic Raw
parts, while later outer replacements may restore entities protected by a late c;
inline pass restores after those outer stages. Preserve that lifecycle difference.
Ordinary inline post replacements now share post_replacements.rs with pass:p.
They run over the same mapped lexical stream after r/m and before passthrough
restoration. Explicit ` +` becomes AttributePart::LineBreak at logical line ends,
including the final line; hardbreaks-option breaks only newline-terminated lines
and leaves the final marker alone. CRLF retains its physical range. Attribute-owned
breaks stay in AttributeRef; source-owned breaks use Substituted with marker/newline
spans. Definitions and snapshots remain unchanged. Document/host hardbreaks aliases
to hardbreaks-option before locking/unsetting. Local %hardbreaks/options=hardbreaks
and named hardbreaks-option affect this block's inline body, not child containers,
list item principal text or the following block. A transient local_hardbreaks
flag travels only in the inline context clone; named lookup, including pass:a,
still sees document/host attributes. Verse keeps newlines.
TokenKind::LineBreak (line-break) projects standalone source-owned breaks from the
full parsed AST, including local options, final lines and include provenance.
Attribute-owned breaks keep attribute-ref and protected regions keep pass-text;
embedded link/formatting results retain their existing outer tokens.
Quote metadata uses mapped lexical slices, preserving special-character
protection, Unicode names and both reference escapes. Source metadata warnings
point to the physical reference; late a may warn again on the generated opening
boundary. These are distinct executed stages, not duplicate diagnostics to remove.
Passthrough metadata and selected q/m stages use the enclosing construct's span;
definitions use the whole declaration. Drop-line filters only the current metadata
or parameter value, not the surrounding formatted text or link.
AttributeList's title/reftext/opts/options fields do not rerun substitutions;
a parsed single-quoted value beginning with an apostrophe also stays literal.
Eligible positional/named fields repeat the full c/q/a/r/m/p pipeline for the
implemented macro families. Passes owns a shared slot pool, diagnostics and depth
budget across link parameters and selected pass stages. Extract before c/q and
restore after p; preserve source slots and their observable mailto numbering.
Unescape macro-label closing brackets once, before AttributeList quote unescaping.
Repeated parameter passes use the enclosing physical link origin; they do not
create pass-text or line-break tokens for generated syntax. Only slots reserved
during source extraction produce passthrough tokens. Nested link/pass warnings
retain the outer physical origin, mapped through include provenance. At depth 32,
keep the remaining text and emit a warning instead of restarting the depth count.
Attribute definitions now execute set/counter/counter2 through a scoped mutable
Evaluation (substitution_context.rs), shared by q/a, selected pass stages and
repeated link parameters. Body text (paragraph/list/verse) executes in a separate
conversion phase after reading, implemented in body_subs.rs. Defer source fields
until styles have selected the surviving body; replay evaluated declarations,
including equal-value assignments and unsets, in document order. The syntax-only
highlighting scan remains read-only and projects executed expressions from AST.
Definitions and body evaluation respect host locks and preserve the
header snapshot; counter names are case sensitive while ordinary references and
set names are normalized. Canonical integers increment without a fixed-width
limit; string seeds follow Unicode successor/carry rules. set supports empty
values, unsets, aliases and relative leveloffset; derived backend/doctype behavior
remains outside the current built-in attribute profile.
Stateful pass bodies execute lazily at restoration, after outer stages. A dropped
slot has no body effects, while repeated slot occurrences execute independently
(e.g. a link target and its fallback label can advance the same counter twice).
Keep the shared 32-level bound and enclosing physical diagnostic origin.
ParseContext keeps reading attributes separate from conversion replay after the
header. Explicit declaration results/unsets enter source-ordered replay, without
the effects from computing them. Section titles execute stateful substitutions once during reading;
IDs and xrefs reuse their resolved inlines. Effects reach later reading but do not
enter declaration replay. The implicit doctitle executes an early a-stage with a
fixed skip policy before header completion. The displayed document title executes
normal substitutions after reading, before body replay, starting from the header
snapshot; locked counter state retains reading effects. It can therefore differ
from the doctitle attribute. Body paragraphs/list/verse share its conversion state.
Header overrides retain their enclosing declaration/host origins; their pending
evaluation is separate from physical body ranges to avoid synthetic-span collisions.
Tokens project source expressions from AST, suppressing generated ones whose origin
is a doctitle declaration. Dot block titles retain source until completion:
an explicit ID plus a literal `{` triggers reading-time normal substitutions;
other rendered titles execute before content during conversion. Overwritten and
orphan titles do not execute. Passthrough titles are omitted from HTML; uncached
ones retain raw source text in AST without inline evaluation or warnings.
A named `title=` from an attribute list shares the title key with `.Title`
(last wins) and never requests `sub_result`; a value changed by the line's early
a-stage is rescanned from its lexical spelling with the field's physical range
(`PendingTitle.origin`). Like Ruby, a block title before a section heading is
carried to the next content block (through further sections; a later title
replaces it); a discrete/float heading discards it.
Example captions are assigned before title caching, in child-before-parent order,
using the shared example-number counter. Its value replays on block entry before
title/content, including locked counters, independently of explicit declarations.
Only reading-time effects change subsequent caption prefixes. Local/document
caption values (including empty strings) suppress numbering. Caption remains a
flat string; rich/raw caption values, other caption families (including the
admonition label from `caption=`) and remaining derived settings are follow-ups.
Block-anchor reftext (`[[id,text]]`) executes the early a-stage during metadata
collection, including overwritten and orphan values. Header collection consumes
initial block metadata once before saving attributes, then transfers it to the
document title or first body block. Body reftext effects affect reading only,
without declaration replay. Formatting uses c/q/r (snapshot and per reference),
preserving reference owners and physical spans; attributes are not rerun and
macros/passthroughs are not executed. Empty expanded labels fall back to a section
title or `[id]`. Header reftext also retains its raw lexical document attribute.
Reader records early reference spans even for discarded metadata, so tokenization
does not need to execute it again. A named `reftext=` shares the anchor's key
(last wins): its field from the line's early a-stage becomes the same `reftext::Read`
(reference owners and spans kept), so formatting and header transfer are shared.
Block.reftext is a read-only c/q/r snapshot at completion (no diagnostics);
references format the read value again during conversion (see below).
Inline anchor *display* is recognized by the macros stage (attribute_macros.rs
apply_anchors, Ruby's InlineAnchorRx after links/email) over the lexical result of
quotes/a/r: expanded values can create or break anchors, the lazy reftext may
reach into a value and consumed quote boundaries are not rebuilt. Source-owned
anchors are `Inline::Anchor { id, span }` (no reftext field: labels come from the
catalog); consumed refs stay empty after it. Value-generated anchors are
`AttributePart::Anchor(id)` (HTML `<a id>`, JSON/WASM kind:anchor), dropped from
borrowed xref labels like link tags; pass:m and link parameter passes share it.
Cross-references are the last macro (attribute_macros.rs apply_xrefs, Ruby's
InlineXrefMacroRx after anchors) over the same lexical stream: `<<{id}>>` takes the
expanded id and values may open, close or wholly create references. Escapes drop
the backslash; outside compat-mode a leading `#` is stripped; the angle label is
left-stripped (blank → textless); the macro label unescapes `\]` and with `=` is an
AttributeList (first positional = label, `role`; named-only → textless). Source-owned
references are `Inline::CrossReference { xref, children, span }` (children:
Text, or one Substituted over the label span when it holds boundaries); consumed refs
stay empty after it. Value-generated ones are `AttributePart::OpenXref(Xref)`/`CloseXref`
(lexical values; HTML `<a href>`, JSON/WASM open-xref/close-xref); `Xref` fields of
the node are decoded. A textless pair gets its label parts and refreshed flat value from
resolve::fill_labels (body_subs at conversion, resolve_reading for headings), with
the unresolved warning (internal ids only) on the reference span. pass:m and
repeated link parameter passes also run the xref stage (OpenXref parts in
Substituted, warnings on the enclosing construct). References generated by an
attribute definition are tagged (pub(crate) `Xref.definition`, ParseContext.definitions
with the reading attributes); resolve::register fixes their natural lookup and label at
the definition's source position (merged with anchor flushing, partial registry, like
Ruby converting while defining) and every use (headings at reading, body_subs) takes the
fixed result via apply_fixed; unresolved warnings stay per use. Targets follow Ruby's branches (xref_target.rs):
`Xref.target` is the href, `refid`/`path`/`fragment` as Ruby's attrs; `doc.adoc#id`
/ `xref:doc.adoc[]` become `relfileprefix + path + (relfilesuffix || outfilesuffix
pinned at header end, built-in .html)` unless the path is `docname` or a full include
(Reader.include_catalog, keyed relative to the document dir via document_path;
lines/tags selections are partial except `tags=**`). `window`/`opts` constraints
apply only to interdocument refs. A bare unknown id with a space/uppercase is a
natural xref (pub(crate) `Xref.natural`, cleared once looked up): conversion builds
Ruby's `@reftexts` once (lexical label spelling, registration order via
Target.order); headings at reading search the partial registry uncached. Textless
labels: `path`, or for `#`/this document its reftext, doctitle or first top-level
section, else `[^top]`. Character references inside decoded node fields are not
expanded; the `title` attribute as doctitle remains work. xrefstyle (macro
`xrefstyle=` in `Xref.xrefstyle`, else the document attribute at the reference) is
resolve::styled over the converted title: Target.numbering = Caption (example
blocks: `full` → `example-caption numeral, “title”`, `short` → `prefix numeral`,
without a numeral or prefix the caption minus `. `; empty caption → title) or
Section (section_numbers.rs: numbered `full` → `<sectname>-refsig N.M, “title”`
with chapters/appendices emphasized, `short` → `refsig N.M`, unnumbered chapters
emphasized by any style); natural lookup and the document label ignore it. Example
numerals come from assign_caption (ParseContext.numerals).
**Section numbering** (`sectnums`, alias `numbered`): ParseContext::read_section
decides at the title with reading attributes (Ruby's initialize_section: sectname
part/chapter/section or a special style; specials only with `sectnums=all`, as
chapters at book level 1; parts with `partnums`); close_sections runs before the
next title (and at body end) and gives a closed chapter the next `chapter-number`
counter value (reading effect only; conversion restarts from the header like Ruby).
nest_sections assigns ordinals among numbered siblings (Roman for parts) and the
sectnum path (an unnumbered parent contributes ""). body_subs sets the heading's
`Block.caption` (`1.2. `, `<chapter-signifier> 3. `, `<part-signifier> II: `) at
conversion with `sectnumlevels`/signifiers at the section; HTML renders it inside
`<hN>`, JSON/WASM already export caption. Built-in `*-refsig`/`appendix-caption`
defaults match Ruby; every label word is an attribute, so documents localize
them (`:appendix-caption: Приложение`, `*-refsig`, `*-signifier`).
**Special sections** (any non-`sectN` section style; a book's `[abstract]` is a
level-1 chapter instead): level 0 becomes 1 (`Section.level`, also the Heading
level) while the title line's `raw_level` decides nesting, out-of-sequence and
level-0 diagnostics. `special` is inherited by nested sections (numbered only
with a numbered parent); level-1 specials other than appendix/preface/abstract
report "<sectname> sections do not support nested sections". An appendix is
always numbered: closing it takes `counter:appendix-number:A` and fixes its
caption from `appendix-caption` at that point (`Appendix A: `, else `A. `),
shown regardless of sectnumlevels; children continue `A.1.`. Reading-time labels
(headings, definition xrefs) see numerals only of sections closed before that
position (Section.closes/at, Ids.position), like Ruby assigning at close.
**Book parts** (nest_sections part_content, Ruby's next_section): a book part's
content before its first chapter is wrapped in a generated `Open` with style
`partintro` (span of the wrapped content); a plain `--`/`[open]` block becomes
the partintro itself, an `[open]` paragraph is rebuilt into a container when a
second block arrives (its id/roles/title/attrs move to the paragraph); after a
`[partintro]`/`--` intro the next block errors "illegal block content outside of
partintro block"; a part not ending in a section errors "invalid part, …".
`[abstract]`/`[partintro]` paragraphs are open blocks with that style.
check_open_styles reports what Ruby's HTML converter excludes (partintro not a
direct child of a book part: error; abstract directly in a book document:
warning) and HTML skips those blocks; abstract renders `<figure class="quoteblock
abstract">`, other non-masquerading open styles become `openblock <style>`. A
headerless book opens a preamble unless its first block is an abstract and keeps
it without sections; header `preface-title` is the book preamble's `Block.title`
(raw source Text, empty span at the preamble start, never rendered in HTML;
JSON/WASM export preamble `title`). Ruby's simple `[open]` paragraph (text without
`<p>`) remains an XFAIL. Manpage synopsis is not implemented.
**TOC**: built-in `toc-placement=auto`/`toc-title`; header.rs save_toc normalizes
`toc`/`toc2` at header completion like Ruby's save_attributes (toc → "",
toc-position left/right/top/bottom with toc-class toc2, or content with placement
preamble/macro; explicit toc-position/placement wins; unset placement = macro;
bypasses locks; body entries untouched). `BlockKind::Toc` is the `toc::[…]` block
macro (parse_toc_macro, any style, text inside a paragraph): AttributeList fields
without subs; id/role/title replace metadata via ParseContext.block_macro, other
named fields (levels) go to attrs.named. JSON/WASM `name:"toc"`, token
`block-macro`. HTML builds the outline from Section blocks (Cx carries the
document): `<nav id="toc" class="toc">` after the title (header attrs, placement
not macro/preamble, document has sections), at the preamble end (`toc-class`), or
at the macro (its attribute_context; else `<!-- toc disabled -->`). Entries are
heading caption + inlines with every `<a>` tag dropped (DropAnchorRx), nested
while level < toclevels/levels (Ruby to_i). The DOM profile maps nav to div.
Xref labels: resolve::register builds the registry after reading (auto ids,
block ids, cataloged anchors in source order); body_subs Execution resolves each
textless xref after its field's subs, like Ruby's xreftext:
nonempty reftext formatted per reference with the current attributes/counters
(effects persist, warnings repeat), else the block/section title, else `[id]`.
A deferred block title converts once (first reference or the block itself);
references inside a label being produced show `[id]` (@resolving_xref), and a
self-reference converts the title again. Labels unwrap Link/CrossReference and
drop anchors (DropAnchorRx). Auto-id headings and anchored `{` titles resolve
references while reading (unregistered ids give `[id]`, a deferred target
title converts early) and the auto id derives from that result.
Block.attribute_context remains the reading snapshot at block completion;
it can therefore differ from resolved inline values.
Tokenization uses isolated contexts and the same declaration replay; it never
mutates caller attributes or previously returned snapshots.
Early catalog registration of anchors consumed by quote metadata, non-link pass
macros and raw whole-doctitle replacement also remain work.
Automatic section IDs sanitize a temporary lexical title: raw XML tags and
references matching the pinned entity grammar are removed, not decoded.
Literal protection and structural inline boundaries remain distinct; title
nodes, attribute snapshots and source spans are unchanged. Default prefix,
separator and deduplication behavior is retained; configurable ID policy remains work.
The tokenizer shares header parsing (author/revision tokens). Setext document titles use adjacent
processed lines, Unicode character lengths (difference <= 1), uniform =/-/~/^/+
underlines and effective level zero. ATX takes precedence. The Heading/header
title span includes both lines; inline spans cover title text only. Highlight
both lines as separate heading tokens and retain physical source fragments across
includes/selection/indent. Setext installs compat-mode before subsequent header
metadata, respecting locked overrides; its substitution effects remain follow-up work.
Body setext sections share setext_level, but only where Ruby looks for sections
(BlockCtx::Section: document level, not containers or list items); a
discrete/float style accepts them anywhere. ATX wins; the underline wins over
block fences (`----`/`--`/`====`/`++++`); paragraphs are not interrupted; a
comment style peeks the underline raw. Heading spans both lines. ATX titles
follow the same context rule (text inside containers, list items and paragraphs;
paragraphs are not interrupted). The reader records all section title lines in
heading_lines, so highlighting emits heading tokens without its own ATX scan.
**Section hierarchy** (parser.rs nest_sections, Ruby's next_section) runs after
reading the body: BlockKind::Section { level, blocks } with the title Heading as
blocks[0] (id/reftext/roles/attrs stay on it) and BlockKind::Preamble { blocks }
(only with a document title and at least one section, or in a headerless book; a book keeps a standalone
preamble). Section spans cover title through last nested block; snapshots are the
last block's. Discrete headings and the doctitle stay plain Headings. Diagnostics
match Ruby: out-of-sequence warnings (book: "expected levels 0 or 1") and the
level-0-outside-book error; without a header, a body that starts with a section
skips top-level checks. Document::flat_blocks/into_flat_blocks give the old flat
order. HTML renders `<section class="sectN role">` (id on the heading, level 0
unwrapped) and `<section id="preamble">`; JSON/WASM use section/preamble; the DOM
profile treats section as div. Sectnums, special sections, book partintro and
preamble cases are implemented (see above). JSON and WASM expose header metadata;
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
effects in conditional expressions remain unimplemented; effects from preceding
definitions are visible to the reader. Basic include expansion is available with an explicit provider.
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
`xref:id[text]` macro (matched by the macros stage, see above), rendered as an
`<a href>` to the resolved target (`#id`, interdocument path or natural title id), with inline markup kept inside the reftext; a **post-read registry pass**
(`adoctra-core/src/resolve.rs`, the first pass over the *whole* document — needed
because a reference may point forward; labels resolve during conversion) builds a registry of **auto-generated
section ids** (asciidoctor's `sectids`: lowercase, non-alphanumerics dropped or
folded to `_`, `_`-prefixed — `== Hello World` → `_hello_world`, with `_2`/`_3`
deduplication) — for every heading except the document title (`Document.has_title`
marks `blocks[0]`), so discrete (`[discrete]`/`[float]`, rendered with the style plus
roles as class and never a section) and body level-0 headings (parts, `sect0`) get
ids too — and resolves each reference: a textless `<<id>>` / `xref:id[]` takes
the section's (or a titled block's) title as display text (inline formatting preserved, inline anchors
dropped; the document title lends its text to an explicit id), an explicit
reftext is kept verbatim, and an unknown id emits a `Warning` diagnostic and
renders the `[id]` *unresolved* fallback (asciidoctor-compatible). A trailing
` [[id]]`/` [[id, reftext]]` on a section, discrete or document title (Ruby's
`InlineSectionAnchorRx`, leftmost match, preceding space required, escape keeps
it) is removed from the title and names it unless metadata already supplied an
id; its raw reftext gets c/q/r without the attribute stage and replaces a pending
reftext (the document title ignores it). **Explicit
anchors** join the same registry: a *block anchor* (a whole-line `[[id]]` /
`[[id,reftext]]`) attaches its id and inline `reftext` to the next block,
overriding a section's auto id; an *inline anchor* (`[[id]]` / `[[id,reftext]]`
sharing its line with text, or the `anchor:id[reftext]` macro) renders as an empty
`<a id>` at that point, its `reftext` feeding a textless `<<id>>` — but only
anchors *cataloged while reading* (`anchor_catalog.rs`, Ruby's
`catalog_inline_anchors`/`InlineAnchorScanRx`) join the registry: ordinary
paragraphs (including `NOTE:` label paragraphs and paragraphs inside containers;
not `[NOTE]`/`[quote]`/`[example]`/`[sidebar]`/`[verse]`-styled ones) and an
anchor *leading* a list item. Their reftext runs the stateful early a-stage with
reading state (an empty expanded shorthand reftext skips the anchor; `\]` is
unescaped in `anchor:`), formatted with c/q/r; the registry keys them by physical
start and registers leftovers whose display is not an anchor. Escaped
`\[[…]]`/`\anchor:` render as literal text without the backslash; a known id with
no natural text falls back to `[id]` *without* a diagnostic and a duplicate id
warns once (first definition wins). **Block attribute lists** share that
seam (`block_attributes.rs`): a line matching Ruby's `BlockAttributeListRx`
(`[` in column 0, then empty or a Word/`.#%{,"'` character; `[]` is consumed,
`[[…]]` is only ever an anchor) is tokenized by the shared AttributeList lexer
(`attribute_list.rs`, also used by include parameters and link labels) and
merged into the pending metadata by Ruby's *integer keys*: a field's index counts
named and `None` fields, a later line replaces the same key, and an empty unquoted
field is an explicit nil. Shorthand (`style#id.role%opt`, disabled by a space) is
parsed from the merged first positional *after* the line's named fields, without
validating ids/roles; empty and repeated shorthand ids warn on the line span. A
named `role=` replaces earlier roles, shorthand roles append; `Block.id` takes the
last id. The first positional is the block **style**, `name=value` pairs land in
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
`.admonition-title` and the rendered child blocks. The label is core's
`Admonition.label` (asciidoctor's `textlabel`): a raw `caption=` attribute, else
the document's `<kind>-caption` parts while reading (built-in `Note`/…, empty
when unset) — *not* asciidoctor's
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
emits one `block-title` token per `.Title` line, with executed stateful expressions
projected from AST as attribute-ref. Building on titles, asciidoctor's
**caption numbering** is wired for example blocks: `ParseContext::assign_caption`
(called at block completion, children first, before title caching)
numbers every **titled** `Example` block — an untitled example is skipped and does
*not* advance the counter, matching asciidoctor — and stores the resolved prefix on
`Block.caption` (`Option<String>`, a ready-to-render `"Example 1. "` like
asciidoctor's `@caption`). The prefix word comes from the `example-caption`
attribute at that point (default `Example`, overridable with
`:example-caption: …`). An unset prefix suppresses the caption and counter increment;
a local/document `caption` replaces it without numbering. The HTML
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
optional `Attribution { author, citetitle }` (each an inline `Vec`) rekeyed by
`apply_style_fixup` from Ruby's second and third positional keys (`[quote,
author, work]`, consumed off `attrs.positional`); a nil key is `None` (so
`[quote, , Work]` and `[quote, role=x, Work]` are citetitle-only) while a quoted
`""` is an empty part. Both stages run while reading the attribute line, as in
Ruby: an early a-stage over the whole line (`sub_input`, stateful
set/counter/counter2, missing policies including drop-line, reading effects only,
initial metadata entering the header snapshot) before AttributeList splits the
*expanded* text, then normal subs for single-quoted values (`sub_result`, not
named title/reftext/options). Unquoted and double-quoted attribution is
`Inline::Raw` source plus `AttributeRef` nodes — HTML emits it unescaped, like
asciidoctor; a transformed single-quoted field maps its nodes and diagnostics to
the field's physical range. Other single-quoted fields keep only visible text
(Ruby stores substituted HTML). An admonition
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
parser completes children of a quote so a titled `Example` *inside* a `____` block
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
lives on the block (`Block.id`, `None` for unanchored doctitles and plain blocks)
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
**rendering of remaining options** (`%nowrap`, …); `%hardbreaks` now affects
inline substitutions. For
**admonitions**, the *paragraph* forms (`[NOTE]` style and `NOTE: ` label) and the
**delimited form** (`[NOTE]` on a `====` block) are done. **Block titles**
(`.Title`) are kept on `Block.title` and rendered except on passthrough blocks; still missing
here: admonitions on list items. **Caption numbering** is done for **example
blocks** (`Example N.` via the `example-caption` attribute, on `Block.caption`); the
`Figure N.` / `Table N.` counters await image/table blocks. The unset forms
`:!example-caption:` / `:example-caption!:` disable example captions and their
counter increments; a custom per-block `caption=` or document `caption` overrides
the prefix without incrementing the counter. Also: **indented/offset
literal** (leading-space lines) and delimiters longer than four chars (the same
single-length limitation applies to `====`/`____`: a nested same-length `====` /
`____` closes the outer block); **anchors on individual list items**,
incremental/block reparse, and a pull/event API over the AST.

When asked to "add a feature," check this list first and say plainly whether it
is new work or wiring up an existing seam.

## Reference

A GitLab project, `fdc-marketplace`, is the practical reference for broader
AI-assisted development conventions (code review, CLAUDE.md style, plugins, RAG
document management with SVN).
