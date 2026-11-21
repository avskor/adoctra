// The editor view of `adoctra-wasm` (`editor_json`, docs/editor.adoc).
// Every offset is a UTF-16 code unit offset into the edited source.

/** `[start, end, kind]`; the kind is a `TokenKind` name (`strong`, `heading`, …). */
export type Token = [start: number, end: number, kind: string];

/** A node of the block tree, flattened in document order. */
export interface Block {
  start: number;
  end: number;
  /** Nesting depth; a node's ancestors precede it with smaller depths. */
  depth: number;
  /** `paragraph`, `listing`, `section`, `list`, `list-item`, `table-cell`, … */
  kind: string;
  /** Start of the `[[id]]` / `[attributes]` / `.Title` lines above the block. */
  meta_start?: number;
  level?: number;
  ordered?: boolean;
  /** `note`, `tip`, `important`, `warning`, `caution`. */
  admonition?: string;
  style?: string;
  language?: string;
}

export interface OutlineEntry {
  level: number;
  start: number;
  end: number;
  id: string | null;
  /** The title's source text. */
  title: string;
}

export interface Anchor {
  id: string;
  start: number;
  end: number;
}

export interface Diagnostic {
  /** The source the diagnostic is in; the range is UTF-16 only in the edited one. */
  source_id: number;
  start: number;
  end: number;
  severity: "error" | "warning" | "info";
  message: string;
}

export interface EditorViewData {
  version: number;
  /** The id of the edited source (diagnostics may name another). */
  source_id: number;
  tokens: Token[];
  blocks: Block[];
  outline: OutlineEntry[];
  anchors: Anchor[];
  attributes: Record<string, string>;
  diagnostics: Diagnostic[];
}

/** One parse of the document. */
export interface ParseOutcome {
  view: EditorViewData;
  /** Embedded HTML, when the engine converts (`@adoctra/wasm-preview`). */
  html: string | null;
}

/** Other files of the document (include targets), by workspace path. */
export type Files = Record<string, string>;

export interface ParseRequest {
  text: string;
  /** The edited file's workspace path, for relative includes. */
  name?: string;
  files?: Files;
}

/** Parses the document somewhere (a worker, the main thread, a test double). */
export interface Engine {
  /** Whether outcomes carry `html`. */
  readonly preview: boolean;
  /** The stylesheet of the preview's markup, when the engine converts. */
  readonly stylesheet: string | null;
  parse(request: ParseRequest): Promise<ParseOutcome>;
  dispose(): void;
}
