// The editor in Vue 3 (render functions, so the playground needs no plugin).
import { AdoctraEditor, AdoctraOutline, AdoctraPreview } from "@adoctra/editor-vue";
import { createApp, h, ref, shallowRef } from "vue";
import { engine as startEngine, sample } from "./shared.js";

createApp({
  setup() {
    const engine = shallowRef(null);
    const editor = shallowRef(null);
    const text = ref(sample);
    startEngine().then((value) => (engine.value = value));
    return () =>
      h("div", { class: "layout" }, [
        h(AdoctraOutline, { editor: editor.value }),
        h("div", { class: "pane-editor" }, [
          h(AdoctraEditor, {
            engine: engine.value,
            modelValue: text.value,
            "onUpdate:modelValue": (value) => (text.value = value),
            onReady: (value) => (editor.value = value),
          }),
        ]),
        h("div", { class: "pane-preview" }, [h(AdoctraPreview, { editor: editor.value })]),
      ]);
  },
}).mount("#app");
