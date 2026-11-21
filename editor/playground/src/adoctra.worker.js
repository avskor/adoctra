// The whole worker of a host application: load the module, serve parses.
import init, * as wasm from "@adoctra/wasm-preview";
import { serve } from "@adoctra/editor-core/worker";

serve(init, wasm);
