// The editor in React (no JSX, so the playground needs no plugin).
import { AdoctraEditor, AdoctraOutline, AdoctraPreview } from "@adoctra/editor-react";
import { createElement as h, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { engine as startEngine, sample } from "./shared.js";

function App() {
  const [engine, setEngine] = useState(null);
  const [editor, setEditor] = useState(null);
  const [text, setText] = useState(sample);
  useEffect(() => {
    startEngine().then(setEngine);
  }, []);
  return h(
    "div",
    { className: "layout" },
    h(AdoctraOutline, { editor }),
    h("div", { className: "pane-editor" }, h(AdoctraEditor, { engine, value: text, onChange: setText, onReady: setEditor })),
    h("div", { className: "pane-preview" }, h(AdoctraPreview, { editor })),
  );
}

createRoot(document.querySelector("#app")).render(h(App));
