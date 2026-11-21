# @adoctra/wasm

The [adoctra](https://github.com/avskor/adoctra) AsciiDoc parser for the
browser, compiled to WebAssembly: the syntax tree, an editor view,
highlighting tokens and diagnostics. It never fails on malformed input;
problems are returned as diagnostics.

```js
import init, { editor_json, parse_json } from "@adoctra/wasm";

await init();
const view = JSON.parse(editor_json("== Title\n\nSome *bold* text."));
// view.tokens, view.blocks, view.outline, view.anchors, view.diagnostics
// (offsets are UTF-16 code units of the source string)
const tree = JSON.parse(parse_json("== Title")); // the whole tree, byte spans
```

- `editor_json(text)` is the compact answer an editor reparses on every
  edit; `parse_json(text)` is the full document tree.
- `ResourceWorkspace` is a virtual file system for `include::` targets and
  images; `DiagramWorkspace` renders diagrams through a Kroki server the
  host fetches from.

This module has no converter. For a live HTML preview load
[`@adoctra/wasm-preview`](https://www.npmjs.com/package/@adoctra/wasm-preview)
instead, which exports everything here plus `preview_json`.

The contract is described in
[docs/editor.adoc](https://github.com/avskor/adoctra/blob/master/docs/editor.adoc).
The API may change between `0.0.x` releases.

AsciiDoc is a registered trademark of the Eclipse Foundation, Inc. This is
an independent processor, not affiliated with or endorsed by the Eclipse
Foundation or the Asciidoctor project.

Licensed under MIT or Apache-2.0, at your option. Third-party notices are in
`LICENSE-third-party`.
