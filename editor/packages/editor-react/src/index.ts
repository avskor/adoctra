// React bindings: a thin layer over `@adoctra/editor-core`. Written with
// `createElement` so the package builds with `tsc` alone.
import {
  createAdoctraEditor,
  mountPreview,
  normalizeAssist,
  type AdoctraEditor as Editor,
  type AdoctraEditorOptions,
  type AdoctraSnapshot,
  type Engine,
  type OutlineEntry,
  type PreviewOptions,
} from "@adoctra/editor-core";
import {
  createElement,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
  type ReactElement,
  type RefObject,
} from "react";

export type { Editor as AdoctraEditorHandle, AdoctraSnapshot, Engine };

type Options = Omit<AdoctraEditorOptions, "parent" | "engine">;

/**
 * Mount an editor in the returned `ref` once `engine` is there. `value` and
 * `assist` are followed (a controlled component passes what `onChange` gave
 * it); the other options are read at mount.
 */
export function useAdoctraEditor(
  engine: Engine | null | undefined,
  options: Options = {},
): { ref: RefObject<HTMLDivElement | null>; editor: Editor | null } {
  const ref = useRef<HTMLDivElement | null>(null);
  const [editor, setEditor] = useState<Editor | null>(null);
  const latest = useRef(options);
  latest.current = options;
  useEffect(() => {
    if (!engine || !ref.current) return;
    const created = createAdoctraEditor({
      ...latest.current,
      parent: ref.current,
      engine,
      onChange: (value) => latest.current.onChange?.(value),
    });
    setEditor(created);
    return () => {
      created.destroy();
      setEditor(null);
    };
  }, [engine]);
  useEffect(() => {
    if (editor && options.value !== undefined) editor.setValue(options.value);
  }, [editor, options.value]);
  // The authoring aids follow the `assist` option (its switches and
  // language; the command set is read at mount).
  const assist = JSON.stringify(options.assist, (_key, value) =>
    typeof value === "function" ? "function" : value,
  );
  useEffect(() => {
    editor?.setAssist(normalizeAssist(latest.current.assist));
  }, [editor, assist]);
  return { ref, editor };
}

const NONE: AdoctraSnapshot = {
  revision: 0,
  parsedRevision: -1,
  view: null,
  outline: [],
  diagnostics: [],
  path: [],
  html: null,
  active: [],
};
const noSubscription = () => () => {};

/** The editor's snapshot (outline, diagnostics, block path, preview HTML). */
export function useAdoctraSnapshot(editor: Editor | null): AdoctraSnapshot {
  return useSyncExternalStore(
    editor ? editor.subscribe : noSubscription,
    editor ? editor.getSnapshot : () => NONE,
    () => NONE,
  );
}

export interface AdoctraEditorProps extends Options {
  engine: Engine | null | undefined;
  className?: string;
  style?: CSSProperties;
  /** Receives the editor once mounted (and `null` when unmounted). */
  onReady?: (editor: Editor | null) => void;
}

export function AdoctraEditor(props: AdoctraEditorProps): ReactElement {
  const { engine, className, style, onReady, ...options } = props;
  const { ref, editor } = useAdoctraEditor(engine, options);
  const ready = useRef(onReady);
  ready.current = onReady;
  useEffect(() => ready.current?.(editor), [editor]);
  return createElement("div", { ref, className, style });
}

export interface AdoctraPreviewProps extends PreviewOptions {
  editor: Editor | null;
  className?: string;
  style?: CSSProperties;
}

export function AdoctraPreview(props: AdoctraPreviewProps): ReactElement {
  const { editor, className, style, css, syncScroll } = props;
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!editor || !ref.current) return;
    const preview = mountPreview(ref.current, editor, { css, syncScroll });
    return () => preview.destroy();
  }, [editor, css, syncScroll]);
  return createElement("div", { ref, className, style });
}

export interface AdoctraOutlineProps {
  editor: Editor | null;
  className?: string;
  /** Replace the default entry (a button that reveals the section). */
  renderEntry?: (entry: OutlineEntry, reveal: () => void) => ReactElement;
}

export function AdoctraOutline(props: AdoctraOutlineProps): ReactElement {
  const { editor, className, renderEntry } = props;
  const { outline } = useAdoctraSnapshot(editor);
  return createElement(
    "nav",
    { className: className ?? "adoc-outline" },
    outline.map((entry) => {
      const reveal = () => editor?.revealOffset(entry.start);
      return createElement(
        "div",
        { key: `${entry.start}:${entry.title}`, "data-level": entry.level },
        renderEntry
          ? renderEntry(entry, reveal)
          : createElement(
              "button",
              { type: "button", onClick: reveal, style: { paddingLeft: `${entry.level}em` } },
              entry.title,
            ),
      );
    }),
  );
}
