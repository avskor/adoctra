// The worker side of `createWorkerEngine`. A host's worker file is:
//
//   import init, * as wasm from "@adoctra/wasm-preview"; // or "@adoctra/wasm"
//   import { serve } from "@adoctra/editor-core/worker";
//   serve(init, wasm);
import { parseWith, type AdoctraWasm, type FromWorker, type ToWorker } from "./engine.js";

interface Scope {
  postMessage(message: FromWorker): void;
  addEventListener(type: "message", listener: (event: { data: ToWorker }) => void): void;
}

/**
 * Load the module with `init` (wasm-bindgen's default export; pass the
 * `.wasm` URL through `input` when the bundler does not resolve it) and
 * answer parse requests.
 */
export function serve(
  init: (input?: unknown) => Promise<unknown>,
  wasm: AdoctraWasm,
  input?: unknown,
  scope: Scope = globalThis as unknown as Scope,
): Promise<void> {
  const queued: ToWorker[] = [];
  let ready = false;
  const answer = (message: ToWorker) => {
    try {
      scope.postMessage({ type: "parsed", id: message.id, json: parseWith(wasm, message.request) });
    } catch (error) {
      scope.postMessage({ type: "error", id: message.id, message: String(error) });
    }
  };
  scope.addEventListener("message", ({ data }) => {
    if (ready) answer(data);
    else queued.push(data);
  });
  return init(input).then(
    () => {
      ready = true;
      scope.postMessage({
        type: "ready",
        preview: wasm.preview_json !== undefined,
        stylesheet: wasm.stylesheet?.() ?? null,
      });
      queued.splice(0).forEach(answer);
    },
    (error) => scope.postMessage({ type: "failed", message: String(error) }),
  );
}
