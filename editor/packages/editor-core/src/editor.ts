// The editor controller: a CodeMirror view parsing through an engine, with
// a snapshot store the framework bindings subscribe to.
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { setDiagnostics, type Diagnostic as CmDiagnostic } from "@codemirror/lint";
import { EditorState, type Extension } from "@codemirror/state";
import { drawSelection, EditorView, highlightActiveLine, keymap } from "@codemirror/view";
import { blockPath } from "./blocks.js";
import { LatestParser } from "./engine.js";
import { breadcrumbs } from "./extensions/breadcrumbs.js";
import { formattingKeymap } from "./extensions/commands.js";
import { completion } from "./extensions/completion.js";
import { folding } from "./extensions/folding.js";
import { blockGutter, type BlockGutterOptions } from "./extensions/gutter.js";
import { parsed, parsedField, setParsed } from "./state.js";
import { theme } from "./theme.js";
import type { Block, Diagnostic, Engine, EditorViewData, Files, OutlineEntry } from "./types.js";

export interface AdoctraEditorOptions {
  /** The element the editor is mounted in. */
  parent: Element;
  engine: Engine;
  value?: string;
  /** The edited file's workspace path (default `main.adoc`). */
  name?: string;
  /** Include targets, by workspace path. */
  files?: Files;
  readOnly?: boolean;
  /** `false` hides the block gutter; an object configures it. */
  gutter?: boolean | BlockGutterOptions;
  breadcrumbs?: boolean;
  /** Milliseconds of quiet before a parse (default 60). */
  delay?: number;
  /** More CodeMirror extensions (themes, keymaps, …). */
  extensions?: Extension;
  onChange?: (value: string) => void;
}

/** What the editor knows after the last parse; a new object per change. */
export interface AdoctraSnapshot {
  /** Counts document changes; `parsedRevision` is the one last parsed. */
  revision: number;
  parsedRevision: number;
  view: EditorViewData | null;
  outline: readonly OutlineEntry[];
  diagnostics: readonly Diagnostic[];
  /** The blocks around the cursor, outermost first. */
  path: readonly Block[];
  /** The preview, when the engine converts. */
  html: string | null;
}

export interface AdoctraEditor {
  readonly view: EditorView;
  readonly engine: Engine;
  getValue(): string;
  /** Replace the document (no-op when equal). */
  setValue(value: string): void;
  /** Set or remove (`null`) an include target and parse again. */
  setFile(name: string, text: string | null): void;
  focus(): void;
  /** Move the cursor to `offset` and scroll it into view. */
  revealOffset(offset: number): void;
  getSnapshot(): AdoctraSnapshot;
  subscribe(listener: () => void): () => void;
  destroy(): void;
}

const EMPTY: AdoctraSnapshot = {
  revision: 0,
  parsedRevision: -1,
  view: null,
  outline: [],
  diagnostics: [],
  path: [],
  html: null,
};

export function createAdoctraEditor(options: AdoctraEditorOptions): AdoctraEditor {
  const { engine } = options;
  const listeners = new Set<() => void>();
  let snapshot = EMPTY;
  let files: Files = { ...options.files };
  let timer: ReturnType<typeof setTimeout> | undefined;
  let destroyed = false;
  // The revision each request was made at, to drop outcomes of older text.
  const revisions = new WeakMap<object, number>();

  const publish = (next: Partial<AdoctraSnapshot>) => {
    snapshot = { ...snapshot, ...next };
    for (const listener of [...listeners]) listener();
  };

  const parser = new LatestParser(
    engine,
    (outcome, request) => {
      const revision = revisions.get(request);
      if (destroyed || revision !== snapshot.revision) return;
      const length = view.state.doc.length;
      // A diagnostic of an included file has no range in this text; it
      // stays in the snapshot only.
      const diagnostics: CmDiagnostic[] = outcome.view.diagnostics
        .filter((diagnostic) => diagnostic.source_id === outcome.view.source_id)
        .map((diagnostic) => {
          const from = Math.min(diagnostic.start, length);
          return {
            from,
            to: Math.min(Math.max(diagnostic.end, from), length),
            severity: diagnostic.severity,
            message: diagnostic.message,
            source: "adoctra",
          };
        });
      view.dispatch({
        effects: [
          setParsed.of(outcome.view),
          ...asArray(setDiagnostics(view.state, diagnostics).effects),
        ],
      });
      publish({
        parsedRevision: revision,
        view: outcome.view,
        outline: outcome.view.outline,
        diagnostics: outcome.view.diagnostics,
        html: outcome.html,
        path: blockPath(outcome.view.blocks, view.state.selection.main.head),
      });
    },
    (error) => console.error("adoctra: parse failed", error),
  );

  const parseNow = () => {
    if (destroyed) return;
    const request = { text: view.state.doc.toString(), name: options.name, files };
    revisions.set(request, snapshot.revision);
    parser.request(request);
  };
  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(parseNow, options.delay ?? 60);
  };

  const gutterOptions = typeof options.gutter === "object" ? options.gutter : {};
  const view = new EditorView({
    parent: options.parent,
    state: EditorState.create({
      doc: options.value ?? "",
      extensions: [
        parsedField,
        history(),
        drawSelection(),
        highlightActiveLine(),
        EditorView.lineWrapping,
        options.gutter === false ? [] : blockGutter(gutterOptions),
        folding(),
        options.breadcrumbs === false ? [] : breadcrumbs(),
        completion(),
        formattingKeymap(),
        keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
        EditorState.readOnly.of(options.readOnly ?? false),
        theme,
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            publish({ revision: snapshot.revision + 1 });
            options.onChange?.(update.state.doc.toString());
            schedule();
          } else if (update.selectionSet) {
            publish({
              path: blockPath(parsed(update.state).blocks, update.state.selection.main.head),
            });
          }
        }),
        options.extensions ?? [],
      ],
    }),
  });
  parseNow();

  return {
    view,
    engine,
    getValue: () => view.state.doc.toString(),
    setValue(value) {
      if (value === view.state.doc.toString()) return;
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
    },
    setFile(name, text) {
      files = { ...files };
      if (text === null) delete files[name];
      else files[name] = text;
      // The same text parses differently now.
      publish({ revision: snapshot.revision + 1 });
      schedule();
    },
    focus: () => view.focus(),
    revealOffset(offset) {
      const at = Math.max(0, Math.min(offset, view.state.doc.length));
      view.dispatch({
        selection: { anchor: at },
        effects: EditorView.scrollIntoView(at, { y: "start", yMargin: 24 }),
      });
      view.focus();
    },
    getSnapshot: () => snapshot,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    destroy() {
      destroyed = true;
      clearTimeout(timer);
      parser.dispose();
      listeners.clear();
      view.destroy();
    },
  };
}

function asArray<T>(value: T | readonly T[] | undefined): readonly T[] {
  if (value === undefined) return [];
  return Array.isArray(value) ? (value as readonly T[]) : [value as T];
}
