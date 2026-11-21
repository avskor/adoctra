// The block gutter: the type of each block on its first line, a bar along
// the block under the cursor.
import type { Extension } from "@codemirror/state";
import { EditorView, gutter, GutterMarker } from "@codemirror/view";
import { blockAt, blockClass, blockLabel, blockStart, blockStartingIn } from "../blocks.js";
import { parsed, parsedField } from "../state.js";
import type { Block } from "../types.js";

class BlockMarker extends GutterMarker {
  constructor(
    readonly label: string,
    readonly kind: string,
    readonly active: boolean,
    readonly title: string,
  ) {
    super();
    this.elementClass = active ? "adoc-gutter-active" : "";
  }

  override eq(other: BlockMarker): boolean {
    return (
      this.label === other.label && this.kind === other.kind && this.active === other.active
    );
  }

  override toDOM(): Node {
    const element = document.createElement("span");
    element.className = `adoc-block-label adoc-block-${this.kind}`;
    element.textContent = this.label;
    if (this.title) element.title = this.title;
    return element;
  }
}

export interface BlockGutterOptions {
  /** The label of a block (default: [`blockLabel`]). */
  label?: (block: Block) => string;
  /** Called when a label is clicked. */
  onClick?: (block: Block, view: EditorView) => void;
}

export function blockGutter(options: BlockGutterOptions = {}): Extension {
  const label = options.label ?? blockLabel;
  return [
    parsedField,
    gutter({
      class: "adoc-block-gutter",
      lineMarker(view, line) {
        const { blocks } = parsed(view.state);
        const starting = blockStartingIn(blocks, line.from, line.to);
        const current = blockAt(blocks, view.state.selection.main.head);
        const active =
          current !== null && line.to >= blockStart(current) && line.from <= current.end;
        if (!starting && !active) return null;
        return starting
          ? new BlockMarker(label(starting), blockClass(starting), active, starting.kind)
          : new BlockMarker("", "none", active, "");
      },
      lineMarkerChange: (update) =>
        update.selectionSet ||
        update.docChanged ||
        update.startState.field(parsedField) !== update.state.field(parsedField),
      initialSpacer: () => new BlockMarker("listing", "none", false, ""),
      domEventHandlers: {
        mousedown(view, line) {
          const block = blockStartingIn(parsed(view.state).blocks, line.from, line.to);
          if (!block) return false;
          if (options.onClick) options.onClick(block, view);
          else {
            // Select the block, metadata lines included.
            view.dispatch({ selection: { anchor: blockStart(block), head: block.end } });
            view.focus();
          }
          return true;
        },
      },
    }),
  ];
}
