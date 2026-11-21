// The toolbar: a panel above the text with the commands as buttons.
import type { Extension } from "@codemirror/state";
import { showPanel, type EditorView, type Panel } from "@codemirror/view";
import { parsedField } from "../state.js";
import { defaultToolbar, type ToolbarItem } from "./catalog.js";
import { activeCommands, type AssistCommand } from "./commands.js";
import { insertBlock, tableTemplate } from "./edit.js";
import type { Messages } from "./messages/types.js";
import { closeMenu, menuList, openPopup } from "./popup.js";

const GRID = 6;

function tablePicker(view: EditorView, messages: Messages): HTMLElement {
  const picker = document.createElement("div");
  picker.className = "adoc-table-picker";
  const caption = document.createElement("div");
  caption.className = "adoc-table-size";
  caption.textContent = messages.menus.table ?? "";
  const grid = document.createElement("div");
  grid.className = "adoc-table-grid";
  grid.style.gridTemplateColumns = `repeat(${GRID}, 1fr)`;
  const cells: HTMLElement[] = [];
  const paint = (rows: number, columns: number) => {
    cells.forEach((cell, index) =>
      cell.classList.toggle("adoc-on", Math.floor(index / GRID) < rows && index % GRID < columns),
    );
    caption.textContent = rows ? messages.tableSize(rows, columns) : (messages.menus.table ?? "");
  };
  for (let index = 0; index < GRID * GRID; index++) {
    const rows = Math.floor(index / GRID) + 1;
    const columns = (index % GRID) + 1;
    const cell = document.createElement("button");
    cell.type = "button";
    cell.className = "adoc-table-cell";
    cell.title = messages.tableSize(rows, columns);
    cell.dataset.rows = String(rows);
    cell.dataset.columns = String(columns);
    cell.addEventListener("mouseenter", () => paint(rows, columns));
    cell.addEventListener("mousedown", (event) => event.preventDefault());
    cell.addEventListener("click", () => {
      closeMenu();
      insertBlock(tableTemplate(rows, columns))(view);
      view.focus();
    });
    cells.push(cell);
    grid.append(cell);
  }
  grid.addEventListener("mouseleave", () => paint(0, 0));
  picker.append(grid, caption);
  return picker;
}

export function toolbar(
  commands: readonly AssistCommand[],
  messages: Messages,
  items: readonly ToolbarItem[] = defaultToolbar,
): Extension {
  const byId = new Map(commands.map((command) => [command.id, command]));
  const panel = (view: EditorView): Panel => {
    const dom = document.createElement("div");
    dom.className = "adoc-toolbar";
    dom.setAttribute("role", "toolbar");
    const buttons = new Map<string, HTMLButtonElement>();
    const menus: { button: HTMLButtonElement; ids: string[] }[] = [];

    const button = (icon: string, title: string, id: string) => {
      const element = document.createElement("button");
      element.type = "button";
      element.className = "adoc-tool";
      element.dataset.command = id;
      element.title = title;
      element.setAttribute("aria-label", title);
      element.textContent = icon;
      element.addEventListener("mousedown", (event) => event.preventDefault());
      dom.append(element);
      return element;
    };
    const below = (element: HTMLElement) => {
      const box = element.getBoundingClientRect();
      return [box.left, box.bottom + 2] as const;
    };

    for (const item of items) {
      if (item === "|") {
        const separator = document.createElement("span");
        separator.className = "adoc-tool-separator";
        dom.append(separator);
      } else if (typeof item === "string") {
        const command = byId.get(item);
        if (!command) continue;
        const element = button(command.icon, messages.commands[item] ?? item, item);
        element.addEventListener("click", () => {
          command.run(view, undefined, view);
          view.focus();
        });
        buttons.set(item, element);
      } else if ("table" in item) {
        const element = button("▦", messages.menus.table ?? "table", "table");
        element.addEventListener("click", () =>
          openPopup(view, tablePicker(view, messages), ...below(element), "table"),
        );
        buttons.set("table", element);
      } else {
        const ids = item.items.filter((id) => byId.has(id));
        if (ids.length === 0) continue;
        const element = button(`${item.icon} ▾`, messages.menus[item.menu] ?? item.menu, `menu:${item.menu}`);
        element.classList.add("adoc-tool-menu");
        element.addEventListener("click", () => {
          const active = new Set(activeCommands(commands, view.state));
          const entries = ids.map((id) => {
            const command = byId.get(id)!;
            return {
              label: messages.commands[id] ?? id,
              icon: command.icon,
              active: active.has(id),
              run() {
                command.run(view, undefined, view);
                view.focus();
              },
            };
          });
          openPopup(view, menuList(entries), ...below(element), item.menu);
        });
        menus.push({ button: element, ids });
      }
    }

    let shown = "";
    const refresh = () => {
      const active = activeCommands(commands, view.state);
      const key = active.join(",");
      if (key === shown) return;
      shown = key;
      const set = new Set(active);
      for (const [id, element] of buttons) element.setAttribute("aria-pressed", String(set.has(id)));
      for (const menu of menus) {
        menu.button.setAttribute("aria-pressed", String(menu.ids.some((id) => set.has(id))));
      }
    };
    refresh();
    return { dom, top: true, update: refresh, destroy: closeMenu };
  };
  return [parsedField, showPanel.of(panel)];
}
