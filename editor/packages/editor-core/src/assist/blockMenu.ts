// The block menu: what can be done with the block whose gutter label was
// clicked.
import type { EditorView } from "@codemirror/view";
import { blockStart } from "../blocks.js";
import type { Block } from "../types.js";
import type { AssistCommand } from "./commands.js";
import type { Messages } from "./messages/types.js";
import { menuList, openPopup } from "./popup.js";

/** The block actions offered for `block`. */
export function blockActions(commands: readonly AssistCommand[], block: Block): AssistCommand[] {
  return commands.filter(
    (command) => command.group === "blockAction" && (command.appliesTo?.(block) ?? true),
  );
}

/** A gutter click handler ([`BlockGutterOptions.onClick`]) opening the menu. */
export function blockMenu(commands: readonly AssistCommand[], messages: Messages) {
  return (block: Block, view: EditorView): void => {
    const at = view.coordsAtPos(Math.min(blockStart(block), view.state.doc.length));
    const gutter = view.dom.querySelector(".adoc-block-gutter")?.getBoundingClientRect();
    const entries = blockActions(commands, block).map((command) => ({
      label: messages.commands[command.id] ?? command.id,
      icon: command.icon,
      run() {
        command.run(view, block, view);
        view.focus();
      },
    }));
    openPopup(view, menuList(entries), gutter?.left ?? 0, (at?.bottom ?? 0) + 2, "block");
  };
}
