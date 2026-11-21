import { describe, expect, it } from "vitest";
import {
  createWorkerEngine,
  LatestParser,
  parseWith,
  type AdoctraWasm,
  type FromWorker,
  type ToWorker,
} from "../src/engine.js";
import { serve } from "../src/worker.js";
import type { Engine, ParseOutcome, ParseRequest } from "../src/types.js";

const view = (text: string) =>
  `{"version":1,"source_id":0,"tokens":[],"blocks":[],"outline":[],"anchors":[],"attributes":{"text":${JSON.stringify(text)}},"diagnostics":[]}`;

function fakeWasm(log: string[] = []): AdoctraWasm {
  return {
    editor_json: (src) => view(src),
    preview_json: (src) => `{"html":"<p>${src}</p>","editor":${view(src)}}`,
    stylesheet: () => "body{}",
    workspace_preview_json: (_workspace, id) => `{"html":"ws${id}","editor":${view("ws")}}`,
    ResourceWorkspace: class {
      set(name: string, text: string) {
        log.push(`set ${name}=${text}`);
      }
      load(target: string) {
        log.push(`load ${target}`);
        return 7;
      }
      editor_json() {
        return view("ws");
      }
      free() {
        log.push("free");
      }
    },
  };
}

describe("parseWith", () => {
  it("parses a lone buffer directly", () => {
    expect(JSON.parse(parseWith(fakeWasm(), { text: "a" })).html).toBe("<p>a</p>");
    const parser: AdoctraWasm = { ...fakeWasm(), preview_json: undefined };
    expect(JSON.parse(parseWith(parser, { text: "a" }))).toMatchObject({
      html: null,
      editor: { attributes: { text: "a" } },
    });
  });

  it("reads includes from a workspace it frees", () => {
    const log: string[] = [];
    const json = parseWith(fakeWasm(log), {
      text: "main",
      name: "doc/a.adoc",
      files: { "doc/b.adoc": "b", "doc/a.adoc": "stale" },
    });
    expect(JSON.parse(json).html).toBe("ws7");
    expect(log).toEqual(["set doc/b.adoc=b", "set doc/a.adoc=main", "load doc/a.adoc", "free"]);
  });
});

/** A worker double running `serve` on the other end of a message pair. */
function fakeWorker(init: () => Promise<unknown>) {
  const toMain: ((event: { data: FromWorker }) => void)[] = [];
  const toWorker: ((event: { data: ToWorker }) => void)[] = [];
  const worker = {
    terminated: false,
    postMessage: (data: ToWorker) => queueMicrotask(() => toWorker.forEach((f) => f({ data }))),
    addEventListener: (_: "message", f: (event: { data: FromWorker }) => void) => void toMain.push(f),
    terminate() {
      this.terminated = true;
    },
  };
  void serve(init, fakeWasm(), undefined, {
    postMessage: (data) => queueMicrotask(() => toMain.forEach((f) => f({ data }))),
    addEventListener: (_, f) => void toWorker.push(f),
  });
  return worker;
}

describe("worker engine", () => {
  it("answers requests after the module loads", async () => {
    const engine = await createWorkerEngine(fakeWorker(async () => {}));
    expect(engine.preview).toBe(true);
    expect(engine.stylesheet).toBe("body{}");
    const [a, b] = await Promise.all([engine.parse({ text: "a" }), engine.parse({ text: "b" })]);
    expect(a.html).toBe("<p>a</p>");
    expect(b.view.attributes.text).toBe("b");
  });

  it("rejects when the module fails to load", async () => {
    await expect(
      createWorkerEngine(fakeWorker(() => Promise.reject(new Error("no wasm")))),
    ).rejects.toThrow("no wasm");
  });
});

describe("LatestParser", () => {
  it("keeps one parse in flight and only the newest request", async () => {
    const started: string[] = [];
    const finish: (() => void)[] = [];
    const engine: Engine = {
      preview: false,
      stylesheet: null,
      parse: (request: ParseRequest) =>
        new Promise<ParseOutcome>((resolve) => {
          started.push(request.text);
          finish.push(() => resolve({ view: JSON.parse(view(request.text)), html: null }));
        }),
      dispose() {},
    };
    const delivered: string[] = [];
    const parser = new LatestParser(engine, (_, request) => delivered.push(request.text));
    parser.request({ text: "1" });
    parser.request({ text: "2" });
    parser.request({ text: "3" });
    expect(started).toEqual(["1"]);
    finish[0]!();
    await new Promise((r) => setTimeout(r));
    // "1" was superseded: not delivered, and "2" never ran.
    expect(started).toEqual(["1", "3"]);
    expect(delivered).toEqual([]);
    finish[1]!();
    await new Promise((r) => setTimeout(r));
    expect(delivered).toEqual(["3"]);
  });
});
