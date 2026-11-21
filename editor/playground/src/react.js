// The editor in React (no JSX, so the playground needs no plugin).
import { AdoctraEditor, AdoctraOutline, AdoctraPreview } from "@adoctra/editor-react";
import { createElement as h, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { createRoot } from "react-dom/client";
import { engine as startEngine, initialAssist, sample, switches } from "./shared.js";

function App() {
  const [engine, setEngine] = useState(null);
  const [editor, setEditor] = useState(null);
  const [text, setText] = useState(sample);
  const [assist, setAssist] = useState(initialAssist);
  useEffect(() => {
    startEngine().then(setEngine);
  }, []);
  return h(
    "div",
    { className: "layout" },
    createPortal(
      switches.map(([key, title]) =>
        h(
          "label",
          { key },
          h("input", {
            type: "checkbox",
            id: `toggle-${key}`,
            checked: assist[key],
            onChange: (event) => setAssist({ ...assist, [key]: event.target.checked }),
          }),
          title,
        ),
      ),
      document.querySelector("#controls"),
    ),
    h(AdoctraOutline, { editor }),
    h(
      "div",
      { className: "pane-editor" },
      h(AdoctraEditor, { engine, value: text, onChange: setText, onReady: setEditor, assist }),
    ),
    h("div", { className: "pane-preview" }, h(AdoctraPreview, { editor })),
  );
}

createRoot(document.querySelector("#app")).render(h(App));
