// Engines: where the document is parsed.
import type { Engine, EditorViewData, ParseOutcome, ParseRequest } from "./types.js";

/** The exports of `@adoctra/wasm` or `@adoctra/wasm-preview` the editor uses. */
export interface AdoctraWasm {
  editor_json(src: string): string;
  preview_json?(src: string): string;
  stylesheet?(): string;
  workspace_preview_json?(workspace: WasmWorkspace, sourceId: number): string;
  ResourceWorkspace: new () => WasmWorkspace;
}

export interface WasmWorkspace {
  set(name: string, text: string): void;
  load(target: string, relativeTo?: number | null): number;
  editor_json(sourceId: number): string;
  free(): void;
}

/** Parse one request with an initialized module; the answer is its JSON. */
export function parseWith(wasm: AdoctraWasm, request: ParseRequest): string {
  const files = request.files ?? {};
  const names = Object.keys(files);
  if (names.length === 0) {
    return wasm.preview_json
      ? wasm.preview_json(request.text)
      : `{"html":null,"editor":${wasm.editor_json(request.text)}}`;
  }
  // Includes read from a workspace of buffers, rebuilt per parse so that no
  // snapshot outlives it.
  const workspace = new wasm.ResourceWorkspace();
  try {
    const main = request.name ?? "main.adoc";
    for (const name of names) {
      if (name !== main) workspace.set(name, files[name]!);
    }
    workspace.set(main, request.text);
    const id = workspace.load(main, null);
    return wasm.workspace_preview_json
      ? wasm.workspace_preview_json(workspace, id)
      : `{"html":null,"editor":${workspace.editor_json(id)}}`;
  } finally {
    workspace.free();
  }
}

export function readOutcome(json: string): ParseOutcome {
  const parsed = JSON.parse(json) as { html: string | null; editor: EditorViewData };
  return { view: parsed.editor, html: parsed.html };
}

/** An engine on the calling thread: for tests, small documents and servers. */
export function createInlineEngine(wasm: AdoctraWasm): Engine {
  return {
    preview: wasm.preview_json !== undefined,
    stylesheet: wasm.stylesheet?.() ?? null,
    parse: async (request) => readOutcome(parseWith(wasm, request)),
    dispose() {},
  };
}

// ---- worker protocol -------------------------------------------------------

export type ToWorker = { type: "parse"; id: number; request: ParseRequest };
export type FromWorker =
  | { type: "ready"; preview: boolean; stylesheet: string | null }
  | { type: "failed"; message: string }
  | { type: "parsed"; id: number; json: string }
  | { type: "error"; id: number; message: string };

/** What the client needs of a `Worker` (a test passes a double). */
export interface WorkerLike {
  postMessage(message: ToWorker): void;
  addEventListener(type: "message", listener: (event: { data: FromWorker }) => void): void;
  terminate(): void;
}

/**
 * An engine in a Web Worker running [`serve`](./worker.ts). Resolves when
 * the worker has loaded the module.
 */
export function createWorkerEngine(worker: WorkerLike): Promise<Engine> {
  return new Promise((resolve, reject) => {
    const waiting = new Map<
      number,
      { resolve: (outcome: ParseOutcome) => void; reject: (error: Error) => void }
    >();
    let next = 0;
    worker.addEventListener("message", ({ data }) => {
      switch (data.type) {
        case "ready":
          resolve({
            preview: data.preview,
            stylesheet: data.stylesheet,
            parse: (request) =>
              new Promise((resolve, reject) => {
                const id = next++;
                waiting.set(id, { resolve, reject });
                worker.postMessage({ type: "parse", id, request });
              }),
            dispose() {
              worker.terminate();
              for (const call of waiting.values()) call.reject(new Error("engine disposed"));
              waiting.clear();
            },
          });
          break;
        case "failed":
          reject(new Error(data.message));
          break;
        case "parsed":
        case "error": {
          const call = waiting.get(data.id);
          waiting.delete(data.id);
          if (data.type === "parsed") call?.resolve(readOutcome(data.json));
          else call?.reject(new Error(data.message));
          break;
        }
      }
    });
  });
}

/**
 * Keeps one parse in flight and only the newest request waiting: typing
 * faster than the parser never queues work, and outcomes arrive in order.
 * `deliver` gets each outcome with the request it answers.
 */
export class LatestParser {
  private running = false;
  private waiting: ParseRequest | null = null;
  private disposed = false;

  constructor(
    private readonly engine: Engine,
    private readonly deliver: (outcome: ParseOutcome, request: ParseRequest) => void,
    private readonly fail: (error: unknown) => void = () => {},
  ) {}

  request(request: ParseRequest): void {
    if (this.running) {
      this.waiting = request;
      return;
    }
    this.running = true;
    this.engine.parse(request).then(
      (outcome) => this.settle(() => this.deliver(outcome, request)),
      (error) => this.settle(() => this.fail(error)),
    );
  }

  private settle(report: () => void): void {
    this.running = false;
    if (this.disposed) return;
    const waiting = this.waiting;
    this.waiting = null;
    // A newer request supersedes this outcome.
    if (waiting) this.request(waiting);
    else report();
  }

  dispose(): void {
    this.disposed = true;
    this.waiting = null;
  }
}
