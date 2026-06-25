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

Working: parser for header attributes, headings, listing blocks, paragraphs,
flat lists (unordered `*`/`-`, ordered `.`; one level, single-line items), and
inline `*strong*` / `_emphasis_`, `` `monospace` `` / `#mark#` (constrained)
and `^superscript^` / `~subscript~` (unconstrained); a single inline scanner
(`adoctra-core/src/inline.rs`) shared by the parser and tokenizer; inline nodes
carry byte spans; attribute references `{name}` are resolved once in core
against the document attributes; HTML and JSON backends; editor layer
(error-tolerant `parse`, spans, diagnostics, tokenizer); WASM `parse_json` /
`tokens_json`; golden compatibility tests against asciidoctor
(`adoctra-html/tests/golden/`, list fixtures under `golden/lists/`).

Scaffold only: DOCX backend; the extension API exists but `parse`/`convert` do
not invoke it yet.

Not yet implemented (common asks — treat as roadmap, do not assume present):
broader language coverage (nested lists, explicit-numbered `1.` and continuation
list items, tables, links/xrefs, footnotes, admonitions, includes, conditionals,
block/inline macros, typographic `REPLACEMENTS`), TOC generation,
incremental/block reparse, and a pull/event API over the AST.

When asked to "add a feature," check this list first and say plainly whether it
is new work or wiring up an existing seam.

## Reference

A GitLab project, `fdc-marketplace`, is the practical reference for broader
AI-assisted development conventions (code review, CLAUDE.md style, plugins, RAG
document management with SVN).
