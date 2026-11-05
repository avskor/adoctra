# Changelog

All notable changes to this project are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- Windows: file names in messages and the `docfile`/`docdir` attributes
  no longer carry the `\\?\` prefix (`\\?\C:\Work\a.adoc`); they are
  written like Asciidoctor's, `C:/Work/a.adoc`.
- DOCX: diagrams are embedded when the output goes to another directory
  (`-o c:\tmp\out.docx`), where their images are written, instead of
  warning "image not found or unreadable".

## [0.0.2] - 2026-11-05

### Fixed

- Windows: absolute paths with a drive letter or backslashes
  (`-o c:\tmp\out.docx`, `-D`, `-B`, includes and images) are resolved
  as absolute paths, like Asciidoctor on Windows, instead of being joined
  to the document's directory (which failed with an invalid file name
  error `@ dir_s_mkdir - …/C:`).

## [0.0.1] - 2026-11-05

First public release. An early version: the processor converts real-world
documents, but the API and output may still change between releases.

### Added

- `adoctra` command-line tool with the options of the `asciidoctor`
  command for document conversion (`-b`, `-d`, `-e`, `-o`, `-D`, `-R`,
  `-B`, `-S`, `-n`, `-a`, `--failure-level`, `-q`, `-v`, `-t`).
- Parser compatible with Asciidoctor 2.0.23: preprocessor directives and
  includes, document header and attributes, sections, all delimited
  blocks, lists, tables, images and media, cross references, footnotes,
  index terms, STEM, table of contents and the full substitution pipeline.
- HTML5 backend with modern semantic markup and the bundled `adoctra.css`
  stylesheet, visually matching Asciidoctor's default output.
- Template-based DOCX backend (`--to docx`, `docx-template` attribute).
- JSON backend with the document tree, attributes and diagnostics.
- Server-side syntax highlighting: a port of Rouge with all 228 lexers
  (`source-highlighter=rouge`).
- Diagram extension: all 58 diagram types of asciidoctor-diagram 3.2.1
  through their tools, Kroki or a PlantUML server (`-r asciidoctor-diagram`).
- Rust extension API: block, block macro, inline macro and tree processors.
- WASM bindings for editors: error-tolerant parsing with byte spans,
  diagnostics and highlighting tokens, a virtual file system for includes,
  and diagrams through Kroki.

[0.0.2]: https://github.com/avskor/adoctra/releases/tag/v0.0.2
[0.0.1]: https://github.com/avskor/adoctra/releases/tag/v0.0.1
