# @adoctra/editor-core

A source editor for AsciiDoc in the browser, on CodeMirror 6 and the
[adoctra](https://github.com/avskor/adoctra) parser (WebAssembly). No
framework; see `@adoctra/editor-vue` and `@adoctra/editor-react` for
bindings.

Everything it shows comes from one parse of the real parser, not from a
JavaScript grammar: highlighting, a gutter naming the type of each block,
breadcrumbs, diagnostics, folding, an outline, completion and a live preview
that scrolls with the source. Authoring aids write the markup for you: a
slash menu, a toolbar, a block menu, hover help and list levels on Tab — each
can be switched off.

```sh
npm install @adoctra/editor-core @adoctra/wasm-preview
```

A worker file of your application (the parser runs off the main thread):

```js
// adoctra.worker.js
import init, * as wasm from "@adoctra/wasm-preview"; // or "@adoctra/wasm"
import { serve } from "@adoctra/editor-core/worker";
serve(init, wasm);
```

The page:

```js
import { createAdoctraEditor, createWorkerEngine, mountPreview } from "@adoctra/editor-core";

const engine = await createWorkerEngine(
  new Worker(new URL("./adoctra.worker.js", import.meta.url), { type: "module" }),
);
const editor = createAdoctraEditor({
  parent: document.querySelector("#editor"),
  engine,
  value: "= Title\n\nHello, *AsciiDoc*.",
  assist: { locale: "en" }, // "ru" is built in; false switches the aids off
});
mountPreview(document.querySelector("#preview"), editor);

editor.subscribe(() => {
  const { outline, diagnostics, path, html } = editor.getSnapshot();
});
```

With `@adoctra/wasm` instead of `@adoctra/wasm-preview` the module is
smaller and there is no preview.

The full description is in
[docs/editor.adoc](https://github.com/avskor/adoctra/blob/master/docs/editor.adoc).
The API may change between `0.0.x` releases.

Licensed under MIT or Apache-2.0, at your option.
