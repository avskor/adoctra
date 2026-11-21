# @adoctra/editor-vue

Vue 3 bindings of the [adoctra](https://github.com/avskor/adoctra) AsciiDoc
editor ([`@adoctra/editor-core`](https://www.npmjs.com/package/@adoctra/editor-core)).

```sh
npm install @adoctra/editor-vue @adoctra/wasm-preview
```

```vue
<script setup>
import { createWorkerEngine } from "@adoctra/editor-core";
import { AdoctraEditor, AdoctraOutline, AdoctraPreview } from "@adoctra/editor-vue";
import { ref, shallowRef } from "vue";

const engine = shallowRef(null);
const editor = shallowRef(null);
const text = ref("= Title\n\nHello, *AsciiDoc*.");
// adoctra.worker.js: see the @adoctra/editor-core README
createWorkerEngine(
  new Worker(new URL("./adoctra.worker.js", import.meta.url), { type: "module" }),
).then((value) => (engine.value = value));
</script>

<template>
  <AdoctraOutline :editor="editor" />
  <AdoctraEditor v-model="text" :engine="engine" :assist="{ toolbar: true, locale: 'en' }"
                 @ready="editor = $event" />
  <AdoctraPreview :editor="editor" />
</template>
```

`useAdoctraEditor(engine, options)` mounts the editor in your own markup and
returns its `snapshot` (outline, diagnostics, block path, preview HTML).

See [docs/editor.adoc](https://github.com/avskor/adoctra/blob/master/docs/editor.adoc).
The API may change between `0.0.x` releases.

Licensed under MIT or Apache-2.0, at your option.
