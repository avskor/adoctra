// The chain of blocks around the cursor, in a panel under the editor.
import type { Extension } from "@codemirror/state";
import { EditorView, showPanel, type Panel } from "@codemirror/view";
import { blockLabel, blockPath, blockStart } from "../blocks.js";
import { parsed, parsedField } from "../state.js";
import type { Block } from "../types.js";

function describe(block: Block): string {
  const label = blockLabel(block);
  switch (block.kind) {
    case "section":
      return `section ${label}`;
    case "paragraph":
      return "paragraph";
    case "list-item":
      return "item";
    case "listing":
      return label === "listing" ? label : `listing (${label})`;
    default:
      return label;
  }
}

export function breadcrumbs(label: (block: Block) => string = describe): Extension {
  const panel = (view: EditorView): Panel => {
    const dom = document.createElement("div");
    dom.className = "adoc-breadcrumbs";
    let shown = "";
    const render = () => {
      const path = blockPath(parsed(view.state).blocks, view.state.selection.main.head);
      const key = path.map((block) => `${blockStart(block)}:${label(block)}`).join("|");
      if (key === shown && dom.childNodes.length > 0) return;
      shown = key;
      dom.textContent = "";
      if (path.length === 0) dom.append(" ");
      path.forEach((block, index) => {
        if (index > 0) dom.append(" › ");
        const crumb = document.createElement("button");
        crumb.type = "button";
        crumb.className = "adoc-crumb";
        crumb.textContent = label(block);
        crumb.onclick = () => {
          view.dispatch({ selection: { anchor: blockStart(block), head: block.end } });
          view.focus();
        };
        dom.append(crumb);
      });
    };
    render();
    return { dom, update: render };
  };
  return [parsedField, showPanel.of(panel)];
}
