// The editor in Vue 3 (render functions, so the playground needs no plugin).
import { AdoctraEditor, AdoctraOutline, AdoctraPreview } from "@adoctra/editor-vue";
import { createApp, h, reactive, ref, shallowRef, Teleport } from "vue";
import { engine as startEngine, initialAssist, sample, switches } from "./shared.js";

createApp({
  setup() {
    const engine = shallowRef(null);
    const editor = shallowRef(null);
    const text = ref(sample);
    const assist = reactive({ ...initialAssist });
    startEngine().then((value) => (engine.value = value));
    return () =>
      h("div", { class: "layout" }, [
        h(
          Teleport,
          { to: "#controls" },
          switches.map(([key, title]) =>
            h("label", { key }, [
              h("input", {
                type: "checkbox",
                id: `toggle-${key}`,
                checked: assist[key],
                onChange: (event) => (assist[key] = event.target.checked),
              }),
              title,
            ]),
          ),
        ),
        h(AdoctraOutline, { editor: editor.value }),
        h("div", { class: "pane-editor" }, [
          h(AdoctraEditor, {
            engine: engine.value,
            modelValue: text.value,
            assist,
            "onUpdate:modelValue": (value) => (text.value = value),
            onReady: (value) => (editor.value = value),
          }),
        ]),
        h("div", { class: "pane-preview" }, [h(AdoctraPreview, { editor: editor.value })]),
      ]);
  },
}).mount("#app");
