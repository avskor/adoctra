#!/usr/bin/env bash
# Regenerate the golden reference fixtures from the *real* asciidoctor.
#
# These are the compatibility oracle: each `<name>.adoc` is rendered by
# asciidoctor (embedded mode, no header/footer) into `<name>.expected.html`.
# The golden test (../golden.rs) compares our HTML backend against them after
# normalizing whitespace and unwrapping asciidoctor's `<div class="paragraph">`.
#
# Requires Ruby and the pinned asciidoctor gem. Re-run after
# changing the inputs or when bumping the reference asciidoctor version.
set -euo pipefail
cd "$(dirname "$0")"

reference=../../../../scripts/asciidoctor-reference.rb
# Check activation before opening any output files, so a missing gem cannot
# truncate a committed reference.
ruby "$reference" </dev/null >/dev/null
echo "using Asciidoctor $(cat ../../../../compatibility/asciidoctor-version)"
# Top-level inline fixtures, flat-list fixtures under lists/, block-level
# fixtures (e.g. source listings) under blocks/, literal blocks under literals/,
# admonitions under admonitions/ (icons under admonition_icons/, below), example blocks under examples/, sidebar blocks
# under sidebars/, block titles under titles/, quote/verse blocks under quotes/
# and verses/, open blocks under opens/, passthrough blocks under passthrough/,
# tables of contents under toc/, images under images/, video/audio under
# media/, tables under tables/, footnotes under footnotes/, description
# lists under dlists/, callouts under callouts/, thematic/page breaks
# under breaks/, manual pages under manpage/ and section titles under
# sections/.
for adoc in *.adoc lists/*.adoc blocks/*.adoc literals/*.adoc admonitions/*.adoc examples/*.adoc sidebars/*.adoc titles/*.adoc quotes/*.adoc verses/*.adoc opens/*.adoc passthrough/*.adoc toc/*.adoc images/*.adoc media/*.adoc tables/*.adoc footnotes/*.adoc dlists/*.adoc callouts/*.adoc breaks/*.adoc manpage/*.adoc sections/*.adoc math/blocks.adoc; do
  [ -e "$adoc" ] || continue
  name="${adoc%.adoc}"
  ruby "$reference" <"$adoc" >"${name}.expected.html"
  echo "  ${name}.expected.html"
done
ruby "$reference" --mathjax-config <math/mathjax.adoc >math/mathjax.expected.html
# Client-side syntax highlighters under highlight/, standalone in safe mode
# (secure mode locks source-highlighter).
for adoc in highlight/*.adoc; do
  ruby "$reference" --standalone --safe <"$adoc" >"${adoc%.adoc}.expected.html"
  echo "  ${adoc%.adoc}.expected.html"
done
# Admonition icons under admonition_icons/, in safe mode (secure mode locks
# icons).
for adoc in admonition_icons/*.adoc; do
  ruby "$reference" --safe <"$adoc" >"${adoc%.adoc}.expected.html"
  echo "  ${adoc%.adoc}.expected.html"
done
# Standalone documents (head, header and footer) under document/.
for adoc in document/*.adoc; do
  ruby "$reference" --standalone <"$adoc" >"${adoc%.adoc}.expected.html"
  echo "  ${adoc%.adoc}.expected.html"
done
