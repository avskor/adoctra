#!/usr/bin/env bash
# Publish the workspace crates to crates.io in dependency order (CI only:
# CARGO_REGISTRY_TOKEN). Versions already on crates.io are skipped, so a
# rerun resumes; crates.io's rate limit for new crates (429) is waited out.
set -euo pipefail

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
crates=(adoctra-highlight adoctra-core adoctra-html adoctra-docx adoctra-json
        adoctra-ext-diagram adoctra adoctra-cli adoctra-wasm)
agent="adoctra-release (https://github.com/avskor/adoctra)"

published() {
  curl -sf -A "$agent" "https://crates.io/api/v1/crates/$1/$version" > /dev/null
}

for crate in "${crates[@]}"; do
  if published "$crate"; then
    echo "$crate $version is already on crates.io"
    continue
  fi
  ok=
  for attempt in 1 2 3 4 5 6; do
    if cargo publish --locked -p "$crate" 2>&1 | tee publish.log; then
      ok=1
      break
    fi
    if grep -q '429 Too Many Requests' publish.log; then
      echo "rate limited (attempt $attempt); waiting 11 minutes"
      sleep 660
    else
      exit 1
    fi
  done
  test -n "$ok" || { echo "$crate was not published"; exit 1; }
done
