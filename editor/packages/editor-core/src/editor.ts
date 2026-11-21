// The editor controller: a CodeMirror view parsing through an engine, with
// a snapshot store the framework bindings subscribe to.
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { setDiagnostics, type Diagnostic as CmDiagnostic } from "@codemirror/lint";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { drawSelection, EditorView, highlightActiveLine, keymap } from "@codemirror/view";
import {
  activeCommands,
  blockMenu,
  defaultCommands,
  defaultToolbar,
  hoverHelp,
  resolveMessages,
  slashSource,
  toolbar,
  valueSource,
  type AssistCommand,
  type Locale,
  type ToolbarItem,
} from "./assist/index.js";
import { actionBlock, blockPath } from "./blocks.js";
import { LatestParser } from "./engine.js";
import { breadcrumbs } from "./extensions/breadcrumbs.js";
import { formattingKeymap, listKeymap } from "./extensions/commands.js";
import { completion } from "./extensions/completion.js";
import { folding } from "./extensions/folding.js";
import { blockGutter, type BlockGutterOptions } from "./extensions/gutter.js";
import { parsed, parsedField, setParsed } from "./state.js";
import { theme } from "./theme.js";
import type { Block, Diagnostic, Engine, EditorViewData, Files, OutlineEntry } from "./types.js";

/** The authoring aids; each surface can be switched off. */
export interface AssistOptions {
  /** `/` on an empty line lists the elements to insert (default true). */
  slashMenu?: boolean;
  /** The toolbar above the text; an object chooses its items (default true). */
  toolbar?: boolean | { items: ToolbarItem[] };
  /** A click on a gutter label opens the block's actions (default true). */
  blockMenu?: boolean;
  /** Help on hovering markup (default true). */
  hover?: boolean;
  /** Completion of styles, languages, attributes and include targets (default true). */
  valueCompletion?: boolean;
  /**
   * Tab and Shift-Tab change the level of a list item, Enter continues the
   * list (default true).
   */
  listKeys?: boolean;
  /** The language of labels and help: `en` (default), `ru` or own messages. */
  locale?: Locale;
  /** Change the command set: receives the defaults, returns the set to use. */
  commands?: (defaults: AssistCommand[]) => AssistCommand[];
}

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
  /** `false` switches every authoring aid off; an object chooses. */
  assist?: false | AssistOptions;
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
  /** The ids of the commands whose element the cursor is in. */
  active: readonly string[];
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
  /** The commands, for a toolbar or menu of the host's own. */
  readonly commands: readonly AssistCommand[];
  /** Run a command by id; `false` when unknown or not applicable. */
  run(id: string): boolean;
  /** Switch authoring aids on or off; unnamed ones keep their setting. */
  setAssist(options: false | AssistOptions): void;
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
  active: [],
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

  // Each aid is a compartment of its own, so it switches without a rebuild.
  let assist: AssistOptions = options.assist === false ? ALL_OFF : { ...options.assist };
  const commands = (assist.commands ?? ((defaults) => defaults))(defaultCommands());
  const compartments = {
    gutter: new Compartment(),
    toolbar: new Compartment(),
    completion: new Compartment(),
    hover: new Compartment(),
    listKeys: new Compartment(),
  };
  const configured = () => {
    const messages = resolveMessages(assist.locale);
    const sources = [];
    if (assist.slashMenu !== false) sources.push(slashSource(commands, messages));
    if (assist.valueCompletion !== false) sources.push(valueSource(messages, () => Object.keys(files)));
    return {
      gutter:
        options.gutter === false
          ? []
          : blockGutter(
              assist.blockMenu !== false && !gutterOptions.onClick
                ? { ...gutterOptions, onClick: blockMenu(commands, messages) }
                : gutterOptions,
            ),
      toolbar:
        assist.toolbar === false
          ? []
          : toolbar(
              commands,
              messages,
              typeof assist.toolbar === "object" ? assist.toolbar.items : defaultToolbar,
            ),
      completion: completion(sources),
      hover: assist.hover === false ? [] : hoverHelp(messages),
      listKeys: assist.listKeys === false ? [] : listKeymap(),
    };
  };
  const initial = configured();
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
        compartments.gutter.of(initial.gutter),
        folding(),
        options.breadcrumbs === false ? [] : breadcrumbs(),
        compartments.toolbar.of(initial.toolbar),
        compartments.completion.of(initial.completion),
        compartments.hover.of(initial.hover),
        formattingKeymap(),
        // Before the default keys: Tab and Enter mean something in a list.
        compartments.listKeys.of(initial.listKeys),
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
          // The pressed state of commands follows the cursor and the parse.
          if (update.docChanged || update.selectionSet || update.transactions.length > 0) {
            const active = activeCommands(commands, update.state);
            if (active.join() !== snapshot.active.join()) publish({ active });
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
    commands,
    run(id) {
      const command = commands.find((candidate) => candidate.id === id);
      if (!command) return false;
      // A block action runs on the block under the cursor.
      const block =
        actionBlock(parsed(view.state).blocks, view.state.selection.main.head) ?? undefined;
      if (command.group === "blockAction" && (!block || command.appliesTo?.(block) === false)) {
        return false;
      }
      const done = command.run(view, block, view);
      view.focus();
      return done;
    },
    setAssist(next) {
      assist = next === false ? ALL_OFF : { ...assist, ...next };
      const now = configured();
      view.dispatch({
        effects: [
          compartments.gutter.reconfigure(now.gutter),
          compartments.toolbar.reconfigure(now.toolbar),
          compartments.completion.reconfigure(now.completion),
          compartments.hover.reconfigure(now.hover),
          compartments.listKeys.reconfigure(now.listKeys),
        ],
      });
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

/**
 * Every switch of `options` spelled out, for a declarative host (a
 * component prop) to pass to `setAssist`: what is not named is on.
 */
export function normalizeAssist(options: false | AssistOptions | undefined): AssistOptions {
  if (options === false) return ALL_OFF;
  return {
    slashMenu: options?.slashMenu !== false,
    toolbar: options?.toolbar ?? true,
    blockMenu: options?.blockMenu !== false,
    hover: options?.hover !== false,
    valueCompletion: options?.valueCompletion !== false,
    listKeys: options?.listKeys !== false,
    locale: options?.locale ?? "en",
  };
}

const ALL_OFF: AssistOptions = {
  slashMenu: false,
  toolbar: false,
  blockMenu: false,
  hover: false,
  valueCompletion: false,
  listKeys: false,
};

function asArray<T>(value: T | readonly T[] | undefined): readonly T[] {
  if (value === undefined) return [];
  return Array.isArray(value) ? (value as readonly T[]) : [value as T];
}
