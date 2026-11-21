#!/usr/bin/env bash
# Build the two WASM packages of the editor:
#   packages/wasm/pkg          adoctra-wasm          (parser only)
#   packages/wasm-preview/pkg  adoctra-wasm-preview  (parser + HTML preview)
# Uses the wasm-bindgen CLI of the version in Cargo.lock
# (`cargo install wasm-bindgen-cli --version <that>`) and Binaryen's
# wasm-opt (installed by `npm ci`).
set -euo pipefail
editor="$(cd "$(dirname "$0")/.." && pwd)"
root="$(cd "$editor/.." && pwd)"
cd "$root"
cargo build --release --locked --target wasm32-unknown-unknown \
  -p adoctra-wasm -p adoctra-wasm-preview
out="$root/target/wasm32-unknown-unknown/release"
build() { # <crate file name> <package directory>
  local pkg="$editor/packages/$2/pkg"
  rm -rf "$pkg"
  wasm-bindgen --target web --out-dir "$pkg" "$out/$1.wasm"
  # Shrink the module with Binaryen: the `binaryen` package of `npm ci`,
  # else a wasm-opt on the PATH. The features come from the module itself.
  local opt="$editor/node_modules/.bin/wasm-opt"
  [ -x "$opt" ] || opt=$(command -v wasm-opt || true)
  if [ -n "$opt" ]; then
    "$opt" -Oz -o "$pkg/$1_bg.wasm" "$pkg/$1_bg.wasm"
  else
    echo "wasm-opt not found: $2 is not optimized" >&2
  fi
  printf '%s: %s bytes, gzip %s\n' "$2" \
    "$(wc -c <"$pkg/$1_bg.wasm")" "$(gzip -9c "$pkg/$1_bg.wasm" | wc -c)"
}
build adoctra_wasm wasm
build adoctra_wasm_preview wasm-preview
