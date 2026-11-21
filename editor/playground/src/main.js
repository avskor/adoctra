// The editor without a framework.
import { createAdoctraEditor, mountPreview } from "@adoctra/editor-core";
import { engine, sample } from "./shared.js";

const editor = createAdoctraEditor({
  parent: document.querySelector("#editor"),
  engine: await engine(),
  value: sample,
});
mountPreview(document.querySelector("#preview"), editor);

const outline = document.querySelector("#outline");
const problems = document.querySelector("#problems");
const render = () => {
  const snapshot = editor.getSnapshot();
  outline.replaceChildren(
    ...snapshot.outline.map((entry) => {
      const button = document.createElement("button");
      button.textContent = entry.title;
      button.style.paddingLeft = `${entry.level}em`;
      button.onclick = () => editor.revealOffset(entry.start);
      return button;
    }),
  );
  problems.textContent = snapshot.diagnostics.map((d) => `${d.severity}: ${d.message}`).join("\n");
};
editor.subscribe(render);
render();
window.adoctraEditor = editor;
