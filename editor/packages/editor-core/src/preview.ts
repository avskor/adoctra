// The live preview: the converted document in a sandboxed frame, scrolled
// with the editor and clickable back to the source.
import type { AdoctraEditor } from "./editor.js";

export interface PreviewOptions {
  /** Extra CSS after the engine's stylesheet. */
  css?: string;
  /** Follow the editor's scroll position (default true). */
  syncScroll?: boolean;
}

export interface AdoctraPreview {
  readonly frame: HTMLIFrameElement;
  destroy(): void;
}

const FRAME_CSS = `
  body { margin: 0; padding: 16px 24px; }
  [data-source-start] { scroll-margin-top: 8px; }
  .adoc-preview-active { outline: 2px solid rgba(37, 99, 235, 0.35); outline-offset: 4px; }
`;

/**
 * Show the editor's `html` in `container`. The document's markup cannot run
 * scripts: the frame is sandboxed without `allow-scripts` (a passthrough
 * block is the author's raw HTML).
 */
export function mountPreview(
  container: Element,
  editor: AdoctraEditor,
  options: PreviewOptions = {},
): AdoctraPreview {
  const frame = document.createElement("iframe");
  frame.className = "adoc-preview";
  frame.setAttribute("sandbox", "allow-same-origin");
  frame.style.cssText = "width:100%;height:100%;border:0;display:block";
  const style = `${editor.engine.stylesheet ?? ""}\n${FRAME_CSS}\n${options.css ?? ""}`;
  frame.srcdoc = `<!doctype html><html><head><meta charset="utf-8"><style>${style.replace(
    /<\/style/gi,
    "<\\/style",
  )}</style></head><body class="article"></body></html>`;
  container.append(frame);

  let shown: string | null = null;
  let marks: { start: number; element: HTMLElement }[] = [];
  let active: HTMLElement | null = null;
  const body = () => frame.contentDocument?.body ?? null;

  const render = () => {
    const target = body();
    const { html, path } = editor.getSnapshot();
    if (!target || frame.contentDocument?.readyState === "loading") return;
    if (html !== null && html !== shown) {
      shown = html;
      const top = frame.contentWindow?.scrollY ?? 0;
      target.innerHTML = html;
      marks = [...target.querySelectorAll<HTMLElement>("[data-source-start]")]
        .map((element) => ({ start: Number(element.dataset.sourceStart), element }))
        .sort((a, b) => a.start - b.start);
      frame.contentWindow?.scrollTo(0, top);
      active = null;
    }
    // Outline the element of the innermost marked block around the cursor.
    let next: HTMLElement | null = null;
    for (let index = path.length - 1; index >= 0 && !next; index--) {
      const start = path[index]!.start;
      next = marks.find((mark) => mark.start === start)?.element ?? null;
    }
    if (next !== active) {
      active?.classList.remove("adoc-preview-active");
      next?.classList.add("adoc-preview-active");
      active = next;
    }
  };

  const follow = () => {
    const view = editor.view;
    const window = frame.contentWindow;
    if (!window || marks.length === 0) return;
    const top = view.lineBlockAtHeight(view.scrollDOM.scrollTop);
    // The last marked element starting at or before the editor's top line.
    let low = 0;
    let high = marks.length - 1;
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      if (marks[middle]!.start <= top.to) low = middle;
      else high = middle - 1;
    }
    const mark = marks[low]!;
    if (mark.start > top.to) return window.scrollTo(0, 0);
    const next = marks[low + 1];
    const y = mark.element.getBoundingClientRect().top + window.scrollY;
    if (!next) return window.scrollTo(0, y - 8);
    // Between two marks, move in proportion to the source in between.
    const nextY = next.element.getBoundingClientRect().top + window.scrollY;
    const fraction = Math.min(1, Math.max(0, (top.from - mark.start) / (next.start - mark.start)));
    window.scrollTo(0, y + (nextY - y) * fraction - 8);
  };

  const click = (event: MouseEvent) => {
    const element = (event.target as Element | null)?.closest<HTMLElement>("[data-source-start]");
    if (element) editor.revealOffset(Number(element.dataset.sourceStart));
    // Links would navigate the frame away from the preview.
    if ((event.target as Element | null)?.closest("a")) event.preventDefault();
  };

  const loaded = () => {
    frame.contentDocument?.addEventListener("click", click);
    render();
  };
  frame.addEventListener("load", loaded);
  const unsubscribe = editor.subscribe(render);
  const sync = options.syncScroll !== false;
  if (sync) editor.view.scrollDOM.addEventListener("scroll", follow, { passive: true });

  return {
    frame,
    destroy() {
      unsubscribe();
      frame.removeEventListener("load", loaded);
      if (sync) editor.view.scrollDOM.removeEventListener("scroll", follow);
      frame.remove();
    },
  };
}
