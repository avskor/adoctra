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
- `adoctra-highlight` — std-only port of Rouge (pinned in
  `compatibility/rouge-version`, MIT, `LICENSE-rouge`) for
  `source-highlighter=rouge`; depends on nothing in the workspace (a backend
  may depend on it, core and WASM never).

Dependency direction is one-way: `cli -> adoctra -> {core, backends}`, each
backend depends only on `core` (plus `adoctra-highlight`). Keep it that way. `core` must never depend on a
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
  `std`-only right now (the CLI uses `chrono` for the local clock and `ureq`
  for URIs); the
  `thiserror`/`docx-rs`/`clap`/`serde` lines in the
  root manifest are commented placeholders. Add a dependency only when it earns
  its place, and prefer `cargo add` so versions stay current.
- WASM glue stays behind `#[cfg(target_arch = "wasm32")]`; the JSON
  serialization layer in `adoctra-wasm` must remain pure Rust so it compiles and
  unit-tests on the native target.
- **`unsafe` is forbidden** (user decision 2026-10-31): no `unsafe` blocks,
  functions or impls in any crate. Solve borrowing problems safely (owned
  frames, `std::mem::take`, indices) even when `unsafe` looks simpler.
- Run `cargo fmt` and keep `cargo clippy --workspace --all-features` clean.
- **Localization works exactly like Asciidoctor Ruby.** Every reader-visible
  word is a document attribute with Ruby's built-in default (`toc-title`,
  `*-caption`, `*-refsig`, `*-signifier`, `preface-title`, …) that documents
  override (`:toc-title: Содержание`). Never hardcode localized text or add an
  own i18n layer/message codes; diagnostics and fixed output strings (e.g.
  `<!-- toc disabled -->`) stay verbatim English, identical to Ruby.
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
cargo run -p adoctra-cli -- examples/sample.adoc --to html -s --stdout   # embedded
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

The full, authoritative implementation record (≈150k chars: every feature,
the Ruby code it ports, invariants to preserve, audit numbers) lives in
**`docs/implementation-notes.md`**. It is deliberately *not* loaded into every
session: before touching an area, `grep` it for the topic (bold lead-ins such
as `**Tables**`, `**Lists**`, `**Footnotes**`, `**Safe mode**`, `**Rouge port**`,
`**Include edges**`, or file/type names like `body_subs`, `attribute_macros.rs`,
`ResourceProvider`) and read the relevant paragraphs. When you complete a step,
update that file (and `docs/implementation-plan.adoc`/`docs/compatibility.adoc`),
**not** this one — keep CLAUDE.md short (well under the 150k-char limit).

### User decisions and priorities

- **Project goal (user decision 2026-10-31):** convert documents, build the
  syntax tree and support the editor. Ruby API compatibility (extension
  registries/classes, highlighter factories, logger, parse_header_only, Ruby
  objects in options, `-T`/`-E`/`-r`/`-I`) is not a goal; leave those audit
  entries UNSUPPORTED. Observable document conversion and CLI behavior are
  still compared with Ruby.
- **Priority:** complete compatibility with pinned Asciidoctor 2.0.23
  (`compatibility/asciidoctor-version`) comes before performance work,
  incremental parsing, DOCX or new extension APIs. Follow the
  compatibility-first queue in `docs/implementation-plan.adoc` and the open-gap
  register at the start of `docs/compatibility.adoc`; read both before
  compatibility work and update their status when completing a step.
- DocBook/manpage *backends* are deferred together with DOCX (user decision
  2026-10-31) — do not start them unprompted. Do not resume allocation
  micro-optimizations as the next task.
- All 228 Rouge lexers are ported (`adoctra-highlight`); the user required all
  of them before claiming Rouge support.
- The CLI's default safe mode jails output too (user decision 2026-10-31).

### Compatibility discipline

