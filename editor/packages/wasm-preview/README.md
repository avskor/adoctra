# @adoctra/wasm-preview

The [adoctra](https://github.com/avskor/adoctra) AsciiDoc parser **and HTML
converter** for the browser, compiled to WebAssembly. It exports everything
[`@adoctra/wasm`](https://www.npmjs.com/package/@adoctra/wasm) does and adds
the conversion, so an editor gets its view and the preview from one parse.
Load one of the two modules, not both.

```js
import init, { preview_json, stylesheet } from "@adoctra/wasm-preview";

await init();
const { html, editor } = JSON.parse(preview_json("== Title\n\nSome *bold* text."));
// html: embedded HTML; each block's first element carries
//       data-source-start / data-source-end (UTF-16 offsets into the source)
// editor: the same view as editor_json() of @adoctra/wasm
const css = stylesheet(); // the stylesheet the markup is written for
```

`workspace_preview_json(workspace, sourceId)` does the same for a document
loaded into a `ResourceWorkspace` (with `include::` targets).

The contract is described in
[docs/editor.adoc](https://github.com/avskor/adoctra/blob/master/docs/editor.adoc).
The API may change between `0.0.x` releases.

AsciiDoc is a registered trademark of the Eclipse Foundation, Inc. This is
an independent processor, not affiliated with or endorsed by the Eclipse
Foundation or the Asciidoctor project.

Licensed under MIT or Apache-2.0, at your option. The module contains a port
of Rouge (`LICENSE-rouge`) and other third-party code
(`LICENSE-third-party`).
