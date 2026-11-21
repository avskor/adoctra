# @adoctra/editor-react

React bindings of the [adoctra](https://github.com/avskor/adoctra) AsciiDoc
editor ([`@adoctra/editor-core`](https://www.npmjs.com/package/@adoctra/editor-core)).

```sh
npm install @adoctra/editor-react @adoctra/wasm-preview
```

```jsx
import { createWorkerEngine } from "@adoctra/editor-core";
import { AdoctraEditor, AdoctraOutline, AdoctraPreview } from "@adoctra/editor-react";
import { useEffect, useState } from "react";

export function Editor() {
  const [engine, setEngine] = useState(null);
  const [editor, setEditor] = useState(null);
  const [text, setText] = useState("= Title\n\nHello, *AsciiDoc*.");
  useEffect(() => {
    // adoctra.worker.js: see the @adoctra/editor-core README
    createWorkerEngine(
      new Worker(new URL("./adoctra.worker.js", import.meta.url), { type: "module" }),
    ).then(setEngine);
  }, []);
  return (
    <>
      <AdoctraOutline editor={editor} />
      <AdoctraEditor engine={engine} value={text} onChange={setText} onReady={setEditor}
                     assist={{ toolbar: true, locale: "en" }} />
      <AdoctraPreview editor={editor} />
    </>
  );
}
```

`useAdoctraEditor(engine, options)` mounts the editor in your own markup and
`useAdoctraSnapshot(editor)` gives its outline, diagnostics, block path and
preview HTML.

See [docs/editor.adoc](https://github.com/avskor/adoctra/blob/master/docs/editor.adoc).
The API may change between `0.0.x` releases.

Licensed under MIT or Apache-2.0, at your option.
