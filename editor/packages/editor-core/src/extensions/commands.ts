// The formatting keymap, on the assist commands' mark toggles.
import type { Extension } from "@codemirror/state";
import { keymap, type Command } from "@codemirror/view";
import { toggleMark } from "../assist/edit.js";
import { changeListLevel, continueList } from "../assist/lists.js";

export { toggleMark };

export const toggleStrong: Command = toggleMark("*");
export const toggleEmphasis: Command = toggleMark("_");
export const toggleMonospace: Command = toggleMark("`");

/**
 * List levels on the keys authors expect: Tab and Shift-Tab change the
 * level of the list items on the selected lines, Enter continues the list.
 * Outside a list the keys fall through.
 */
export function listKeymap(): Extension {
  return keymap.of([
    { key: "Tab", run: changeListLevel(1) },
    { key: "Shift-Tab", run: changeListLevel(-1) },
    { key: "Enter", run: continueList },
  ]);
}

export function formattingKeymap(): Extension {
  return keymap.of([
    { key: "Mod-b", run: toggleStrong },
    { key: "Mod-i", run: toggleEmphasis },
    { key: "Mod-`", run: toggleMonospace },
  ]);
}