- Passing the normalized corpora is not proof of full compatibility; an
  expected failure is not evidence of conformance. Keep known differences
  explicit and visible in the audit (visual parity, CLI behavior, diagnostics,
  resource loading, untested language/encoding combinations). Historical
  exceptions are limitations of any compatibility claim, not completed work.
- HTML keeps modern semantic markup by design; byte-exact Ruby HTML is not a
  goal, maximal *visual* parity is (`visual-v1`, `scripts/visual.py`,
  `docs/visual.adoc`; check with safe-mode Ruby + Chrome screenshots).
  Re-record baselines only after reviewing an IMPROVED diff; never loosen
  tolerances.
- Never suppress or normalize away diagnostic differences in a harness:
  document them and freeze the complete actual result as XFAIL; drift and
  XPASS fail CI.
- Do not describe the scoped document-processing contract as unrestricted
  replacement of Asciidoctor. Never claim the entire encoding surface is
  covered.
- Do not replace explicit `ResourceProvider` policy with ambient I/O; neither
  core nor the parser loads files or URLs on its own.
- Every reader-visible label is an attribute with Ruby's default (see
  Conventions); diagnostics stay verbatim English.

### Compatibility harnesses (details in the docs named)

| Harness | Manifest / report | Doc |
|---|---|---|
| `scripts/acceptance.py` (`body-dom-diagnostics-v1`) | `compatibility/acceptance/cases.json`, `compatibility/upstream/cases.json` | `docs/acceptance.adoc`, `docs/upstream-tests.adoc` |
| `scripts/structure.py` (`block-structure-v1`) | `compatibility/acceptance/structure.json` | `docs/acceptance.adoc` |
| `inline-semantics-v1` | `compatibility/{upstream,acceptance}/semantics.json` | `docs/semantic-profile.adoc` |
| `diagnostic-positions-v1` | `compatibility/acceptance/positions.json` | `docs/diagnostic-positions.adoc` |
| `scripts/upstream_audit.py` (whole pinned upstream suite) | inventory under `compatibility/upstream/` | `docs/implementation-notes.md` |
| `scripts/cli_matrix.py` (`cli-v1`) | `compatibility/cli/cases.json`, `report.json` | `docs/cli.adoc` |
| `scripts/visual.py` (`visual-v1`) | `compatibility/visual/report.json` | `docs/visual.adoc` |
| `scripts/audit_uri_cache.py` | `compatibility/uri-cache/report.json` | `docs/implementation-notes.md` |
| `scripts/audit_include_order.py` | `compatibility/encoding-order/` | `docs/implementation-notes.md` |
| `scripts/performance.py` | `performance/*.json` | `docs/performance.adoc` |

Keep performance claims separate from conformance. The source/context API is
documented in `docs/parser-api.adoc`.

### What exists, in one paragraph

A Ruby-faithful reader/parser (preprocessor with conditionals, includes with
selection/encodings/limits, header and attributes with full substitution
pipeline and stateful set/counter evaluation, sections/numbering/special
sections/book parts/manpages, all delimited blocks and masquerades, lists,
callouts, tables incl. AsciiDoc cells, images/SVG/data-uri/video/audio,
xrefs/anchors/footnotes/index terms/UI macros/STEM, TOC, block `subs`,
source highlighters incl. the Rouge port), HTML backend (standalone and
embedded, XML syntax, `adoctra.css`), JSON backend, WASM bindings with spans,
tokens and diagnostics, and a CLI matching Ruby's options. **Scaffold only:**
DOCX backend; the extension API exists but `parse`/`convert` do not invoke it.
**Not yet implemented:** incremental/block reparse, a pull/event API, DocBook/manpage backends, Ruby
API/extensions. (Admonition labels in list items and list item anchors were
rechecked against Ruby on 2026-11-01 and match.)

When asked to "add a feature," check `docs/implementation-notes.md` first and
say plainly whether it is new work or wiring up an existing seam.

## Reference

A GitLab project, `fdc-marketplace`, is the practical reference for broader
AI-assisted development conventions (code review, CLAUDE.md style, plugins, RAG
document management with SVN).
