#!/usr/bin/env bash
# Publish the npm packages of the editor in dependency order (CI only:
# NODE_AUTH_TOKEN). Versions already on the registry are skipped, so a
# rerun resumes. Run after `npm ci`, `npm run build:wasm` and
# `npm run build`; `--dry-run` packs without publishing.
set -euo pipefail
cd "$(dirname "$0")/.."
packages=(wasm wasm-preview editor-core editor-vue editor-react)
for package in "${packages[@]}"; do
  dir="packages/$package"
  name=$(node -p "require('./$dir/package.json').name")
  version=$(node -p "require('./$dir/package.json').version")
  if [[ "${1:-}" != "--dry-run" ]] && npm view "$name@$version" version >/dev/null 2>&1; then
    echo "$name $version is already on npm"
    continue
  fi
  echo "publishing $name $version"
  npm publish --workspace "$dir" --access public "$@"
done
