// A small menu attached to the editor: the toolbar's drop-downs and the
// block menu. Closes on a choice, an outside press, Escape or scrolling.
import type { EditorView } from "@codemirror/view";

export interface MenuEntry {
  label: string;
  icon?: string;
  active?: boolean;
  run(): void;
}

let close: (() => void) | null = null;

export function closeMenu(): void {
  close?.();
}

/** Show `content` in the editor at `left`/`top` (viewport coordinates). */
export function openPopup(view: EditorView, content: HTMLElement, left: number, top: number, name: string): void {
  closeMenu();
  const box = view.dom.getBoundingClientRect();
  const popup = document.createElement("div");
  popup.className = `adoc-menu adoc-menu-${name}`;
  popup.style.left = `${Math.max(0, left - box.left)}px`;
  popup.style.top = `${Math.max(0, top - box.top)}px`;
  popup.append(content);
  view.dom.append(popup);
  // Keep it inside the editor.
  const overflow = popup.getBoundingClientRect().right - box.right;
  if (overflow > 0) popup.style.left = `${Math.max(0, left - box.left - overflow - 4)}px`;

  const outside = (event: Event) => {
    if (!popup.contains(event.target as Node)) closeMenu();
  };
  const key = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      closeMenu();
      view.focus();
    }
  };
  const scroller = view.scrollDOM;
  // A scroll event of an earlier scroll may still be on its way: close
  // only when the text really moves under the menu.
  const openedAt = scroller.scrollTop;
  const scrolled = () => {
    if (Math.abs(scroller.scrollTop - openedAt) > 4) closeMenu();
  };
  document.addEventListener("mousedown", outside, true);
  document.addEventListener("keydown", key, true);
  scroller.addEventListener("scroll", scrolled);
  close = () => {
    close = null;
    document.removeEventListener("mousedown", outside, true);
    document.removeEventListener("keydown", key, true);
    scroller.removeEventListener("scroll", scrolled);
    popup.remove();
  };
}

export function menuList(entries: readonly MenuEntry[]): HTMLElement {
  const list = document.createElement("div");
  list.className = "adoc-menu-list";
  list.setAttribute("role", "menu");
  for (const entry of entries) {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "adoc-menu-item";
    item.setAttribute("role", "menuitem");
    if (entry.active) item.setAttribute("aria-checked", "true");
    const icon = document.createElement("span");
    icon.className = "adoc-menu-icon";
    icon.textContent = entry.icon ?? "";
    item.append(icon, entry.label);
    // Keep the editor's focus and selection.
    item.addEventListener("mousedown", (event) => event.preventDefault());
    item.addEventListener("click", () => {
      closeMenu();
      entry.run();
    });
    list.append(item);
  }
  return list;
}
