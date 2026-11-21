// The editor without a framework.
import { createAdoctraEditor, mountPreview } from "@adoctra/editor-core";
import { engine, initialAssist, sample, switches } from "./shared.js";

const editor = createAdoctraEditor({
  parent: document.querySelector("#editor"),
  engine: await engine(),
  value: sample,
  assist: initialAssist,
});

// Each aid switches off and on without rebuilding the editor.
const controls = document.querySelector("#controls");
for (const [key, title] of switches) {
  const label = document.createElement("label");
  const box = document.createElement("input");
  box.type = "checkbox";
  box.checked = true;
  box.id = `toggle-${key}`;
  box.onchange = () => editor.setAssist({ [key]: box.checked });
  label.append(box, title);
  controls.append(label);
}
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
