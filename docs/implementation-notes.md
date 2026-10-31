# Implementation notes

Detailed implementation status of adoctra, moved out of `CLAUDE.md` so it is
read on demand instead of loaded into every AI session. The short summary,
priorities and hard rules stay in `CLAUDE.md`; this file is the authoritative
record of *how* each feature is implemented and which Ruby behavior it ports.
Search it by topic (e.g. `grep -n '\*\*Tables\*\*' docs/implementation-notes.md`)
before changing an area, and update it when completing a step.

**Project goal (user decision 2026-10-31): convert documents, build the
syntax tree and support the editor. Ruby API compatibility (extension
registries/classes, highlighter factories, logger, parse_header_only, Ruby
objects in options, `-T`/`-E`/`-r`/`-I`) is not a goal; leave those audit
entries UNSUPPORTED. Observable document conversion and CLI behavior are
still compared with Ruby.**

**Current user priority: complete compatibility with pinned Asciidoctor
2.0.23 comes before further performance optimization, incremental parsing,
DOCX or new extension APIs.** Follow the compatibility-first queue in
`docs/implementation-plan.adoc` and the open-gap register at the start of
`docs/compatibility.adoc`. Passing the existing normalized corpus is not proof
of full compatibility. HTML keeps modern semantic markup by design; byte-exact
Ruby HTML is not a goal, maximal visual parity with Ruby's rendering is. The
`visual-v1` profile (`scripts/visual.py`, docs/visual.adoc) measures it: words
with computed style/geometry plus pixels, Chrome via pinned playwright-core,
local environment-bound baseline `compatibility/visual/report.json`. adoctra.css
now carries Ruby's typography, column and block styles (body line-height 1,
lists, TOC, admonition as a CSS table, example box on its `div.content`,
sidebar, quote/verse, tables, dl, media). The corpus also has the 21
acceptance documents (resources in a temp tree) and 4 embedded cases (fragment
plus its own stylesheet), and `golden/rouge/themes.adoc` under all 25 Rouge
themes in class and inline mode (`table.linenotable`/`pre pre` rules ported
from Ruby), plus `golden/roles/roles.adoc` (Ruby's utility roles and
unmarked list styles) and `golden/admonition_icons/files.adoc` (real PNG
admonition/callout icons; image callout rows are Ruby's table cells, the last
list item in an admonition keeps its `p` gap; `subheader` is on Ruby's
wrapper, so only colour/weight reach the element), `golden/admonition_icons/font-files.adoc`
(a vendored Font Awesome 4.7.0 under `admonition_icons/fontawesome/`, MIT +
SIL OFL 1.1, linked with `iconfont-remote!`) and `golden/lists/checklist.adoc`
(Ruby's checklist `-1em`/icon rules and form-control normalize), 14 pages
of other projects (AsciiDoc Language spec 0.3.0, Quarkus 3.40.1, JUnit 6.1.3,
Groovy 5.1.3, also in the acceptance/structure/semantics corpora; performance
workloads stay on `WORKLOAD_ORIGINS`) and `golden/hdlists/horizontal.adoc`:
`[horizontal]` renders `<div>` groups in `<dl class="hdlist">`, laid out as
Ruby's table (`display: table`; one term is a cell, several share an anonymous
cell like Ruby's `<br>`; widths are `--adoctra-hdlist-*width` variables): 694 of 696 observations match; the remaining case
(a raw unclosed `<i>`) is documented. Ruby zeroes last-child margins on
wrappers, so mirror that per element (lists/paragraphs keep theirs).
Re-record only after reviewing an IMPROVED diff; never loosen tolerances. Visual parity, CLI behavior,
diagnostics, resource loading and untested language combinations must remain
visible in the audit. Historical
exceptions (including Rouge) are limitations of any compatibility claim, not
completed requirements. Ruby API/extension execution remains outside the
existing document-processing contract; do not describe that scoped contract
as unrestricted replacement of Asciidoctor. Do not resume allocation
micro-optimizations as the next task. Include encoding now supports Unicode and
53 single-byte codecs, plus Shift_JIS, Windows-31J, EUC-JP, eucJP-ms, CP51932,
Big5, Big5-HKSCS, Big5-UAO, CP950, CP951, GB2312, GB12345, GBK, GB18030, EUC-KR, CP949
and ISO-2022-JP (ISO2022-JP), CP50220, CP50221, stateless-ISO-2022-JP,
ISO-2022-JP-KDDI, stateless-ISO-2022-JP-KDDI, EUC-JIS-2004 (EUC-JISX0213)
and SJIS-KDDI/SJIS-DoCoMo/SJIS-SoftBank, plus UTF8-DoCoMo/UTF8-KDDI/UTF8-SoftBank and CESU-8/UTF8-MAC.
Finite positive `lines` selections recover an intact decoded prefix when only
the unread tail is malformed; `SourceFile::is_complete` exposes this explicitly.
Skipped malformed UTF-8/BINARY lines now have explicit omission records with
raw bytes and physical line numbers. BINARY tag markers are scanned from these
raw lines; diagnostics expose `omitted_span` byte ranges within the omitted line.
Remaining include work is untested encoding/selection combinations and the documented URI/jail gaps.
Include normalization strips a BOM on the first selected line, preserving the
snapshot and byte spans; a BOM on later selected lines is not a leading BOM.

The accepted compatibility roadmap and continuation status live in
`docs/implementation-plan.adoc`; the reference contract is in
`docs/compatibility.adoc`. Read these before compatibility work and update their
status when completing a step. The machine-readable Asciidoctor version is
`compatibility/asciidoctor-version`. Keep known differences explicit; an expected
failure is not evidence of conformance. Preserve the std-only core and existing
crate boundaries while implementing the roadmap incrementally.

`scripts/acceptance.py` runs the separate pinned real-document corpus in
`compatibility/acceptance/cases.json`. Its `body-dom-diagnostics-v1` profile
compares the existing body DOM projection plus ordered warning/error severity
and text, including HTML backend warnings. It is not an ASG or source-position
profile. Fixtures carry SHA-256 and provenance; retain their license notices.
Offline runs use stored JSON references; `--verify-reference` uses the pinned
Ruby launcher with `--report`. Never suppress diagnostic differences to pass:
document them and freeze the complete actual result as XFAIL; drift and XPASS
fail CI. See `docs/acceptance.adoc` for the 21 document runs and 7 diagnostic
probes (28 PASS, no XFAIL), update commands and corpus limitations.
The separate `compatibility/acceptance/structure.json` manifest uses
`block-structure-v1`: Ruby `--structure` projects the loaded parser tree without
conversion; `scripts/structure.py` adapts the CLI's JSON backend. Twenty-one document
runs and five structural probes pass. Keep this profile separate from HTML and
diagnostics; it checks block topology/metadata, lists and tables, not text,
inline semantics, all attributes, source positions or official ASG/TCK.
The corpus includes six full pages each from the Asciidoctor 2.0.23 and Spring
Framework 6.2.0 manuals, pinned by commit and file checksums with licenses,
Antora descriptors and six real include examples. Host attributes are explicit
resolved values; Antora site extensions/catalog resolution are not exercised.
Keep upstream pages unchanged even when they expose parser failures.

`compatibility/upstream/cases.json` is a separate selection of 43 adapted
scenarios from 30 Asciidoctor 2.0.23 tests (20 attributes, 13 substitutions,
10 includes), all PASS in `body-dom-diagnostics-v1`. Full original Ruby test
files, include fixtures and LICENSE are pinned; each case identifies its test
name/line and adaptation. See `docs/upstream-tests.adoc`. The acceptance runner
validates provenance and runs offline/against Ruby in CI. Companion core tests
check physical positions and composite provenance in the upstream include
fixtures. This is not the entire upstream suite or a source-position profile.
Host overrides now accept Ruby's string soft-default (`name@` / `value@`) and
hard/soft unset forms. CLI repeated overrides honor the last spelling of a name;
API callers should supply one spelling per effective key in the BTreeMap.

`scripts/upstream_audit.py` is a differential audit of every document the
pinned upstream test suite builds (all 24 test files vendored unchanged in
compatibility/upstream/sources/test; inventory recorded by
scripts/upstream_record.rb, which runs the upstream suite — a manual step).
Supported entries (recorded options mapped to `-S`/`-s`/`-a`) are compared in
body-dom-v1 plus warnings against pinned Ruby without optional highlighter
gems; others are UNSUPPORTED with a reason. The reference mimics one Ruby CLI
run per document (output chomped + LF, highlighter library status reset).
2342 PASS / 1 DIFF / 1 RUBY_ERROR / 277 UNSUPPORTED of 2621; CI freezes
every status. Output-neutral Ruby API options (sourcemap, catalog_assets,
timings, eruby) pass to the reference only; `backend` (html5/html/xhtml/
xhtml5) maps to a locked `-a backend=`. Remaining UNSUPPORTED: docbook/
manpage backends (243; deferred with DOCX by user decision 2026-10-31 —
do not start them unprompted), extension/logger API (22), parse_header_only (6).
**File output** (Ruby's Asciidoctor.convert/Invoker): entries with
to_dir/to_file run one CLI per side (`-D`/`-o`; Ruby through the reference
script's `--invoke` = Asciidoctor::Cli::Invoker) in the materialized
checkout (input = the docfile fixture when identical, else stdin); written
files are snapshotted, restored and compared (HTML projected plus final LF,
default stylesheets by presence). CLI: `-D`, `-o -`, file
`<docname><outfilesuffix>` next to the input or in `-D`, outfilesuffix from
the `-o` extension, paths via normalize_system_path with `recover: false`
(jail = `-B` or cwd from safe mode up — the CLI default safe jails output
too, user decision 2026-10-31; `-o` within `-D` is jailed in the document
base dir), Ruby's error texts, no final LF, no "wrote" line, copycss paths
like Ruby. Core: `ParseOptions.output` → `Document.output` (OutputTarget/
OutputPaths; `docname` after reading, header-pinned outfilesuffix) and
conversion-only `outfile`/`outdir` below server; `resolve_system_path`
is public.
**CLI matrix** (docs/cli.adoc, `scripts/cli_matrix.py`, profile cli-v1,
compatibility/cli/cases.json + report.json in CI): 62 scenarios run by
adoctra and Ruby's `asciidoctor` (scripts/ruby-cli.rb = Cli::Invoker) in one
temp dir; compares exit code, stdout (HTML projected, usage/version by
kind), normalized stderr and written files; 61 PASS, 1 RUBY_ERROR (Ruby's
uncaught AmbiguousOption). crates/adoctra-cli/src/args.rs ports
OptionParser (long-option/value prefixes, clusters, Ruby error texts; usage
on stdout for errors); glob.rs is Dir.glob; main.rs logs through LOG
(`-q`/`-v`, `--failure-level` max severity; `DiagnosticReport.verbose_only`
= Ruby's `if logger.info?`, e.g. the unlocated INFO `possible invalid
reference: <id>`). `-B` is `ParseOptions.base_dir` (Ruby `:base_dir`);
provider requests stay relative to the referring file. **XML syntax**: `Document.htmlsyntax` is the
converter's syntax at the last header `backend` (Ruby's create_converter;
a later `:htmlsyntax:` changes only the attribute); HtmlBackend renders
Ruby's `@void_element_slash`/`append_boolean_attribute`/`xmlns` through a
per-render XmlMode (`slash()`, `boolean_attr()`); `golden/xhtml/` compares
void tags and boolean attributes. Keep `&`/`<` out of adoctra.css comments
(it is inlined into XML documents). All upstream test/fixtures are vendored (SHA-256 in the
inventory); the harness materializes them in a temp root replacing
`<UPSTREAM>` on both sides and maps `base_dir` to the CLI's `-B`. It serves
the tests' web server at their fixed 127.0.0.1:9876 (a copy of
test_helper.rb's using_test_webserver, confined to test/) during the run;
the Ruby side gives each entry its own open-uri-cached cache_path.
The body-dom-v1 projection maps Ruby's icon colist table and adoctra's
`ol.colist.icons` (span.colist-icon + div) to ol > li > colist-icon/-text.
The reference loads the pinned Rouge (other highlighter gems stay
stubbed); every syntax_highlighter_test.rb entry passes. The last DIFF is
an incomplete inline SVG neither
side can project (raw bodies differ by the intended wrapper). Single-case regressions live in
crates/adoctra-core/tests/upstream_single_cases.rs (compat-mode
`language` → `source-language` in header.rs save_compat_language, FLEXIBLE_ATTRIBUTES
sectnums, `max-include-depth` 64 default attribute, invalid-header, fragment,
AttributeList rekey attribution, folded dlist indentation, placeholders);
the harness marks a document-set non-HTML backend UNSUPPORTED and projects
unknown raw tags as text;
`--show ID` prints both sides. Malformed/mismatched conditional directives
and an unterminated comment block keep short editor messages and put Ruby's
reader message verbatim in `report.message` (Reader::error_reported).
**Inline doctype**: HtmlBackend renders `doctype=inline` like Ruby's
`Document#convert` (render_inline_doctype): only the first block's content
(paragraph, verbatim/raw/STEM, verse or a styled paragraph's simple content),
else Ruby's "no inline candidate; …" warning (typo kept) and empty output;
standalone and embedded alike. The CLI ends non-empty HTML with a LF like
Ruby's CLI and prints backend warnings in embedded mode too.
`compatibility/upstream/semantics.json` uses `inline-semantics-v1`: 129 PASS
cover 20 attribute and four include scenarios from the upstream selection plus
105 original semantic probes (14 tables; 27 xref/indexterm/stem/typographic quote
probes; 33 footnote/callout probes). `compatibility/acceptance/semantics.json` adds
12 PASS on unchanged full Asciidoctor/Spring pages, with the same sources,
include resources and host attributes as the HTML corpus. The Ruby oracle has its own converter (no HTML
converter or HTML parsing); paired synthetic boundaries leave content exposed
to later substitution stages. The Python adapter reads the resolved JSON AST
and selected header/final attributes. CLI `--to json --json-attributes` opts in
to `JsonBackend::convert_with_attributes`; default JSON omits these snapshots.
The schema includes link metadata, UI labels, inline/block images, lists with
checkboxes/description/attached blocks, simple/compound containers and verbatim
content with exact whitespace and language. Tables include column style/width/
autowidth/alignment, head/body/foot, cell spans/style/alignment/background,
paragraph boundaries, literal text and nested AsciiDoc documents. Header cells
have one inline stream; cell styles remain separate from explicit formatting.
CSV/DSV literal cells retain normal substitutions but must not fold newlines;
the parser fix has Unicode/LF/CRLF span, HTML golden, JSON and WASM tests.
Table-wide layout attributes and nested document attribute snapshots are not
compared. Unknown nodes, raw markup,
unsupported link/image options and warning/error output fail the profile.
Implicit xrefs use Ruby's catalog/xreftext and strip nested semantic anchors,
with recursion guards and root/path/id fallbacks. Indexterms retain visible
inlines or null, term hierarchy and see/see-also. Inline/block stem keeps notation
and exact whitespace; typographic quotes preserve formatting and metadata.
Attribute snapshots now complete definition-time xref targets/labels from the
existing fixed-reference registry, without repeating substitutions/counters.
Keep forward-definition fallbacks fixed; test header/final snapshots and use spans.
Footnotes retain markers and ordered root/nested catalogs, including unused or
removed attribute definitions and duplicate counter values. Callouts retain exact
position, number/id, guards/XML, icons/src and ordered colist coids. JSON/WASM
export `text_range` byte offsets into normalized block text, separate from source
spans; adapters must not reparse callout syntax. The core keeps registered
footnotes even without a visible attribute use; lazy outer titles belong to the
outer document even when a table-cell xref first converts them. Extra xref
metadata and embedded SVG/inline icons remain outside the profile. The schema does not cover all nodes, all attributes,
source positions or ASG/TCK. See `docs/semantic-profile.adoc`; keep the independent
39 Ruby converter checks and offline/verify runs for both manifests in CI.

`scripts/performance.py` measures native parse, tokens, JSON serialization and
parse-plus-JSON on all 12 pinned manual pages and 12 deterministic stress inputs.
It builds a release example in adoctra-json with no new dependencies; disk I/O
is outside timers, source registration and destruction are inside. Tokens runs
its own full parse; JSON serialization reuses an AST. Reports retain raw samples,
input/source/artifact fingerprints and host/toolchain metadata. Raw Cargo WASM
size (plus deterministic gzip) is measured before wasm-bindgen, not as a web
package or runtime benchmark. See `docs/performance.adoc` and
`performance/baseline.json`. CI runs a smoke check without timing thresholds;
the separate manual workflow uploads full measurements. Keep benchmarks separate
from concurrent builds/tests, and keep performance claims separate from conformance.
The driver also accepts `--profile-operation parse|tokens`: it runs only that
operation, without preflight parse/JSON, and emits `adoctra-profile-driver-v1`.
Use external profilers on a separate symbolized release build; input loading and
output are visible to them. The substitution-buffer study in
`performance/substitution-buffers.json` keeps repeated timings and glibc
libmemusage counters. Its gprofng timer warnings preclude quantitative CPU
claims; allocation volume is neither RSS nor peak live AST size. In
substitution_text.rs, replacement_origin computes source ownership/coverage
without a temporary Text; restore_boundaries skips rebuilding unchanged
fragments. Preserve zero-length marks, attribute ownership and transformed
metadata when optimizing this stream.
The follow-up `performance/link-stages.json` measures skipping the three link
stages when their current lexical stream lacks the necessary markers, or when
all candidates are malformed/ignored. Check the assembled stream at each stage,
not individual fragments or the original source: attributes and earlier stages
can supply syntax. Keep scanner rules, escaping and source ownership intact.
The gprofng timer warning reproduces outside the sandbox too; a 100 ms probe
fails initialization. Neither probe supplies usable quantitative CPU evidence.
`Attributes` now shares its two ordered tables independently through `Arc` and
copies a table on mutation. Keep `local_hardbreaks` outside this storage; clones
must isolate plain/semantic writes, removals and case-sensitive counters.
Removing an absent key must not detach either table. Public value/parts APIs,
deterministic iteration and snapshot semantics are unchanged. Measurements live
in `performance/attribute-snapshots.json`; do not equate allocation volume with RSS.
The `performance/unchanged-stages.json` study removes whole-stream copies when
attribute expansion, quote formatting, passthrough/STEM extraction or final
line-break substitution makes no match. Use the scanner's consumed position,
not equal output text or an empty result, to detect this: missing references
still report diagnostics, and invisible counter/set expressions have effects.
Check attribute braces in the assembled stream, preserving metadata, source
owners and zero-length parts. The six new core tests cover these boundaries.
The `performance/owned-parts.json` study moves owned strings through semantic
part normalization and lexical-to-AST conversion. `push_value` drops only empty
Text/Raw and merges only adjacent parts of the same kind; boundaries survive.
`Fragment::into_part` gives stored semantic parts priority, then decoded text,
then lexical spelling, preserving raw protection. Save source owner/span before
consuming fragments where needed. Shared-table copy-on-write rules are unchanged.
When writing the AST, newline folding retains the owned string if it has no
CR/LF; raw text and fields with folding disabled stay untouched. Preserve the
existing byte-for-byte replacement of each CR and LF with one space.
`Text::protect_source` reuses the source-only stream after passthrough/STEM
extraction when its assembled spelling contains no `&`, `<` or `>`. It still
resets raw protection and mapping exactly like `append_source`: slot keys can
have the same byte length as their physical spans. This helper is for source
fragments and private slot keys, before semantic parts/marks enter the stream;
do not apply it to arbitrary substitution streams. Inputs containing special
characters keep the previous rebuilding path. The paired measurements and
allocation counters are in `performance/source-protection.json`.

`compatibility/acceptance/positions.json` uses `diagnostic-positions-v1`:
18 original probes, all PASS. Ruby `--diagnostic-positions` exports
primary MemoryLogger source_location file/line or null; the CLI adapter keeps
the same schema, warning order and duplicates. Only stdin and the temporary
fixture root are canonicalized. The default CLI (`--diagnostics compat`) uses
Diagnostic.report presentation metadata, including absent cursors, include
selection line numbering and section lookahead across include EOF. Physical
span/severity/message remain available to API/WASM and `--diagnostics editor`.
Do not normalize diagnostic differences away in the harness. The eight former
XFAILs were fixed and their baselines removed without changing Ruby references.
Columns/byte spans and secondary include_location are outside this comparison;
native include tests check physical ranges/Unicode columns separately.
See `docs/diagnostic-positions.adoc`; retain the strict schema, six Ruby exporter
checks and offline/verify CI runs. Diagnostic struct literals now need
`report: Default::default()` unless explicitly setting presentation metadata.

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
values, unsets, aliases and relative leveloffset. builtin_attributes.rs ports
Ruby's backend/doctype profile (`update_backend_attributes`/
`update_doctype_attributes`): default `html5`/`article` with `backend-*`,
`basebackend-*`, `filetype-*`, `doctype-*` flags and combinations, `filetype`,
`htmlsyntax`, `outfilesuffix`, `pagewidth`; header `:doctype:`/`:backend:`
update them (body assignments are plain); ParseContext keeps Ruby's
`@doctype`/`@backend` apart from the attributes (book/manpage decisions use it);
nested documents reset the doctype to `article`. Compliance defaults
(`attribute-missing`/`-undefined`) and the header/footer labels are defaults.
Like Ruby's Document#initialize, `asciidoctor` ('') and `asciidoctor-version`
(`adoctra_core::COMPATIBILITY_VERSION`, the pinned reference release) are locked
overrides replacing host and document values (safe mode, dates and embedded: see
**Safe mode** and **Standalone document**).
Stateful pass bodies execute lazily at restoration, after outer stages. A dropped
slot has no body effects, while repeated slot occurrences execute independently
(e.g. a link target and its fallback label can advance the same counter twice).
Keep the shared 32-level bound and enclosing physical diagnostic origin.
ParseContext keeps reading attributes separate from conversion replay after the
header. Explicit declaration results/unsets enter source-ordered replay, without
the effects from computing them. Section and discrete titles execute stateful
substitutions once during reading only where Ruby converts them while reading
(a generated id with `sectids`, or an explicit id plus a `{` in the title);
IDs and xrefs reuse their resolved inlines. Effects reach later reading but do not
enter declaration replay. Other titles are deferred like block titles
(`Title::Nodes` placeholder) and convert at their first use during conversion:
body_subs converts the outline at a TOC (header auto TOC with the header
attributes — standalone `toc-placement=auto`, embedded not macro/preamble —,
the preamble's TOC before its content, a `toc::[]` macro with its attributes,
nested while level < toclevels/levels), a standalone document without a title
converts its first section for `<title>` first, a reference label, else the
heading itself. The implicit doctitle executes an early a-stage with a
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
caption values (including empty strings) suppress numbering. Captions keep the
markup Ruby stores in `@caption`: `Block.caption` is the flat text and
`Block::set_caption` fills `Block.caption_parts` (empty when plain) from the
`<context>-caption`/document `caption` value parts (counter numeral as text),
a block `caption=` (raw unless single-quoted: BlockAttrs.caption keeps the
substituted parts via resolve::label_parts; context.rs block_caption) and
section signifiers/`appendix-caption` (Section.caption parts). Admonition labels
share block_caption; xrefstyle's caption fallback chomps `. ` from the parts.
HTML renders caption_html (parts, else escaped text) in titles, figcaption,
table caption, headings and TOC; JSON/WASM add `caption_parts` when non-empty.
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
section, else `[^top]`. Decoded fields keep character references (and an `&amp;`
that would read as one) in their HTML spelling (entity_references.rs
decode_protection, no entity table); labels built from them (`[id]`, path) and
inline-image default alts make them Raw (reference_parts), and HTML's
reference_value keeps them in xref fields and raw block media `src`/`poster`/
`link`, like Ruby inserting those values verbatim. Like Ruby's save_attributes,
header.rs save_doctitle makes the `title` attribute the `doctitle` when no entry
or document title set one (also in nested documents); body_subs removes `title`
before the body converts, like `Document#content` (body entries replay it), while
HTML `<title>` and the manpage `<h1>` read it from the header snapshot. xrefstyle (macro
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
JSON/WASM export preamble `title`). A `[partintro]` paragraph is compound, a
lone `[open]` intro paragraph stays simple (see **Simple content**).
**Manual pages** (manpage.rs, Ruby's parse_manpage_header, at the end of
collect_header, never in nested documents): `doctitle` gives `mantitle`
(a-stage, lowercase) and `manvolnum` (ManpageTitleVolnumRx, else "non-conforming
manpage title" and fallbacks). Unless `manname` and `manpurpose` are both set,
the header consumes the NAME section: metadata before it goes to the first body
block (Header.body_pending), a level-1 title (converted while reading), then
the lines up to a blank line (line comments skipped, lstripped, joined by a
space) split by ManpageNamePurposeRx; name and purpose get the a-stage only.
Values are header attributes (`manname`, `mannames` as Ruby's Array#inspect,
raw `manpurpose`, `manname-title`, `manname-id`). Errors ("name section
expected", "… must be at level 1", "non-conforming name section body") rewind
to the line after the blanks, so the body reads metadata again (roles double,
like Ruby); a nonconforming body keeps the title registered (`_name_2` in the
body). `Document.manpage` = Manpage { names, name_section: Option<NameSection
{ heading, body: Option<Span> }>, section_title }; resolve::register visits the
heading after the doctitle (labels resolved as while reading), parse_reader
name_attributes refreshes `manname-id`/`manname-title`, footnotes include it,
visit maps its spans. An unstyled `SYNOPSIS` title (ASCII case-insensitive) is
the special `synopsis` section. HTML: `<h1>… Manual Page</h1>` (without a title
the first section's title) and, with `manpurpose`, `<section class="sect1">`
h2/p before the auto TOC (title upcased like Ruby when the first section's is,
markup untouched); JSON/WASM `manpage`; tokens: heading, attribute-ref only in
the name lines (Reader.custom_subs).
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
at the macro (its conversion attributes; else `<!-- toc disabled -->`). Entries are
heading caption + inlines with every `<a>` tag dropped (DropAnchorRx), nested
while level < toclevels/levels (Ruby to_i). The DOM profile maps nav to div.
**Images**: `BlockKind::Image { target, src, alt }` is the `image::target[…]`
block macro (image.rs match_macro = Ruby's BlockMediaMacroRx: lazy target up to
the first `[` after a non-blank, line ends with `]`; a block start only, text
inside a paragraph; a literal/listing/source/verse style keeps the line as that
verbatim paragraph, as strict_verbatim_paragraphs — also for `toc::[]`). The
bracket list is block_attributes::macro_list: the attribute line's early a-stage
(stateful, missing policies) without sub_result or shorthand, positional
alt/width/height also named (field order), title/reftext/id/role/options shared
with the metadata keys via ParseContext.block_macro (MacroMeta also carries
reftext/options; a list continuation takes it alone). The target then gets the
same a-stage; empty after `drop-line` (Reference.drops_line) drops the block and
its metadata. complete_image drops the style (it becomes the alt without an
explicit `alt`, else the basename sans extension with `_`/`-` → space), applies
replacements to a non-default alt (generated entities Raw, apply_parts) and
makes a unitless scaledwidth `%`. `src` is image.rs web_path (Ruby's secure-mode
image_uri/normalize_web_path: URIs keep form with spaces as %20, else joined to
`imagesdir`, `.`/`..` resolved) recomputed by body_subs with the conversion
attributes. Titled images are figures: assign_caption uses caption_context
(`figure` → `figure-caption`, built-in `Figure`, counter `figure-number`), and
Numbering::Caption carries that context for xrefstyle (`Figure 1, “T”`/`Figure
1`). HTML renders `<figure class="imageblock [float] [text-<align>] roles">`
with `<img src alt width height>` (inside `<a class="image" href>` with
`link`, window/noopener/nofollow like append_link_constraint_attrs) and a
trailing `<figcaption class="title">`; the DOM profile maps them to Ruby's divs.
JSON/WASM `name:"image"` with target/src/flat alt; token `block-macro`.
**SVG** (Ruby's svg branch of convert_image/convert_inline_image, below secure
mode, target containing `.svg` or `format=svg`): data_uri.rs Images::svg decides
`Svg::Inline(Option<contents>)` for `%inline` (read_svg = read_contents: a URI
only with `allow-uri-read`, else the file under `imagesdir` through
`load_binary`, BOM removed, lines right-stripped; empty/unreadable → None with
Ruby's warnings, the alt span) or `Svg::Interactive { fallback }` (the
`fallback` attribute's image_uri) where the image converts (block: body_subs;
inline: the macro). An inline SVG keeps its plain web path (no data-uri read).
`BlockKind::Image.svg` / `InlineImage.svg`; the public `inline_svg` (preamble up
to `<svg`, `width`/`height` replacing width/height/style of the start tag) is
shared by HTML and the inline image's lexical markup; post_replacements copies
contiguous runs so a multi-line atomic part survives. JSON/WASM `svg` {mode,
content|fallback}; `golden/svg/` runs the reference `--safe` in that directory.
**data-uri**: data_uri.rs Images is Ruby's `image_uri` below secure mode:
every image source (block/inline images, `icon:`, admonition and callout
icons) becomes `data:<mime>;base64,…` where it converts, read through the
optional `ResourceProvider::load_binary` (`BinaryResource`, default
Unsupported) relative to the primary document under `imagesdir`/`iconsdir`;
URIs only with host `allow-uri-read` (content type from the provider, the URI
kept on failure). Unreadable files embed empty data and warn at the image
(the diagnostics sink of the converting field). parse_reader owns Images and
ParseContext/Evaluation/Execution borrow it (`with_images`); string APIs have
no provider. MemoryResources `insert_binary`, CLI files and http(s), WASM
`ResourceWorkspace.set_binary`.
**Inline images/icons**: attribute_macros.rs apply_images (Ruby's
InlineImageMacroRx, the first built-in macro, before links) over the lexical
result of quotes/a/r, also in pass:m and repeated link parameter passes. The
list gets normalize_text (strip, LF → space, `\]`) and AttributeList without
sub_result (posattrs alt,width,height / icon size; opts split). InlineImage keeps
lexical fields (target, src, attributes; Ruby inserts them verbatim) and alt
parts (boundaries/raw survive; default alt = basename). src is image.rs
web_path under `imagesdir` at the macro; icons capture the `icons` mode (None →
`[alt&#93;`, font → `<i class="fa …">`, else icon_uri: `icon` attr or target +
`icontype` under `iconsdir`, default `<imagesdir option or ./images>/icons`);
header save_icons normalizes `icons`/`icontype` like save_attributes. Secure
mode locks `icons` unset (see **Safe mode**). Source-owned
results are `Inline::Image { image, span }`, attribute-owned
`AttributePart::Image` (lexical = Ruby's span markup). HTML renders Ruby's
`<span class="image|icon …">`; section ids ignore it; borrowed xref labels drop
its link (DropAnchorRx). JSON/WASM name:"image"/kind:"image"; token
`inline-macro`.
**Video/audio**: `BlockKind::Video { target, src, poster, embedded }` /
`BlockKind::Audio { target, src }` share image.rs match_macro (`Media`, Ruby's
posattrs: video `poster,width,height`, audio none), the macro list, target
a-stage and drop-line; the style is dropped and titles are never captioned.
media.rs resolves sources once in core (complete_media at reading, body_subs at
conversion), like convert_video/convert_audio: a `youtube`/`vimeo` poster gives
an `embedded` player URL (`asset-uri-scheme`, default https; list/playlist or
hash from the target or attributes, start/end/theme/lang and options in Ruby's
order, literal `&`), else the `imagesdir` web path plus `#t=start,end`, and a
non-empty poster resolved the same way. HTML renders `<figure
class="videoblock|audioblock">` with a leading `<figcaption class="title">` and
an `<iframe>` (no obsolete frameborder) or `<video>`/`<audio>` with Ruby's
boolean attributes and fallback text; the DOM profile maps them to Ruby's divs.
JSON/WASM `name:"video"`/`"audio"`; token `block-macro`.
**Tables**: `BlockKind::Table(Box<Table>)` is a `|===`/`,===`/`:===`/`!===`
block (a longer `=` run closes only with the identical line; any style keeps it
a table; a paragraph ends at the fence). parser.rs read_table captures the lines
to that fence without `//` line comments; table.rs ports Ruby's parse_table,
Table::ParserContext, Cell and Column: psv cell specs at line start and before a
separator (CellSpecStart/EndRx, leftmost end match — Ruby drops a trailing
` b` style-like word), `\|` escapes, colspan/rowspan (`2.3+`), duplication
(`3*`, every copy converts again), alignments and styles (d/s/e/m/h/l/a), csv
(quotes, `""`, unclosed-quote recovery) / dsv / tsv and `separator` (a start
separator slices one character, like Ruby), the implicit header (first line holds
the whole first row, then a blank line; reinitialized cells when rejected),
`%header`/`%noheader`/`%footer`, Ruby's widths (`cols` specs, `~` autowidth,
Float#truncate(4), donation to the last column, Ruby number spelling) and its
error messages. Cell text is `(offset, char)` so stripping/unescaping/unquoting
keep physical spans; body cells split into paragraphs at blank lines (trailing
empty converted paragraphs dropped), header cells are one run, literal cells keep
raw text. Leading cell anchors are cataloged in Ruby's order (header row at
partition). body_subs converts head, body, foot and resolves frame/grid/stripes
(`table-*` fallbacks, topbot → ends) and per-cell `cellbgcolor` after the cell's
content. Titles number with `table-caption`/`table-number`. HTML keeps Ruby's
semantic `<table class="tableblock …">` markup exactly (golden/tables compares
lines); JSON/WASM `name:"table"` with columns, head/body/foot cells (spans in
WASM); tokens `table-delimiter`/`cell-separator` from parser spans, cell text
scanned between separators (the innermost table decides a line).
**AsciiDoc cells** (`a`, Ruby's inner Document) are `CellContent::Blocks`, read
when the cell is created (cells complete in Ruby's creation order, `Cell.order`):
parser.rs read_nested_document builds `Reader::nested` (the cell's lines at their
reader offsets, no preprocessing; Reader::absorb keeps highlighting records, its
entries never apply to the parent) and runs header, body and nest_sections inside
ParseContext::enter_nested/exit_nested. The nested document starts from the
parent's reading attributes (without doctitle/notitle/showtitle/toc/toc-position,
defaults are not reapplied, like Ruby's `unless parent_doc`, so a parent's
unset default stays unset) with every inherited key locked except compat-mode, doctype,
notitle, showtitle, toc, toc-placement and toc-position (Ruby deletes them from the
overrides, the parent's host locks included) and an unlocked `notitle` default;
catalogs and ids are shared. A nested header title becomes `blocks[0]`, a level-0
Heading deferred like a block title (`CellContent::Blocks.has_title`,
ParseContext.nested_titles keeps it out of automatic ids; its lines are recorded
as heading tokens), and nest_sections opens a preamble for it; HTML renders it as
Ruby's convert_embedded `<h1>` unless its conversion attributes have `notitle`.
`CellContent::Blocks.attributes` is the nested header snapshot: like Ruby's
convert_embedded the inner document's automatic TOC (header `toc`, placement not
macro/preamble) follows its title, a preamble placement ends its preamble and a
`toc::[]` lists its own sections. body_subs swaps Execution.outline/preamble_toc
for the cell (title first, then the TOC's lazy titles, then the body); HTML Cx
carries the converting document's outline and header (`Cx.outline/header`). Its counters (`{counter:…}`,
caption numbers) delegate to the root document's attributes and locks, like
Ruby's `counter` (Evaluation::with_root, both at reading and conversion).
BodySubs.nested keeps its header snapshot, entries and locks per cell; body_subs
converts the blocks with them and restores the parent (a repeated cell keeps its
deferred fields for every copy). Inside it, psv tables use `!` (`!sv`, also for
`|===`). HTML wraps the blocks in `<div class="content">`; JSON/WASM `blocks`.
Like Ruby, the cell's first line is preprocessed again when it contains `::`
(`a|include::…[]`, single-line `ifdef`, escapes): Reader::preprocess_cell reads
the cell's lines again as inputs after every loaded line (only the first one
preprocessed, with its own conditional stack and the parent's reading
attributes), keeping increasing reader offsets and physical origins per run
and line feed; an unchanged first line keeps the original lines.
**Lists**: parser.rs ports Ruby's parse_list / parse_description_list /
parse_list_item / read_lines_for_list_item with lists.rs (UnorderedListRx `-`,
`*`-runs, `•`; OrderedListRx `.`-runs, `1.`, `a.`/`A.`, `i)`/`I)`;
DescriptionListRx `::`/`:::`/`::::`/`;;` and their sibling expressions;
AnyListRx; resolve_ordered_list_marker with the "list item index: expected …"
warning; is_delimited_block?). Leading whitespace is allowed before a marker. An
item's lines are collected up to a sibling (blank lines, `+` continuations as
placeholders, detached/frozen continuations, delimited blocks only after `+`,
literal paragraphs, greedy description terms, nested lists of any other marker),
then parsed as blocks from a `Reader::nested` sub-reader (collect_blocks_in with
`BlockCtx::InListItem { list_type, text_only, adjacent }`: text-only first block
— no macros, admonition labels, titles or attribute entries — and paragraphs that
end at list lines only when adjacent); an adjacent paragraph folds into the item
text (fold_first, indentation removed). A list line no longer interrupts a
paragraph (Ruby), and a plain paragraph ends at a delimited block or block
attribute line. `ListItem.marker`/`checkbox`; ordered lists get Ruby's implicit
style in `attrs.style` (from `.`-run length or the marker), checklists the
`checklist` option. `BlockKind::DescriptionList { items }` with
`DescriptionListItem { terms: Vec<ListTerm>, description: Option<ListItem> }`;
terms without a description merge into the next; a term's leading anchor takes
the rest of the term as reftext. Unstyled indented paragraphs are literal blocks
(adjust_indentation!) except a list item's text-only/description-adjacent one.
HTML: `<ol class="style" type start reversed>` (no `type` for a lettered/numbered
marker's own style, as Ruby's symbol style), checklist markers (interactive,
font icons; `BlockKind::List.icons` captures the document `icons` mode where a
checklist converts, like `convert_ulist`, so a body entry before it applies;
JSON writes it on checklists only), `<dl>` (dt.hdlist1 without style), horizontal `<dl class="hdlist">`
of `<div>` entry groups laid out as Ruby's layout table (`display: table`,
labelwidth/itemwidth as `--adoctra-hdlist-*width`), qanda
`<ol class="qanda">`; the DOM profile maps Ruby's dlist/qlist wrappers and hdlist
table. JSON/WASM `dlist`, item `checkbox`; parser-recorded `list-marker` tokens
(markers and delimiters).
**Callouts**: callouts.rs ports CalloutSourceRx (`sub_callouts`): marks ending a
verbatim line (`<1>`, `<.>` numbered per block, `<!--N-->`, escaped `\<1>` losing
its backslash), each chain head keeping a `// `/`# `/`-- `/`;; ` guard (or the
block/document `line-comment` prefix, empty = none). parser.rs apply_callouts runs
at block completion on `Listing`/`Literal` (`callouts: Vec<Callout>` with text
ranges, guard, xml form, Ruby id `CO<list>-<n>` from ParseContext.callouts =
Ruby's Callouts catalog, shared with nested documents, and the mark's source span
matched on the line with the same ending). `BlockKind::CalloutList { items:
Vec<CalloutListItem { item, coids }> }` is parse_callout_list (CalloutListRx, no
indent; items read like other lists; "callout list item index" and "no callout
found for <N>" warnings; the catalog moves to the next list). HTML renders Ruby's
`<b class="conum">(N)</b>` with the guard (font/image icons with `icons`) and an
`<ol class="colist arabic">`; with `icons` each item starts with the same number
icon (`<span class="colist-icon">`, text in a `<div>`) and the stylesheet lays out
Ruby's icon/text columns (`ol.colist.icons`), so it looks like Ruby's table
(checked with screenshots); conum styles follow Asciidoctor's default
stylesheet. JSON/WASM `callouts` and `colist` with `coids`; tokens `callout`
(from AST) and `list-marker`.
**UI macros** (only with `experimental` at the substitution point):
attribute_macros.rs apply_ui runs first in the macros stage (before images,
also in pass:m and link parameter passes) Ruby's InlineKbdBtnMacroRx, then
InlineMenuMacroRx, then the `"File > Save"` InlineMenuRx shorthand, each over
the previous pass's lexical result. `UiMacro::Keyboard { keys }` (`\]`
unescaped, strip, split at the first `,`/`+` after the first char, a trailing
delimiter is itself the last key), `Button { label }` (normalize_text), `Menu {
menu, submenus, item, font }` (items split at `&gt;` else `,` with Ruby's split
dropping trailing empties; `menu:X[]` → item None = menuref; `font` = `icons`
is `font` at the macro, picks the caret). Labels keep lexical parts. Source-owned
`Inline::Ui { ui, span }`, attribute-owned `AttributePart::Ui`. While the later
macro stages run, Text::replace_ui keeps the macro as marks carrying Ruby's
markup around its labels (substitution_text.rs UiMark), so links, xrefs,
images and footnotes see Ruby's lexical text and nest inside a label; leaving
the Text (into_parts/write_back) rebuilds the node (restore_ui). Unlike Ruby,
content containing a nested macro runs to the bracket balancing it
(content_end/nested_macro: `btn:[link:x[y]]`, a literal `kbd:[[]` still ends at
the first `]`) and `,`/`+`/`&gt;` inside a nested macro or `<<…>>` do not split
keys/items (nested_regions), so the output stays well-formed where Ruby
misnests tags. A boundary crossing the markup is split (repair: closed at a
label end and reopened in the next label/after the macro; formatting opened
before closes before it with an empty span that `rebuild` accepts); a link
opened before the macro and closed inside a label keeps Ruby's literal markup.
HTML renders Ruby's `<kbd>`/`span.keyseq`/`b.button`/`span.menuseq`;
JSON/WASM name/kind `kbd`/`button`/`menu`; token `inline-macro`.
**Footnotes**: attribute_macros.rs apply_footnotes is Ruby's
InlineFootnoteMacroRx, the last macro (after xrefs), over the lexical stream, also
in pass:m and link parameter passes: `footnote:[text]`, `footnote:id[text]`,
`footnote:id[]`, legacy `footnoteref:[id,text]`/`[id]` (deprecation warning unless
compat-mode), escapes, empty macros stay text, content lazy to the first unescaped
`]` not followed by a generated `</a>`. Content keeps the earlier stages' output as
lexical parts (normalize_text: strip, LF → space, `\]`); passthrough slots inside
are restored with it. A definition takes `counter:footnote-number` through the
Evaluation (so nested documents number on the root) and registers in the
document's Footnotes registry (Evaluation::with_footnotes: ParseContext while
reading — section titles, anchored titles, definitions — then body_subs; nested
documents swap their own, like Ruby's `catalog.merge footnotes: []`); a known id
is a reference, an unknown id without text warns "invalid footnote reference".
Like Ruby, conversion restarts the counter from the header attributes. The
atomic `AttributePart::Footnote(Box<Footnote>)` (kind/index/id/content/sequence,
lexical = Ruby's sup markup) becomes source-owned `Inline::Footnote`. After
conversion parser.rs collect_footnotes builds `Document.footnotes`
(FootnoteDefinition, registration order, deduplicated by sequence) and each
AsciiDoc cell's `CellContent::Blocks { footnotes }`. Section ids see `[N]`;
borrowed xref labels drop the marker link (sup with role); textless xrefs inside
the content get labels. HTML renders Ruby's markers and the `<div
id="footnotes">` list after the body (and inside AsciiDoc cells) unless
`nofootnotes`; JSON/WASM `name:"footnote"` / `kind:"footnote"` (type, index, id,
parts) and a document `footnotes` list; token `inline-macro` projected from AST.
Duplicate ids warn like Ruby (resolve.rs register_id): `id assigned to
block|section|anchor already in use: <id>` (section for non-discrete headings).
Xref labels: resolve::register builds the registry after reading (auto ids,
block ids, cataloged anchors) in Ruby's order: a section heading at its title,
any other block after its content and its cataloged inline anchors (end of
`next_block`, where an anchored title also resolves its references); at conversion apply_xrefs resolves
each source xref inside the macros stage (Evaluation::label → body_subs Execution
as substitution_context::Labels, which borrows the evaluation's attributes/counters/
footnotes/root via Lent), so a converted target title numbers its footnotes before
the field's later footnotes and the label's lexical text is visible to them;
the field's resolution then only warns (Xref.resolved) and installs the label nodes
kept in Xref.label with their own spans (generated `[id]` text gets the reference's
restored span). Only definition-fixed xrefs (apply_fixed) fill after the field's subs.
Labels follow Ruby's xreftext:
nonempty reftext formatted per reference with the current attributes/counters
(effects persist, warnings repeat), else the block/section title, else `[id]`.
A deferred block title converts once (first reference or the block itself);
references inside a label being produced show `[id]` (@resolving_xref), and a
self-reference converts the title again. Labels unwrap Link/CrossReference and
drop anchors (DropAnchorRx). Auto-id headings and anchored `{` titles resolve
references while reading (unregistered ids give `[id]`, a deferred target
title converts early) and the auto id derives from that result.
Block.attribute_context remains the reading snapshot at block completion;
it can therefore differ from resolved inline values. Converters read
Block.conversion_context (Block::conversion_attributes, falling back to the
reading snapshot for unvisited blocks): body_subs Execution takes it on block
entry, after replay and caption counters and before title/content, sharing one
Arc while the values are equal. It has the replayed declarations and earlier
conversion effects (a paragraph's `{set:}`) but no reading-only effects
(orphan declarations, section-title expressions), like Ruby's converter.
Tokenization uses isolated contexts and the same declaration replay; it never
mutates caller attributes or previously returned snapshots.
Inline anchors in quote metadata, pass macros and doctitle replacements are
cataloged exactly where Ruby's raw scan finds them (pinned by `anchor-catalog-*`).
A replacing `doctitle` converts its stored spelling (attributes::reference_text of
its parts, header.rs effective_title) as the title source, like Ruby: markup from
a whole-value passthrough is escaped text, entity references stay text.
Automatic section IDs sanitize a temporary lexical title: raw XML tags and
references matching the pinned entity grammar are removed, not decoded.
Literal protection and structural inline boundaries remain distinct; title
nodes, attribute snapshots and source spans are unchanged. resolve.rs IdPolicy is
Ruby's Section.generate_id with the heading's reading attributes: `sectids` (default
on; unset → no id, not registered, TOC `href="#"`), verbatim `idprefix` (`_` when
unset) and the first char of `idseparator` (empty only deletes spaces; tr_s squeezes
space/`.`/`-`/separator runs, prefix included), dedup `<id><sep>2`. Generating an id
cuts a longer `idseparator` in the reading attributes (ParseContext::generating_id,
lock-free, reading only), like Ruby's write-back.
The tokenizer shares header parsing (author/revision tokens). Setext document titles use adjacent
processed lines, Unicode character lengths (difference <= 1), uniform =/-/~/^/+
underlines and effective level zero. ATX takes precedence. The Heading/header
title span includes both lines; inline spans cover title text only. Highlight
both lines as separate heading tokens and retain physical source fragments across
includes/selection/indent. Setext installs compat-mode before subsequent header
metadata, respecting locked overrides; substitutions then behave as with an explicit
`:compat-mode:` (pinned by the `setext-compat-*` cases).
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
profile treats section as div. Like Ruby's convert_section, a section title
with an id (not floating/document titles) wraps its numbered title in `<a
class="link">` with `sectlinks` and adds `<a class="anchor">` before it
(`sectanchors=after`: after), reading the heading's conversion attributes;
adoctra.css styles them like Ruby (§ on hover, checked with screenshots). Sectnums, special sections, book partintro and
preamble cases are implemented (see above). JSON and WASM expose header metadata;
WASM includes source spans. See parser-api.adoc for remaining header limitations.
Comments (`//`, matching slash fences of length >= 4, and `[comment]` paragraphs
or open blocks) are omitted contextually. ParseResult.comments retains source
ranges, exported to WASM and comment tokens. Inline scanning across removed
comment lines restores spans through a fragment map. Verbatim regions preserve
comment-looking text. `ParseResult.tokens` is produced by the same structural
parse as the document. Reader records metadata, header, fences, macros, list
markers, table separators and verbatim bodies; tokenizer only projects their
spans. Normal inline evaluation records syntax over whole fields before macros
consume it, so multiline quotes follow AST boundaries. Selected `subs` retain a
read-only syntax scan of the whole field filtered by enabled stages. No effects
or structural grammar are replayed for highlighting. WASM `parse_json` (including
ResourceWorkspace) exports `tokens` alongside document and diagnostics, so
editors need one call and includes load once. Legacy `tokens*` APIs return the
same result field. Incremental parsing and a pull/event API remain future work.
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
Operands containing `{` run Ruby's `sub_attributes(attribute_missing: 'drop')`
through ParseContext::preprocessor_subs (inline_attributes.rs preprocessor:
the shared a-stage over raw source, Unicode Word names, values in Ruby's stored
spelling via reference_text, e.g. `a&lt;b`), left operand first; set/counter
expressions execute there as reading effects only (later conditions see them,
conversion restarts from the header snapshot; before header completion they
enter it), never in skipped branches. Reader::has/step/process take the mutable
ParseContext for this. Effects from preceding definitions are visible to the reader. Basic include expansion is available with an explicit provider.
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
includes within its existing root (or `-B/--base-dir`); stdin reads from
`-B`, else a `docdir` attribute, else the cwd (which becomes docdir), like
Ruby's base_dir; string APIs have no provider. With an absolute `docdir`
(Ruby's base_dir) system_path.rs ports `normalize_system_path`: include and
image targets resolve from the current include's directory (the base dir in
the primary), jailed from safe mode up with Ruby's recovery and verbatim
unlocated warnings; the provider gets that path relative to the referrer
(document_path stays metadata) and messages name the absolute path
(`include file not found: …`, `<docfile>: SVG does not exist …`). A
single-line ifdef reprocesses `include::` content; stdin registers includes
relative to the base dir.
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
Includes also support indent and depth; the API attribute `max-include-depth`
(read when the reader is created, Ruby `to_i`, default 64, bounded by
`IncludeLimits.max_depth`; not positive → includes stay literal) is Ruby's
absolute maximum, and errors name the relative limit like Ruby. Indent runs after selection and before
preprocessing, with optional tab expansion from tabsize. Reader line fragments map
expanded tabs to their physical byte and exclude synthetic padding from highlighting.
Physical snapshots never change. Indentation growth (including intermediate tab
expansion) is preflighted and charged to the include byte budget. Depth is scoped to
an input: a child override can extend an inherited limit but never the host maximum;
returning to a parent restores its limit. Reader normalization also runs without
indent: primary and AsciiDoc inputs rstrip ASCII whitespace/NUL, while raw includes
only chomp. Unicode spaces and internal tabs survive this stage. Only primary input
condenses final empty lines; includes preserve block boundaries. Removed suffixes
and retained line endings have separate physical fragments; token projection
must not normalize processed raw content again. Verbatim `tabsize`/`indent`
outside include indentation follow build_block (see **Source defaults and
verbatim indentation**). Multiline attributes now use
one parser-owned collector for header/body: space-backslash and legacy space-plus,
marker consistency, blank/EOF termination, locked/unset consumption, and cross-source
continuations. Preprocessing sees the old context until collection finishes. Reader
records consumed attribute lines and values for tokenizer classification; preserve
physical spans and avoid independently interpreting continuation text as syntax.
Body entries wait in ParseContext.pending_entries for the next block of their
collection (Ruby's `:attribute_entries` in the pending block attributes):
collect_blocks_in flushes them into conversion replay before parse_block,
restores the replay when no block results (a skipped `[comment]` paragraph or
block, a dropped block macro clear Ruby's attributes) and drops orphans at the
collection end (container, list item, AsciiDoc cell). Section titles take them,
so they cross section boundaries. Reading attributes keep orphan values (later
conditions see them), as does Block.attribute_context, but not the conversion
attributes HTML reads.
ResourceProvider::document_path optionally supplies a slash-separated absolute
path for primary docfile/docdir/docname/docfilesuffix attributes. MemoryResources
and CLI opt in; opaque providers default to None. Resource parsing/highlighting
locks these attributes before the header, overriding caller values and preserving
them across includes. Keep metadata separate from canonical referrers, never reload
the primary snapshot, and do not infer cwd or paths in string APIs. Include parameter lists now follow the pinned
AttributeList grammar, including ignored positional/unknown fields, quote recovery,
duplicate/None rules and options merging/spacing. Encoding is handled by
`ResourceProvider::prepare_include` before selection; its default delegates to
`load_include`, then `load_with_encoding`, preserving custom/text-only decoders.
CLI/MemoryResources use `BinaryResource::prepare_include`: valid input and
transcoding codecs retain the previous decoder; malformed UTF-8/BINARY can be
deferred to reader selection. Selected invalid lines fail; unselected invalid
lines are omitted from the UTF-8 view and retained as `OmittedSourceLine` records.
UTF-8 tag-regexp candidates (both `::` and `[]`) must be valid even outside a
selected tag. BINARY markers use ASCII word/space classes on original bytes;
raw non-ASCII names do not equal UTF-8 attribute names. Wildcards track raw names
without lossy decoding. A diagnostic on an omitted marker uses an empty snapshot
anchor and `omitted_span: OmittedSourceSpan { line, start, end }`, whose offsets
are bytes within the original line. `SourceFile::omitted_slice` resolves them;
WASM adds this optional field to diagnostic JSON and CLI editor output labels
the range as omitted bytes. Conversion cursors remain at the include directive.
Physical tag warnings are interleaved with reader diagnostics in emission order,
before missing-tag reports. Invalid UTF-8 names in warnings use `<binary:HEX>`
instead of crashing like Ruby's JSON logger; this is a documented oracle boundary.
Only finite positive line selections pass a decoding bound; open ranges,
invalid first line numbers and tag selections read the whole resource.
If the full decoding fails but the consumed prefix is valid, `IncludeResource`
returns that prefix with `complete=false`. Valid resources retain full snapshots.
No replacement characters are fabricated. I/O still fetches the whole resource.
SourceMap registers the decoded UTF-8 snapshot, so spans are offsets in that
snapshot, not offsets in the original file when decoding or omissions change
its layout. Names, text, completeness and omission records jointly identify
reusable snapshots. `SourceFile::omitted_lines` and WASM
`source_omitted_lines_json` expose each insertion offset, physical line number
and raw bytes (hex in JSON). `SourceFile::position` accounts for gaps; omitted
bytes count toward the include budget. Sparse snapshots cannot be reparsed as
implicitly valid primary sources. `is_complete`/WASM `source_is_complete`
distinguish these views and prefixes from full files. BOM/CRLF remain intact.
Unicode (including CESU-8/UTF8-MAC), 53 single-byte codecs and 29 Japanese/Big5/GB/Korean/mobile codecs use std-only decoding with mappings generated
by `scripts/generate_encodings.rb` from the pinned Ruby environment. Unknown
names fall back to UTF-8 like Ruby; codecs unavailable in the reference fail
as unreadable. All names in the pinned registry are classified explicitly.
Japanese variants retain Ruby's distinct mappings (notably SJIS aliases
Windows-31J, not Shift_JIS); malformed and unmapped sequences are unreadable.
The five Big5 variants preserve 91516 separate mappings, including private-use
and supplementary-plane characters. Big5-HKSCS:2008 aliases Big5-HKSCS; csBig5
is unknown in pinned Ruby and falls back to UTF-8. Do not import web Big5
semantics: HKSCS 8862/8864/88a3/88a5 are undefined, not decomposed characters.
Finite line selection may recover a prefix, but transcoding validates skipped
bytes within the consumed portion and tag reads. Big5 uses the same snapshot,
source identity, diagnostic and provider contracts as the other codecs.
GB2312/EUC-CN/eucCN and GB12345 retain simplified/traditional differences.
GBK/CP936 accepts byte 80 as euro; GB18030 rejects it and uses a2e3 instead.
The GB family adds 60725 short mappings. GB18030's 1087996 four-byte mappings
are compacted into 207 verified contiguous ranges, with holes and Unicode limits
checked explicitly. Preserve pinned Ruby's a8bc -> U+E7C7 and 8135f437 -> U+1E3F;
do not substitute another GB18030 revision. The independent fingerprint covers
all 1587600 syntactic quadruples, 158760 truncated prefixes and 3072 mutations,
in addition to the exhaustive single-byte/pair checks. GB1988 remains unavailable
because pinned Ruby has no UTF-8 converter for it.
EUC-KR/eucKR and CP949 have separate tables (8226 and 17048 mappings). CP949
accepts extension Hangul such as 8141 -> 갂; EUC-KR rejects those bytes.
Both retain strict transcoding and the existing finite-prefix/source contracts.
ISO-2022-JP/ISO2022-JP uses an explicit state machine: ESC $ B/@ selects JIS
X 0208, ESC ( B/J selects ASCII (Roman preserves backslash/tilde like Ruby).
Its 6879 JIS mappings reuse EUC-JP's table. LF/CR/control bytes in JIS mode,
SO/SI, katakana/revision/JIS X 0212 escapes and malformed sequences fail;
a final reset to ASCII is optional. The independent state fingerprint checks
656645 inputs across all five starting designations. CP50220/CP50221 share
the state machine with CP51932's table (7336 JIS pairs, including vendor
extensions and wave dash -> U+FF5E).
ESC $ @ rejects the NEC/IBM extension rows (2d, 79-7c) but retains Windows
Unicode mappings for the base characters; only ESC $ B enables those rows.
Both decode halfwidth katakana via ESC ( I or SO, and accept raw A1-DF bytes
outside JIS mode. SI selects ASCII without restoring the state preceding SO.
Controls/newlines in katakana and shifts inside a JIS
pair are invalid. Each variant's independent fingerprint covers 1575948 inputs
in twelve initial designations/shift combinations. This implements decoding
only, not the variants' different encoding rules.
stateless-ISO-2022-JP decodes each 90/92 + A1-FE + A1-FE triple with EUC-JP's
6879 JIS mappings (1978/1983 produce the same Unicode). Its Ruby IO transcoder
rejects ESC/SO/SI, preserves other ASCII and has no escape state changes.
String#encode bypasses the transcoder on ASCII-only inputs and can incorrectly
accept those controls; its oracle must use Encoding::Converter like Ruby IO.
Bare EUC pairs, halfwidth kana and JIS X 0212 prefixes are invalid, including
inside triples.
Its independent fingerprint covers 196865 inputs: all single bytes, pairs and
90/92-prefixed triples. Both prefixes producing the same UTF-8 share a snapshot.
KDDI variants reuse CP51932 with 819 generated overrides (508 defined, 311
explicitly undefined). 21 mappings expand to two Unicode scalars, including
regional-indicator flags and keycaps without VS16. ISO-2022-JP-KDDI has 7185
mappings under ESC $ B; ESC $ @ can be entered/exited empty but its characters
are invalid. Stateless KDDI has 7573 mappings under prefix 92 only, including
IBM rows 93-97; unlike ordinary stateless JP, its IO converter preserves
literal ESC/SO/SI. Both use strict Ruby IO fingerprints (853510 inputs).
Never generate mappings by reverse UTF-8 encoding: Ruby can encode 😀 to u+
but cannot decode it. Compound output keeps UTF-8 spans, Unicode scalar columns
and decoded-byte budget accounting. Public APIs and schemas stay unchanged.
EUC-JIS-2004/EUC-JISX0213 reuses the EUC-JP byte grammar with its own 11271
scalar mappings (8772 plane 1, 2436 plane 2, 63 kana), including 303 outside BMP.
Pinned Ruby rejects 25 combining mappings (a4f7, etc.); keep these undefined,
never import other codecs' decompositions or silently compose Unicode output.
Plane 2 differs from legacy JIS X 0212: 8fa1a1 -> 𠂉 is new; 8fa2af -> ˘
only works in EUC-JP. The independent strict IO fingerprint covers 131329
inputs. Appending its table preserves all previous table indices and contents.
SJIS-KDDI shares Windows-31J's byte grammar/base table with 647 overrides:
120 defined replacements (21 compounds), 527 explicitly undefined mappings.
Ruby decodes 9140 non-ASCII characters; 13 mappings contain supplementary
scalars. ASCII controls, kana, vendor extensions and unmodified private-use
characters retain Ruby IO behavior. F6A5 -> 🇯🇵 differs from Windows-31J's
U+E4CC, while F040 -> U+E000 is retained; F64A is undefined despite Ruby's
reverse UTF-8 encoding of 😀. Its strict IO fingerprint covers 65793 inputs.
Mapping overrides are now explicit slices, with separate JIS/SJIS KDDI key
domains; never apply one domain's overrides to the other.
SJIS-DoCoMo shares the same Windows-31J grammar/base with 282 overrides:
70 replacements, 212 explicitly undefined. Ruby decodes 9455 non-ASCII
characters, including 11 keycap combinations and one supplementary mapping
(F8CD -> 🅿). F987 -> 1 + U+20E3 differs from Windows-31J U+E6E2; F040 keeps
U+E000. F996 is undefined despite reverse UTF-8 encoding of 😀. Literal ASCII
controls and kana stay unchanged; its strict IO fingerprint covers 65793 inputs.
The generator emits each mobile SJIS override table from the same grammar;
keep their entries independent. ISO-2022-JP-2 has no Ruby converter.
SJIS-SoftBank uses the same Windows-31J grammar/base with 944 overrides:
171 replacements, 773 explicitly undefined. Ruby decodes 8894 non-ASCII
characters (42 compound mappings, 28 containing supplementary scalars).
Duplicate forms F6E4/FBAB -> 🇯🇵 and F2E5/F7BC -> 1 + U+20E3 share decoded
snapshots, while Windows-31J can produce different snapshots for those bytes.
Literal ESC $ G/P, SI and truncated escape-looking strings are preserved by
Ruby IO, with no escape state machine. Its strict fingerprint covers 197121
inputs including every selector/payload pair with and without SI. F99A is
undefined despite reverse UTF-8 encoding of 😀; F040 keeps U+E000.
UTF8-DoCoMo validates UTF-8 strictly and applies 282 Unicode-scalar PUA
u32 overrides (70 replacements, 212 undefined); these keys are independent
of SJIS byte keys. U+E63E -> ☀, U+E6E2 -> 1 + U+20E3, U+E66C -> 🅿;
11 replacements are compound and one supplementary. All other Unicode
scalars pass unchanged, including literal 😀; U+E6F1 is undefined despite
Ruby reverse encoding of that emoji. Unlike ordinary UTF-8, malformed bytes
on skipped lines or outside tags still fail. Finite lines may recover only
an intact prefix. UTF-8 BOM remains in the source snapshot; explicit UTF-16
BOM is invalid. Equivalent PUA/literal decoded text shares snapshots.
Its strict fingerprint covers 1183513 inputs, including every Unicode scalar,
short byte strings, truncated and mutated UTF-8 (9501193 probes overall).
UTF8-KDDI uses the same strict UTF8-mobile path with an independent 1294-entry
Unicode-scalar PUA table: 240 replacements, 1054 undefined. All differences
were checked across every Unicode scalar and lie in BMP PUA. Two banks each
provide the same 120 replacements (42 compound/26 supplementary mappings total):
U+E4CC/U+EFA5 -> 🇯🇵, U+E522/U+EFFB -> 1 + U+20E3, U+E4A6/U+EF7E -> 🅿.
Both banks and equivalent literal text share snapshots; UTF-8/DoCoMo can
preserve those PUA scalars in separate snapshots. Literal 😀 passes, while
U+E472 is undefined despite Ruby reverse encoding. Its independent strict
fingerprint adds 1183513 probes (10684706 overall). Keep SJIS byte keys separate.
UTF8-SoftBank has its own 485-entry PUA scalar table: 86 replacements,
399 undefined, 21 compound mappings, 14 containing supplementary scalars.
Do not reuse SJIS-SoftBank's distinct byte table (171 replacements). Every
Unicode scalar was verified; all differences are in BMP PUA. U+E04A -> ☀,
U+E50B -> 🇯🇵, U+E21C -> 1 + U+20E3, U+E14F -> 🅿; U+E000/U+E53F pass
unchanged. Literal 😀 passes, U+E059 fails despite Ruby reverse encoding.
ASCII/ESC $ G/P/SI stay literal. Strict validation before selection, finite
prefix recovery, BOM, decoded byte budgets and snapshot reuse follow the
other UTF8-mobile codecs. Its independent fingerprint adds 1183513 inputs
(11868219 overall). Keep Unicode-scalar and encoded SJIS-byte keys separate.
CESU-8 uses a std-only strict decoder for canonical one-to-three-byte UTF-16
units. Adjacent high/low surrogates combine into one supplementary scalar;
native four-byte UTF-8, lone/reversed/separated surrogates, overlong forms and
truncations fail. Raw NUL is valid; C0 80 is not. BMP/private-use/noncharacters
pass unchanged. UTF-8 BOM stays in the snapshot; explicit UTF-16 BOM fails.
Strict consumed-prefix validation, finite lines recovery and decoded UTF-8
budgets follow other strict codecs; skipped malformed lines/tags still fail.
Source spans address the decoded UTF-8, so a six-byte CESU-8 surrogate pair
occupies four snapshot bytes and one Unicode column. Equivalent native UTF-8
loaded with its own encoding can reuse the snapshot; native UTF-8 bytes must
still fail when interpreted as CESU-8. Its independent strict IO fingerprint
covers 2495258 inputs (14363477 overall), including every UTF-16 unit, every
valid surrogate pair and every forbidden native supplementary UTF-8 scalar.
UTF8-MAC (UTF-8-MAC/UTF-8-HFS) validates UTF-8 strictly, then composes
adjacent pairs greedily: 871 verified BMP mappings plus 11172 algorithmic
Hangul joins. Ruby preserves scalar input, historical exclusions and mark
order; e + U+0301 -> é, while a + U+0315 + U+0300 stays in that order.
A + U+030A + U+0301 chains to Ǻ; iota + diaeresis + acute gives U+03CA + acute.
No general NFC decomposition/reordering is performed. ASCII flushes preceding text but may start a new pair; supplementary scalars
flush and pass through. BOM, strict selection and decoded budgets follow
other strict UTF-8 codecs, with one IO buffering boundary: LF is held until
one following scalar or clean EOF. A malformed first scalar after selected LF
fails; a valid lookahead allows recovery before a later error. The lookahead
is validated without entering the retained prefix snapshot.
Composed/decomposed equivalent input shares snapshots;
ordinary UTF-8 may retain a separate decomposed snapshot. The generator uses
Ruby stdlib canonical candidates, filtering pairs through the strict converter;
all 12043 source-table compositions were independently audited (871 + 11172).
2195 canonical probe strings include excluded/chained forms. Its independent
fingerprint adds 1218628 inputs (15582105 overall). All names in the pinned
Ruby registry now have a decoder or an explicit unavailable-converter result;
the generator aborts if a new usable converter lacks an implementation.
The decoder's exhaustive short-input fingerprints can be reproduced with
`ruby scripts/generate_encodings.rb --fingerprints`. Other Ruby versions,
untested encoding combinations and decoding/selection order remain open;
never claim that the entire encoding surface is covered. Selected malformed UTF-8 and implicit UTF-16 BOM selection
can crash Ruby outside its include error handler; adoctra must stay error-tolerant.
Default UTF-8/BINARY selection runs on original byte-LF slots before BOM
recognition. If the first kept line has a UTF-16 BOM, decode kept array lines
using that BOM, including a later hint with different endianness. Preserve raw
unselected lines or explicit omissions. Carry the raw tag mask through decoding;
never reapply tag filters to decoded marker-like text. Without a kept BOM, retain
selected UTF-8/ASCII bytes including NUL. Whole-resource/explicit codec loading
keeps its usual behavior; custom host decoders remain unchanged.
Private physical-line frames preserve IO boundaries when LF is a byte inside
an encoded character (e.g. UTF-16BE 上). Snapshot text contains no invented LF;
`SourceFile::position`, reader views and snapshot identity include the frames.
Contextual views are incomplete and cannot be reparsed as primary sources.
Reuse a complete Unicode snapshot only when all bytes and Unicode LF boundaries
agree. Raw tag diagnostics are remapped after conversion; omitted diagnostics
retain their original byte marker range. Public provider APIs/JSON schemas stay
unchanged. Reproduce the focused audit with
`python3 scripts/audit_include_order.py --report /tmp/include-order.json`;
inputs and the recorded report live in `compatibility/encoding-order/`.
It compares normalized HTML plus ordered diagnostics and reports uncaught Ruby
exceptions as RUBY_ERROR, never PASS. CLI failure cannot be quarantined.
Binary fixtures use `binary_resources` (hex bytes) in the minimal corpus.
Ordinary/intrinsic attribute substitution runs once
before parsing the list, with both reference escapes and attribute-missing policies.
Single quotes do not run inline substitutions here: the pinned reader omits
sub_result. Missing-attribute warnings map to physical include directives; preserve
this mapping through selection/indentation. Targets now share this single-pass
substitution grammar, with the pinned reader's
missing-reference rules: warn uses drop-line and warns instead of loading an empty
result unless optional; drop-line skips missing targets before parameter parsing.
Blank expanded targets never call the provider and are handled before encoding/depth
validation. Diagnostics retain physical directive spans after selection/indent.
Like Ruby's replace_next_line, a blank target (unexpanded in the text) or a
failed load becomes a
generated `Unresolved directive in <path> - include::<target>[<attrlist>]` line
(reader.rs unresolved_include, origin = the directive): `<path>` is the primary
file name (`<stdin>` without document_path) or the include's path relative to
the document directory (xref_target::relative_path), else its source name.
InvalidEncoding is reported as `include file/uri not readable` at the directive;
optional suppresses only NotFound. Exceeded depth, cycles and byte limits keep
the directive; without a
provider (stdin, string APIs) includes stay literal.
Targets and parameter lists use the same preprocessor_subs (set/counter are
reading effects) exactly where Ruby's preprocess_include_directive calls
sub_attributes: a target containing `{` once (missing policy fixed, warn →
drop-line); when blank, `target + ' '` again for drop-line, the list
(`parse_attributes sub_input`, document policy read lazily) for optional, and
`target + ' '` once more for the warn message; otherwise the list only after the
secure-mode link, the provider and depth checks. Informational drop-line
messages of directive substitutions are not reported (reader.rs
directive_diagnostics); warnings are. Highlighting reads processed lines with
Reader::plain and never executes them again.

Working: parser for header attributes, headings, listing blocks, literal blocks
(`....` and `[literal]`), admonitions (paragraph form), paragraphs,
lists (see **Lists**), and
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
`.admonition-title` and the rendered child blocks in `div.admonition-content`
(adoctra.css lays them out like Ruby's label/content table, checked with
screenshots). Like `convert_admonition`, `Admonition.icons`/`src` capture the
`icons` mode where the block converts (body_subs): `font` renders `<i class="fa
icon-TYPE" title="label">` unless the block has `icon`, other modes `<img src
alt>` from image.rs icon_uri (block `icon`, else the type, plus `icontype`,
under the document `iconsdir`); JSON/WASM export `icons`/`src`;
`golden/admonition_icons/` runs the reference in safe mode. The label is core's
`Admonition.label` (asciidoctor's `textlabel`): the block's `caption=` (raw, or
the substituted parts of a single-quoted value), else
the document's `<kind>-caption` parts while reading (built-in `Note`/…, empty
when unset) — *not* asciidoctor's
table layout; id/roles hang on the `<aside>`. This is also where adoctra first **ships a
reference stylesheet**: `adoctra-html` exposes `STYLESHEET` (`assets/adoctra.css`,
a modern theme — custom properties, `color-mix()`, logical properties — that
looks like Ruby's default stylesheet) and `convert()` inlines it into a `<head>` so output is styled out of the
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
the body. The HTML backend renders `<div class="exampleblock">` (id/roles) with
the title, then the children in `<div class="content">` (Ruby's structure: the
example's box is the content wrapper, below the title), `adoctra.css`
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
JSON/WASM serialize a `caption` field (the string, or `null`). Besides example blocks,
listing/source blocks (Ruby's `CAPTION_ATTRIBUTE_NAMES[:listing]`,
also `[listing]` paragraphs, fenced code and masquerades) number with
`listing-caption`/`listing-number` only when the document sets `listing-caption`
(no built-in default; `caption=` or a document `caption` still apply), literal
blocks never; images number with `figure-caption` (see **Images**) and tables with
`table-caption` (see **Tables**).
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
`golden/opens/`, passthrough fixtures under `golden/passthrough/`, table
fixtures under `golden/tables/`, break fixtures under `golden/breaks/` and
manual page fixtures under `golden/manpage/`).

Scaffold only: DOCX backend; the extension API exists but `parse`/`convert` do
not invoke it yet.

Index terms (`((…))` / `indexterm2:[…]` visible, `(((…)))` /
`indexterm:[…]` concealed) run after images and before links, as Ruby's
`InlineIndextermMacroRx`. `Inline::IndexTerm` / `AttributePart::IndexTerm`
keep visible text parts or concealed terms, plus `see` / `see_also` metadata.
Concealed terms have zero-length lexical fragments; flow terms use empty
boundary marks so later macros can nest in their text. Unlike UI macros,
flow terms need no formatting-boundary repair because they emit no wrapper.
HTML shows only visible text; JSON/WASM expose `indexterm` and the metadata;
the tokenizer uses parser-reported `inline-macro` spans. Covered in core and
all consumers, `golden/index_terms.adoc`, and the compatibility corpus.

Inline math (`stem:`, `asciimath:`, `latexmath:`) is extracted by `inline_pass`
after ordinary passthroughs, before quotes/attributes, as Ruby's
`InlineStemMacroRx`. `StemStyle` is selected at extraction; generic `stem:`
uses the effective `stem` attribute (`latexmath`/`latex`/`tex`, else AsciiMath).
`Inline::Stem` / `AttributePart::Stem` retain notation and substituted parts;
selectors use the shared pass pipeline (special characters by default).
HTML emits math delimiters, JSON/WASM expose `stem` with `style` and `parts`.
The tokenizer uses extraction spans and projects multiline source formulas
from the complete parse to suppress protected contents' tokens. Block math
and loading MathJax in full HTML are implemented as described below. Core/all-consumer tests,
`golden/stem.adoc`, and compatibility fixtures cover this step.

Block math uses `BlockKind::Stem { style, inlines }` on `++++` only.
`stem` resolves its second positional (including nil vs quoted empty) against
reading-time attributes. BodySubs defers content with selected stages; default
special-character protection, block selector modifiers, macros and stateful
expressions share the pass pipeline. Raw content trims blank edge lines after
substitutions. Callouts in STEM `subs` work like other blocks (see below).
JSON/WASM expose `stem` with `style` and `inlines`; `stem-delimiter`/`stem-text`
tokens come from Reader records, including nested cells and includes. HTML uses
Ruby's block delimiters and AsciiMath line splitting. `Document.final_attributes`
keeps conversion state separately from header attributes. HTML emits the pinned
MathJax 2.7.9 configuration in head when final `stem` is set, with final `eqnums`
and header `asset-uri-scheme`; equation numbering is escaped as script data.
Golden block/config fixtures and 15 compatibility cases cover the feature;
a separate Chrome 155 browser check loaded MathJax from the real CDN and passed
5 scenarios for both adoctra and Ruby, including desktop/mobile screenshots,
numbering, inline/display modes, ignore classes and body attribute replay.
Local browser artifacts are in `target/browser-stem/` (not committed).

Bibliography lists use the existing List style and neutral Anchor/Text parts
in Inline::Substituted. Direct unordered children of bibliography sections
inherit the style; nested lists and containers do not. Reading catalogs only a
leading triple-bracket anchor of an unordered bibliography item, keeping its
bracketed reftext without early attribute substitution; first id wins. Conversion
recognizes the first line-leading triple-bracket anchor in a bibliography list
item (including explicitly styled ordered/description lists), after links and
before ordinary anchors. Its label runs c/q/r again like Inline#reftext, so
already generated formatting tags become escaped text. Generated anchors render
without being cataloged. Checklist recognition is disabled for bibliography.
JSON/WASM expose parts and style; tokenizer projects the complete anchor span
from parsed substituted nodes. Covered by core/all-consumer tests, a list golden
fixture and 22 compatibility cases including include and nested cells.

**Block `subs`** (parser.rs commit_subs, Ruby's commit_subs/resolve_block_subs/
catalog_callouts) runs on completion for every block: pass_subs::resolve_block
expands groups and +/-/`name+` modifiers from the content model's Defaults
(Normal: paragraphs, styled paragraph forms, verse; Verbatim: listing/literal;
Basic: STEM; None: pass), warns "invalid substitution type(s) for <context>",
and an equal selection changes nothing. Paragraphs, verse, STEM and the single
child paragraph of a styled paragraph (`[NOTE]`, `NOTE:`, `[example]`, `[quote]`,
`[sidebar]`, `[open]`) select stages on their deferred field (BodySubs
BlockSubs); delimited containers ignore `subs`. Listing/literal/pass blocks get
`inlines: Option<Vec<Inline>>` (one Inline::Substituted, backends render it
instead of `text`), sourced from ParseContext.verbatim_bodies (fence bodies,
indented literal paragraphs without their indentation, adjusted by indentation.rs)
or the block span (`[literal]`/`[pass]` paragraphs, the latter comment-filtered).
Verbatim/raw results trim blank edge lines after substitution. With callouts,
the raw source marks register in the catalog (callouts::catalog, Ruby's
CalloutScanRx) or the stage is dropped; conversion's Stage::Callouts matches
lexical `&lt;N&gt;` marks (callouts::apply_parts, line-comment via
Passes.line_comment) into atomic `AttributePart::Callout(InlineCallout)` with
the catalog ids in order, the `icons` mode and image src at conversion. HTML
renders Ruby's conum markup (guard lexical), JSON/WASM kind:"callout".
Every callout icon (ast::Callout, CalloutListItem, InlineCallout: `icons`
mode and image `src`) is captured by body_subs where the block converts, like
Ruby's convert_inline_callout/convert_colist, not from the header; image
icons are vertically centred in `pre` and colists (checked with screenshots).
With header `icons=font` the HTML head links the icon font like Ruby's
convert_document: Font Awesome 4.7.0 from cdnjs (`asset-uri-scheme`,
`iconfont-cdn`) while `iconfont-remote` is set — a core default, since adoctra
HTML is standalone — else `<iconfont-name|font-awesome>.css` under `stylesdir`
(core fallback `.`) via the public `normalize_web_path(…, false)`.
adoctra.css ends with Ruby's built-in roles (16 colours and `-background`s,
big/small, underline/overline/line-through, Asciidoctor's values) plus rules
reproducing Ruby's cascade: a colour role on a block does not recolour
verbatim `pre` (nor its background), admonition, quote/verse text, table
cells, section titles or code, while a role on the inner element still
applies (checked against Ruby in Chrome).
Highlighting keeps only inline tokens of executed stages inside such text
(Reader.custom_subs; verbatim paragraph forms have none by default) and
projects converted marks as `callout` (Reader.callout_marks).

**Delimiter lengths**: lists.rs `delimiter` is Ruby's `is_delimited_block?` with
`BlockMatchData` (tip + terminator), checked on the unindented line (an indented
fence is a literal paragraph). `----`/`....`/`====`/`****`/`____`/`++++` may be
longer uniform runs; only the identical line closes a block (collect_blocks
bounds a container by it, read_container/capture_verbatim_body compare exactly),
so blocks of one kind nest by fence length. DELIMITED_BLOCKS masquerades:
`[literal]` on `----` is a Literal, `[listing]`/`[source]` on `....` a Listing.
Styled non-verbatim paragraphs end at delimiters and block attribute lines like
plain ones (StartOfBlockProc); verbatim paragraphs (`[literal]`/`[source]`/
`[verse]`) only at a blank line or `+`. The parser records every fence and verbatim
body line in Reader.block_tokens; the tokenizer projects them instead of its own
fence toggles (masquerade bodies are text tokens of their kind, AsciiDoc cells
and includes too).
**Breaks and fenced code**: parser.rs layout_break is Ruby's layout-break branch
of `next_block` with Markdown syntax: `'''`+ is `BlockKind::ThematicBreak`, `<<<`+
`BlockKind::PageBreak`, and MarkdownThematicBreakRx (`---`, `- - -`, `***`,
`___`, uniform spaces, indented up to three spaces) a thematic break. They are
checked at a block start before block macros (not in a list item's text-only
block or under a verbatim style) and never interrupt a paragraph. Metadata is
kept, but like an unrendered passthrough the title is not executed; HTML emits
Ruby's `<hr>` / `<div style="page-break-after: always;"></div>` without
metadata, JSON/WASM the ASG `name:"break"` with `variant` thematic/page, tokens
`thematic-break`/`page-break` from the parser. Fenced code (```` ``` ````, tip of
lists.rs delimiter) is a `Listing` closed by exactly three backticks:
MacroMeta.style forces `source` over the metadata style (its `source` rekeying
does not apply, block_attributes::unkeyed_fields) and fenced_code_attributes
gives `language`/`linenums` like Ruby (a leading comma keeps the whole text).
**Source defaults and verbatim indentation**: parser.rs source_defaults is Ruby's
`:listing`/`:source` branch at block completion: a Listing without a style
becomes `source` when its second positional or the reading attribute
`source-language` gives a language (an empty value counts; `[#id]`/`[foo]`
follow style); a `source` listing takes `source-language` without its own;
`linenums` from positional 3 (block_attributes rekey_linenums), `%linenums`/
`linenums-option` or `source-linenums-option`; `indent` from `source-indent`.
Fenced code replaces even a metadata language with the default when its fence
has none. `[listing]`/`[source]` paragraphs are verbatim Listing blocks (end
only at a blank line or `+`), `[listing]` also masquerades on `--`.
indentation.rs ports `adjust_indentation!` (tabs to character tab stops, then
common-indent removal and new indent; Ruby `to_i`, values clamped at 4096) over
a FilteredSource whose generated segments pin padding (zero width) and expanded
tabs (the tab byte) to physical bytes. adjust_verbatim applies the block's
`indent` and block/document `tabsize` to Listing/Literal `text` and stores it
in ParseContext.verbatim_bodies for `subs` conversion and callout catalogs;
verse bodies are deferred adjusted (Reader::scan_verse). Indented literal
paragraphs keep Ruby's plain strip (no indent/tabsize). Callout spans align
from the line end, so they survive the adjustment.
**Simple content**: a styled paragraph form (`[NOTE]`/`NOTE:`, `[example]`,
`[sidebar]`, `[open]`/`[abstract]`, `[quote]`) keeps its paragraph as the only
child and sets `Block.simple` (Ruby's `content_model: :simple`); delimited
blocks never do. part_content clears it for a `[partintro]` paragraph and a
rebuilt `[open]` intro, like Ruby's next_section. HTML render_body writes the
child's inlines without `<p>` (Ruby's `content`); JSON/WASM add
`"simple":true` to such containers. The body-dom-v1 profile projects Ruby's
admonitionblock table and adoctra's `aside.admonition` to one admonition node
(type, id, roles, text label, body) and compares blockquote content strictly.
**Markdown blockquotes and quoted paragraphs** (parser.rs quote_paragraph,
Ruby's ordinary paragraph branch after the admonition label): an unstyled
paragraph (no style, `normal`, or one Ruby does not know as a paragraph style;
not a list item's text-only block) whose first line starts with `> ` is a
compound `Quote`: line comments are skipped, `> `/`>` markers stripped (lazy
lines kept), a final `-- ` credit line popped with blank lines before it, and
the rest parsed as blocks from a `Reader::nested` at the stripped offsets
(Scope::Container: sections are text, nested `> ` quotes need a new
paragraph). A paragraph starting with `"` whose last line is a `-- ` credit and
whose previous line ends with `"` is a simple `Quote` of the text between the
quotes. Both force style `quote` via MacroMeta (positional fields are not
rekeyed). The credit gets normal subs while reading (reading_title, effects
reach reading only; an empty result has no attribution) and stays whole in
`author` (ParseContext.credits); its xrefs resolve after the quote's children
(resolve.rs resolve_attribution, attribute-line attributions before them), then
blockquote_credit.rs splits it like Ruby's `split ', ', 2` of the converted
markup: the first `, ` of contiguous output text (Text/Raw/plain attribute
values, adjacent leaves joined; markup between breaks contiguity) in document
order, also inside formatting, links, xref labels and Substituted parts. Ruby
then leaves those tags open; the browser keeps the `cite` inside the innermost
element and closes it with it, so core builds that DOM from well-formed
pieces: author (elements closed), citetitle (the innermost element's
remainder inside copies of its ancestors) and `Attribution.trailing` (the rest,
inside copies of the outer elements), rendered after `</cite>`; JSON/WASM
export `trailing` only when non-empty. adoctra.css hides the attribution `<br>`
and makes `cite` a block like Ruby's stylesheet (checked in Chrome). Inline anchors
of the stripped lines are cataloged again after the nested parse, like Ruby
(a duplicate warns). Markers are `quote-delimiter` tokens (Reader.quote_markers;
the tokenizer classifies the rest of the line).
**Collapsible examples and nowrap**: assign_caption gives an Example with
the `collapsible` option (`%`, `options=`, `-option`) `caption = Some("")`
(Ruby's `:example` branch sets caption ''), so it never numbers and
`caption=` is ignored. HTML renders Ruby's `<details>` (id, roles, `open`
option) with `<summary class="title">` = uncaptioned title or verbatim
`Details`, children unwrapped. `prewrap` is a core default attribute; HTML
adds `nowrap` to listing/source/literal `<pre>` with `%nowrap` or without
`prewrap` in the block's conversion attributes. adoctra.css wraps `pre` like
Ruby (`pre-wrap`, `pre.nowrap` keeps lines) and styles `details` markers.
Like Ruby's `Block#content`, parser.rs trim_verbatim_edges drops blank edge
lines of a multi-line Listing/Literal/Passthrough `text` at block completion
(before callouts), also without custom subs.

Not yet implemented (common asks — treat as roadmap, do not assume present):
broader language coverage. The full block-attribute list is now *parsed* (style, named
attributes, `%options`, id/role folding), but most of it has nothing to render
against yet. Math styles (`[stem]`/`[asciimath]`/`[latexmath]`) apply to `++++`
only, as in Ruby. `[source,<lang>]`, `[literal]`, `[pass]`,
`[example]`, `[sidebar]`, `[quote]`/`[verse]`, `[open]` and the admonition styles
are wired. The **passthrough block** (`++++` / `[pass]`) is done — verbatim and
emitted *completely raw* (no escaping, no wrapper, id/roles dropped). The **open
block** (`--` / `[open]`) is done, including *both* its *container* masquerades
(`[quote]`/`[example]`/`[sidebar]`/`[NOTE]` on `--`) and its **verbatim**
masquerades (`[source]`/`[literal]`/`[verse]`/`[pass]` on `--`, switched at the
`--` fence into a `Listing`/`Literal`/`Verse`/`Passthrough`). `%hardbreaks`
affects inline substitutions and `%nowrap`/`prewrap` render (see **Collapsible
examples and nowrap**); other options are carried but mostly not rendered. For
**admonitions**, the *paragraph* forms (`[NOTE]` style and `NOTE: ` label) and the
**delimited form** (`[NOTE]` on a `====` block) are done. **Block titles**
(`.Title`) are kept on `Block.title` and rendered except on passthrough blocks; still missing
here: admonitions on list items. **Caption numbering** is done for **example
blocks** (`Example N.` via the `example-caption` attribute, on `Block.caption`) and
images (`Figure N.`), listings (`Listing N.` with a document `listing-caption`)
and tables (`Table N.`). The unset forms
`:!example-caption:` / `:example-caption!:` disable example captions and their
counter increments; a custom per-block `caption=` or document `caption` overrides
the prefix without incrementing the counter. Also: ids of individual list items,
incremental/block reparse, and a pull/event API over the AST.

**Standalone document**: `HtmlBackend::new()` writes Ruby's
`convert_document`: head metadata (`lang`/`nolang`, `encoding`, generator
adoctra, `app-name`, `description`, `keywords`, `author` from
`Document::authors_line`, `copyright`, `favicon`, `<title>` = Ruby's sanitized
doctitle with fallbacks), the stylesheet branch (`stylesheet` ''/`DEFAULT` =
adoctra.css inlined or linked under `stylesdir` with `linkcss`, an explicit
`webfonts` link, a custom sheet linked or read through the resources, none
without `stylesheet`), icon font, head docinfo, `<body id class>` (doctype,
auto-TOC `toc-class`/`toc-<position>`, the `role` attribute), header docinfo,
`<header id="header">` (title unless `notitle`, `.details` from
`Document::authors` and revnumber/revdate/revremark, then the auto TOC;
manpage: title, TOC, NAME), `<main id="content">`, footnotes,
`<footer id="footer">` (version and `last-update-label docdatetime` unless
`reproducible`, from final_attributes), MathJax and footer docinfo (final
attributes). `HtmlBackend::with_resources(provider, referrer)` reads custom
stylesheets and docinfo (`adoctra_core::docinfo`: docinfo/docinfo1/docinfo2,
docinfodir, `docinfosubs` default attributes); `render` returns the warnings.
`HtmlBackend::embedded()` (CLI `-s`) is Ruby's `convert_embedded` (title hidden
by `notitle`). `ParseOptions.embedded` mirrors Ruby's standalone option
(`embedded` locked, embedded `notitle` default unless host
`showtitle`/`notitle`; standalone `copycss`/`iconfont-remote`/`stylesheet`/
`webfonts`), and a `showtitle` entry stores the inverse `notitle` first.
`ParseOptions.dates` (host clock, core reads none) fills Ruby's
`local*`/`doc*` date attributes; the CLI passes `SOURCE_DATE_EPOCH` or local
time (chrono, CLI only) and the input mtime, accepts `-a name[=value]`, creates
the output directory and copies stylesheets with `linkcss`+`copycss`. The
bench runs `-s -a showtitle`; `golden/document/` compares the standalone shell
with `asciidoctor-reference.rb --standalone`. adoctra.css lays out the header,
details, footer and the fixed `toc2` sidebar like Ruby (checked with
screenshots).
**Source highlighters**: adoctra-html highlight.rs ports Ruby's client-side
SyntaxHighlighter adapters. `Highlighter::of(header)` is save_attributes'
choice (`source-highlighter` in the header snapshot, basebackend html, no
`<name>-unavailable`; Cx carries it, so AsciiDoc cells share it):
`highlightjs`/`highlight.js` (`<pre class="highlightjs highlight">`, code
`language-<lang|none> hljs`), `prettify` (`prettyprint`, `linenums[:start]`),
`html-pipeline` (`<pre class="listing" lang>`, listing = Ruby's wrapper);
id/roles on the `<pre>` as for plain source. coderay/pygments act like Ruby
without their gems (the reference environment): `CodeRay`/`pygments` pre
class and Ruby's `optional gem … not available` warning in Output.warnings,
once when a source listing converts (Cx.source_listing).
Head theme and footer scripts (before MathJax) read final attributes with the
header `asset-uri-scheme`; embedded output has none. Unknown names keep the
plain `highlight` listing; rouge is server-side (below).
**Rouge port** (user decision 2026-10-31: all 228 lexers before claiming
support): crate `adoctra-highlight`; `scripts/generate_rouge.rb` generates
declarative rules from the pinned Rouge classes, Ruby block rules are ported
by hand per lexer. Done so far: `adoctra_highlight::regex`, a std-only
backtracking Onigmo engine. `Regex::match_at(rest, 0)` is
`StringScanner#skip` (the subject is the scanner's rest: `\A`/`^`/`\b` at
the scan position, no lookbehind before it); `search` is `=~`. Onigmo rules
kept: `(?i)` without colon covers later alternatives of its group, `{n,m}+`
is not possessive, `\w\d\s\h` ASCII (never case-extended) but `\b` Unicode
Word, `/i` classes add no Latin-1 (U+0080-U+00FF) variants, one-char class
= literal, `\NN` octal without that group. Lookaround/atomic/`\g<0>` are
committing sub-matches; a 5M step budget bounds backtracking. Unicode
property/POSIX/fold tables (`regex/unicode_tables.rs`) and the conformance
corpus (`tests/data/regex_corpus.txt`: all 5913 rule regexps at every demo
position, FNV fingerprints) are generated (`generate_rouge.rb unicode|
regex-corpus`, verified in CI); every new pattern feature needs a Ruby probe.
`token`/`theme`/`format` (HTML, HTMLInline, Asciidoctor's line highlighter
and line table) and `lexer` (RegexLexer runtime: lazy `[:root]`, mixins, 5
null steps, Error per unmatched char, `LexError` where Rouge raises, merged
`continue_lex`) are done; tokens/themes (via the RougeAdapter itself) and
`generated/lexers.rs` come from `generate_rouge.rb tokens|themes|lexers`.
The lexer export translates Ruby block ASTs (token/groups/push/pop!/goto/
reset_stack/delegate/recurse, `m[i]`, `include?` on per-class String
collections or closed-over locals, regexps, state tests, `case`, String/
Token locals declared up front, `if` token expressions); `start` blocks and
`initialize` become hooks (push, `@x = K.new(options)`, `x.reset!`);
delegation to ivars or memo methods (`@x ||= K.new(options)`) uses
persistent `Lexer.subs` (`TemplateLexer#parent` hand-written). Hand-written
`rule_<line>`/`start_<line>`/`init`/`reset`/`stream` in
`src/lexers/<rouge file>.rs` win; anything else lands in `LexerDef.missing`
and the lexer is unused, as is one delegating to an unused lexer (166 of
228 ported, now all 228). Also translated: delegate aliases (`@lang = @css`),
`coll.any? { |x| … }`, nil-aware `m[i]`/ivar comparisons, stack sizes,
String ivars (`Lexer.strings`) and `Regexp.escape`, parenthesized conditions, methods of closed-over
modules, `values.any? { |c| c.include? x }` (union), `include?` over only
the String members (Symbols never equal a match), `delegate self.clone`
(re-entrant: the clone shares the stack), and a missing constant becomes
the NameError Ruby raises when the rule fires; `puts … if @debug` is
dropped. Token methods of the lexer returning a token or nil
(`name_token(m[0]) || Name`, token parameters like apache's
`name_for_token(x, Name::Label)`), `push do … end` states (plain rules and
mixins; regexps interpolating String locals are cached per value), `next`,
`case … when /re/`, `/re/.match`, `"…#{x}"`, `a | b` unions, `map[k] || k`
and `casecmp` are translated too; a block closing over different values
(graphql's `pop_unless_list[t]`) gets one callback per translation.
`guess.rs` ports `Lexer.guesses`/`guess`/`lookup_fancy` (Mimetype,
Modeline and Source guessers over `Lexer.all` order; generated `detect?`,
hand-written `detect` for racket); `G` corpus lines check it against Ruby.
`examples/lex_dump.rs` prints a lexer's tokens for debugging. Hand ports so far:
markdown (fences via `lookup_fancy`/guess, unclosed fences line by line),
http (body lexer by `Content-Type` or the `content` option), slim/haml
(filters; haml's from `initialize` and `filters[name]=lexer` options), vue
(`lookup_lang`), meson (string register). `Lexer.vars` keeps one value per
type (a lexer's and a mixin's), `Lexer.classes` holds lexer-class ivars
(`@lang = CSS`, delegation makes a new instance); `until`/`while` and a
token-valued `case` are translated, as are `:"params#{name}"` states
and `a + b` strings; tcl (string brace count), cmake (state tokens,
bracket length), coq (dotted ids), epp/escape/lasso (lexer options) are
hand-written; sed's unregistered `Sed::Regex`/`Sed::Replacement` are
exported under qualified names `Lexer.find` never finds, ada (idents), ecl
(sized types), wollok (`@entities`), plaintext, template_lexer (`parent`), console
(+irb), python (string register), yaml (+digdag: indentation stack), rust (macro delimiters), crystal (ruby's helpers, Crystal sigils), elixir (`~` sigils; no letter = Ruby's TypeError), lua/moonscript (builtins; lua's Symbol-key options never apply), handlebars (parent `:attr` pop), util (Rouge::Indentation: reset!/starts_block/indentation, used by sass/haml/slim), php (+hack: options, start state, builtins), ruby (`%`-sigils build a cached dynamic state: `dynamic_state`/
`Rule::Compiled`; heredoc queue); a hand module declares its delegates as
`Delegates (tag, …): dep, ….` doc lines and the Rouge data it needs as
`Data: Klass.method, ….` (exported as `DATA_KLASS_METHOD` slices). `tests/data/lexer_corpus.txt` also
holds `C` cases (`find_fancy` names with options) from `LEXER_CASES` in
generate_rouge.rb (all matching Rouge, incl. hand-written plaintext, all matching Rouge's token streams
on the demos in `tests/data/lexer_corpus.txt`). adoctra-html `rouge.rs` is
the RougeAdapter: `<pre class="rouge highlight">`, `rouge-css` class (head
`<style>` inserted after the body like Ruby's docinfo, `<link>` with
linkcss; `Output.highlighter_stylesheet` for the CLI's copycss) or inline
(pre base style), `rouge-style` (class mode: first listing wins), linenums
table/start, `highlight` lines, callouts extract/restore, `lang?opts`, php
`start_inline`, plaintext fallback (unported lexer or LexError), and
`Block#content`'s blank edge-line trim. Listings with default subs
highlight in the adapter; custom subs keeping specialcharacters highlight
inside core's subs pipeline (below). `golden/rouge/` compares standalone
documents. All 228 lexers are ported; j, robot_framework and sas are
hand-written, and a state Rouge cannot load (ceylon's unreachable
`comment`, sas's `datalines4`) is in `LexerDef.broken`: entering it fails
like Ruby. Custom subs keeping specialcharacters highlight in core
(`highlight_stage.rs`, Ruby's `highlight` sub): core extracts callouts,
calls `ConversionStart::highlight` (`HighlightRequest` → `Highlighted`;
`HtmlStart` answers for rouge from the header and shares the adapter state
with `render_with_start`), fixes highlighted pass slots and splits the HTML
into Raw tags and decoded Text, so later stages see Ruby's string; marks
return as Callout parts. Without the hook it is Ruby's `sub_source`. The CLI
passes the hook in embedded mode too; `golden/rouge/` parses with it.
highlight.js/prettify are available today. adoctra.css
carries Ruby's `pre.highlightjs`/`pre.prettyprint` rules (checked with
screenshots). `golden/highlight/` runs the reference `--standalone --safe`.
**Safe mode**: `ParseOptions.safe: SafeMode` (Unsafe/Safe/Server/Secure,
default Safe — Ruby's API defaults to Secure, its CLI to Unsafe; providers
own access policy) sets Ruby's `safe-mode-*` overrides; `allow-uri-read` is
host-only. Server+: `copycss`/`source-highlighter` locked unset, `backend`
locked html5, `docdir` '' and `docfile` relative, `user-home` `.`. Secure:
`max-attribute-value-size` 4096 (ParseContext truncates entry values with
`limit_bytesize` over parts), `linkcss` and `icons` locks, include directives
become generated `link:target[role=include]` lines (reader.rs include_link,
also without a provider and in verbatim), no docinfo (HTML) and no CLI
stylesheet copy. URI includes (or relative targets in a URI include) without
`allow-uri-read` become the same link; with it the provider loads them; a URI
stylesheet is read through the HTML resources. `SafeMode::of(attributes)`
reads the level back. The CLI takes `-S`/`--safe-mode`/`--safe` (default
safe), unjails FileResources in unsafe mode, passes `user-home` below server
and fetches http/https with `ureq`; the bench runs it with `-S secure`
(include fixtures `-S safe`). URI includes with `cache-uri` now call the optional
`ResourceProvider::enable_uri_cache` hook after permission/depth checks. The
default is a no-op for host providers/WASM; native FileResources keeps a sticky
in-memory cache for its lifetime. Cache full successful responses before
decoding/selection, retain Content-Type, retry HTTP errors, and never cache local
files. Hits decode as BINARY, ignoring new encoding, like open-uri-cached 2.0.0;
Ruby exceptions remain separate from successful comparisons. Byte budgets and
snapshot identity still apply on every include. URI optional misses are errors;
their report uses the resolved URI and directive cursor. document_path must
return None for URLs so unresolved nested includes retain their URL.
The pinned optional gem version is in compatibility/open-uri-cached-version.
scripts/audit_uri_cache.py compares HTML projection, positioned diagnostics and
request traces against local HTTP servers: 32 PASS / 2 RUBY_ERROR. CI verifies
the complete compatibility/uri-cache/report.json baseline. Images with data-uri,
inline SVG and embedded stylesheets now activate the same cache after permission
checks. SVG uses ResourceProvider::load_asset_text (default strict binary decode
with BOM detection); native cache hits preserve BINARY decoding. Do not replace
bad SVG bytes with U+FFFD. Asset warnings keep editor spans but have Unlocated
conversion reports. The separate --assets audit compares exact styles/links as
well as body DOM, diagnostics and requests: 39 PASS in assets.json.
Only the three known standalone shell tags are mapped to their Ruby divs.
Like Ruby's `convert_document` (head after `<title>`, before the header outline
and body), the stylesheet is read before body images: core calls the public
`ConversionStart` hook (backend.rs) once in body_subs Execution::execute after
the document title, with the header attributes, and reports its warnings as
Unlocated conversion diagnostics there; `parse_resource_with_start`/
`parse_with_start` take it. `HtmlBackend::start()` reads the custom stylesheet
(and enables the cache) then; `render_with_start` reuses it (no second read).
The CLI passes it for standalone HTML. Plain `render` still reads at render.
Do not normalize request order away. CI fails on baseline drift and XPASS.
Cross-provider/process/disk caching, URI normalization and untested
encoding/selection combinations remain open; jail recovery needs an
absolute docdir (otherwise providers deny). Do not replace explicit provider policy with ambient I/O.

**Include edges**: reader.rs `directive` follows Ruby's IncludeDirectiveRx
(no empty target, none starting/ending with whitespace: such lines are text).
Without a provider a URI target is still linked when Ruby may not read it
(no `allow-uri-read`, positive host `max-include-depth`: Reader
unread_includes); file targets stay literal. `skip-front-matter` skips a
leading `---`…`---` block of the primary input (before the header) and of
each include (Input::skip_front_matter, by the input cursor, physical spans
kept) and sets `front-matter` (a reading effect inside the body).

**Titles and block anchors**: parser.rs parse_heading is Ruby's
ExtAtxSectionTitleRx (`=` or Markdown `#` markers, 1-6, spaces/tabs, an
optional closing marker of the same length after a blank: `== T ==`).
parse_block_anchor is BlockAnchorRx: ids `[\p{Alpha}_:][\p{Word}\-:.]*` (no
leading digit), and an empty `[[]]` is an anchor that clears the pending id.

**Fuzzing** (`scripts/fuzz.py`, profile `fuzz-v1`, docs/fuzzing.adoc): seeded
mutations of `compatibility/cases.json` sources and golden fixtures plus deep
nesting families. Stage `robust` runs the `adoctra-wasm` example `fuzz`
(editor path, 1 MiB worker stack, panics/aborts/timeouts, main-source span
bounds and char boundaries); stage `diff` compares the CLI with a persistent
Ruby worker (`scripts/asciidoctor-batch.rb`) on the acceptance body DOM and
diagnostics and minimizes differences. CI runs `robust` (seeds 1–3, depths
8/16). Fixed from findings: `lists::delimiter` byte slicing, metadata past the
end of input, anchored ATX headings/block anchors (no leading whitespace),
Ruby strip semantics (`ruby_strip.rs`: ASCII whitespace and NUL; Unicode spaces
are content), `unterminated <context> block` after the style masquerade
(`unterminated_context`), excluded abstract/partintro skipped by conversion
(`BodySubs::excluded`), section diagnostics inserted at their reading position
(`Section::diagnostics_before`), unknown paragraph style reset before the
literal check, `Reader::content` normalizing line endings to LF. Harnesses
read process output as bytes (`compatibility.run`, `acceptance.invoke`,
`read_reference`): text mode turned a lone CR into LF. Block reading has no recursion per level (`read_blocks`: a heap stack of
`Run` frames; containers, list items and blockquotes suspend as `Step::Collect`
with a `'static` `Resume` and outer `Then`s; lists implement `ItemList`, ready
items continue in place). Open: AST passes, backends and drop still recurse
(about 1024 levels on a 1 MiB release stack; user decision: remove the
recursion, no limit); AsciiDoc table cells read nested documents recursively
(bounded by separator syntax), cross-pass diagnostic order
(XFAIL `fuzz-preprocessor-error-order`, `fuzz-missing-attribute-in-title-order`)
and five body XFAILs (`fuzz-*` in `compatibility/cases.json`).
