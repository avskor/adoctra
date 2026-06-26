#!/usr/bin/env bash
# Regenerate the golden reference fixtures from the *real* asciidoctor.
#
# These are the compatibility oracle: each `<name>.adoc` is rendered by
# asciidoctor (embedded mode, no header/footer) into `<name>.expected.html`.
# The golden test (../golden.rs) compares our HTML backend against them after
# normalizing whitespace and unwrapping asciidoctor's `<div class="paragraph">`.
#
# Requires `asciidoctor` on PATH (developed against 2.0.23). Re-run after
# changing the inputs or when bumping the reference asciidoctor version.
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v asciidoctor >/dev/null 2>&1; then
  echo "asciidoctor not found on PATH; cannot regenerate fixtures" >&2
  exit 1
fi

echo "using $(asciidoctor --version | head -1)"
# Top-level inline fixtures, flat-list fixtures under lists/, and block-level
# fixtures (e.g. source listings) under blocks/.
for adoc in *.adoc lists/*.adoc blocks/*.adoc; do
  [ -e "$adoc" ] || continue
  name="${adoc%.adoc}"
  asciidoctor -s -o - "$adoc" >"${name}.expected.html"
  echo "  ${name}.expected.html"
done
