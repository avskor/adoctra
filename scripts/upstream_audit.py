#!/usr/bin/env python3
"""Differential audit of every document the pinned Asciidoctor test suite builds.

`--record UPSTREAM` runs the complete upstream test suite of the pinned commit
(each test file in its own Ruby process) with scripts/upstream_record.rb, which
records each top-level Asciidoctor::Document the tests construct: its source,
the options the test used and the test declaration (file, line, name). Entries
are deduplicated by (source, options) and written to the inventory with every
provenance occurrence and the SHA-256 of each upstream test file.

The default run converts each supported entry with pinned Ruby (one process,
scripts/upstream-audit-reference.rb, the recorded options) and with the CLI
(`-S` safe mode, `-s` for embedded output, `-a` attributes and doctype), and
compares the existing body-dom-v1 projection (standalone shells mapped like
the asset audit), the standalone document shell (`<html>`/`<body>` attributes
and the `<head>` children, see `shell`) and the ordered warning/error texts
with their logger cursor (file and line, or none), and the document model
(block structure with each block's metadata, and header/final attribute
snapshots, against the CLI's JSON). Both sides get the same
SOURCE_DATE_EPOCH and TZ. Statuses: PASS, DIFF (body and/or diagnostics),
RUBY_ERROR, ADOCTRA_ERROR. Entries whose options this CLI contract cannot
express (non-HTML backends, extensions, base_dir/to_file, …) are UNSUPPORTED
with their reason, never silently dropped.

This audits observable behaviour of upstream inputs; it is not running the
upstream assertions, and a PASS is limited to the projection. `--baseline`
freezes every status: a changed status (regression, improvement or new
error) fails, so improvements are re-recorded deliberately.
"""

import argparse
import html as html_module
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import re
import socketserver
import subprocess
import sys
import tempfile
import threading
from contextlib import contextmanager

from compatibility import BodyParser, Element, projection
import structure

ROOT = Path(__file__).resolve().parent.parent
AUDIT = ROOT / "compatibility" / "upstream" / "audit"
INVENTORY = AUDIT / "inventory.json"
SOURCES = ROOT / "compatibility" / "upstream" / "sources" / "test"
COMMIT = "2c03d22eb9fe83d04ee452939581d1b842f6d8b4"
SUPPORTED_OPTIONS = {"standalone", "header_footer", "parse", "attributes", "safe", "doctype",
                     "input_mtime", "warnings", "failure_level", "linkcss_default", "base_dir"}
# Ruby API options that only fill API state (source locations, the asset
# catalog, timings, the eRuby implementation of templates the html5 converter
# does not use): the reference passes them on, the CLI has nothing to change.
PASSIVE_OPTIONS = {"sourcemap", "catalog_assets", "timings", "eruby", "in_place"}
# Ruby's file output (Asciidoctor.convert writing a file): such an entry runs
# as one CLI invocation per side (`-D`/`-o`, see run_cli) in the materialized
# checkout, and the files it writes are compared too.
OUTPUT_OPTIONS = {"to_dir", "to_file"}
# The attributes Ruby's convert_file derives from an input file: a file
# input supplies them itself.
INPUT_ATTRIBUTES = ("docfile", "docdir", "docfilesuffix", "docname")
# Backends Ruby's html5 converter handles (`xhtml*` sets htmlsyntax=xml); a
# trailing `@` is the soft-set attribute spelling.
HTML_BACKENDS = ("html5", "html", "xhtml5", "xhtml")
FIXTURES = SOURCES / "fixtures"
PLACEHOLDER = "<UPSTREAM>"
SAFE = {"unsafe": "unsafe", "safe": "safe", "server": "server", "secure": "secure",
        0: "unsafe", 1: "safe", 10: "server", 20: "secure"}
ENVIRONMENT = {"SOURCE_DATE_EPOCH": "1700000000", "TZ": "UTC"}
PROFILE = "upstream-audit-body-shell-positions-model-v7"
# Ruby's default `webfonts` (an empty value) links these Google fonts; the
# adoctra theme uses system fonts (an explicit `webfonts` value is compared).
# The opening comments of Ruby's default stylesheet and adoctra.css.
DEFAULT_STYLESHEET_HEADERS = ("/*! Asciidoctor default stylesheet", "/* adoctra reference stylesheet.")
DEFAULT_WEBFONTS = ("css?family=Open+Sans:300,300italic,400,400italic,600,600italic"
                    "%7CNoto+Serif:400,400italic,700,700italic%7CDroid+Sans+Mono:400,700")
# Line 0 is Ruby's `cursor_at_prev_line` on a first line.
DIAGNOSTIC = re.compile(r"adoctra: (?:(.+?):(0|[1-9]\d*): )?(warning|error|info): (.*)", re.DOTALL)


def source_name(name, root):
    """A diagnostic's file as both sides can compare it: standard input as
    `<stdin>`, a file inside the materialized checkout relative to it."""
    if name in (None, "-", "<stdin>"):
        return "<stdin>"
    if root is not None:
        prefix = str(root).rstrip("/") + "/"
        if name.startswith(prefix):
            return name[len(prefix):]
    return name


def cli_diagnostics(stderr, root=None):
    """Warnings and errors from the CLI's stderr with their file:line (a
    line without the prefix continues the previous message, which keeps its
    line feed like Ruby's)."""
    lines = []
    for line in stderr.removesuffix("\n").split("\n") if stderr else []:
        if lines and not line.startswith("adoctra: "):
            lines[-1] += "\n" + line
        else:
            lines.append(line)
    diagnostics = []
    for line in lines:
        match = DIAGNOSTIC.fullmatch(line)
        if match is None:
            raise ValueError(f"unrecognized adoctra diagnostic: {line!r}")
        name, lineno, severity, message = match.groups()
        if severity == "info":
            continue
        location = {"file": source_name(name, root), "line": int(lineno)} if name else None
        diagnostics.append({"severity": severity, "message": message, "location": location})
    return diagnostics


def reference_diagnostics(diagnostics, root=None):
    """Ruby's logged diagnostics with the same file names as the CLI's."""
    result = []
    for diagnostic in diagnostics:
        location = diagnostic.get("location")
        if location is not None:
            location = {"file": source_name(location["file"], root), "line": location["line"]}
        result.append({"severity": diagnostic["severity"], "message": diagnostic["message"],
                       "location": location})
    return result


TAGS = re.compile(r"<[^>]*>")


def attribute_text(value):
    """An attribute value as text: Ruby stores it after the header
    substitutions (markup and character references), adoctra in its source
    spelling with formatted parts beside it; both compare without markup."""
    return html_module.unescape(TAGS.sub("", value))


# Block metadata (`meta` on structure nodes): a context's default style is
# no explicit one (Ruby stores it, adoctra leaves it unset); an admonition's
# style is its type, already in the structure. Ruby's attributes that only
# hold parser state or repeat other fields are not compared; the media
# target and alt and a quote's attribution compare as their own fields.
DEFAULT_STYLES = {"listing": {"listing"}, "literal": {"literal"}, "example": {"example"}, "open": {"open"},
                  "sidebar": {"sidebar"}, "quote": {"quote"}, "verse": {"verse"}, "pass": {"pass"},
                  "table": {"table"}, "olist": {"arabic"}, "colist": {"arabic"}, "paragraph": {"normal"},
                  "floating_title": {"discrete", "float"}}
INTERNAL_ATTRIBUTES = {"style", "id", "role", "title", "reftext", "options", "attribute_entries",
                       "cloaked-context", "textlabel", "name", "tablepcwidth", "rowcount", "colcount",
                       "default-alt", "caption", "attribution", "citetitle", "target", "alt", "imagesdir"}
MEDIA = ("image", "video", "audio")


def inline_text(nodes):
    """The text of a JSON inline list (raw markup dropped, references decoded)."""
    out = []
    for node in nodes or []:
        if node.get("name") == "image":
            # Ruby's text of an inline image: an icon without `icons` is
            # `[alt]`; an image or a font/image icon is markup only.
            out.append(f"[{node.get('alt', '')}]" if node.get("icon") and node.get("icons") is None else "")
        elif node.get("name") == "footnote":
            # Ruby renders the marker: `[index]`, or `[id]` when unresolved.
            out.append(f"[{node.get('index') or node.get('id') or ''}]")
        elif node.get("inlines") is not None:
            out.append(inline_text(node["inlines"]))
        elif node.get("parts"):
            out.append("".join(attribute_text(p["value"]) if p.get("kind") == "raw" else p.get("value", "")
                               for p in node["parts"] if "value" in p))
        elif node.get("name") == "raw":
            out.append(attribute_text(node.get("value", "")))
        elif isinstance(node.get("value"), str):
            out.append(node["value"])
    return "".join(out)


def normal_metadata(context, style, options, attributes, fields):
    if context == "admonition" or style in DEFAULT_STYLES.get(context, set()):
        style = None
    # Block attribute values are source text on both sides (an attribute
    # list only substitutes attribute references), so they compare as is.
    named = {name: value for name, value in attributes.items()
             if value is not None and name not in INTERNAL_ATTRIBUTES and not name.endswith("-option")}
    return {"style": style, "options": sorted(set(options)), "attributes": named,
            **{name: value for name, value in fields.items() if value is not None}}


def adoctra_location(node, root=None):
    location = node.get("location")
    return None if location is None else {"file": source_name(location["file"], root), "line": location["line"]}


def adoctra_metadata(block, root=None):
    """structure.py's `meta` hook for adoctra's JSON (a section's heading;
    a list item or table cell has only its location)."""
    if "name" not in block:
        return "item", {"location": adoctra_location(block, root)}
    # A section passes its heading; a standalone heading is a floating title.
    floating = block.get("style") in ("discrete", "float")
    context = {"heading": "floating_title" if floating else "section",
               "list": "olist" if block.get("ordered") else "ulist"}.get(block["name"], block["name"])
    fields = {}
    if block["name"] in MEDIA:
        fields["target"] = block.get("target")
        if block["name"] == "image":
            fields["alt"] = attribute_text(block.get("alt") or "")
    if block.get("reftext") is not None:
        fields["reftext"] = inline_text(block["reftext"])
    # The title as text (a section's is its heading) and the caption (an
    # admonition's label; a section's number is no caption in Ruby).
    heading = block["name"] == "heading"
    title = block.get("inlines") if heading else block.get("title")
    fields["title"] = inline_text(title) if title is not None else None
    fields["caption"] = None if heading else (block.get("label") if block["name"] == "admonition"
                                              else block.get("caption"))
    attribution = block.get("attribution") or {}
    if attribution.get("author"):
        fields["attribution"] = inline_text(attribution["author"])
    if attribution.get("citetitle"):
        fields["citetitle"] = inline_text(attribution["citetitle"])
    if context != "preamble":
        fields["location"] = adoctra_location(block, root)
    return context, normal_metadata(context, block.get("style"), block.get("options", []),
                                    block.get("attributes", {}), fields)


def reference_location(location, root=None, docfile=None):
    """Ruby's cursor file names the primary document by its `docfile` (the
    harness passes it on standard input): that is the CLI's `<stdin>`."""
    if location is None:
        return None
    primary = docfile is not None and location["file"] in (docfile, docfile.rsplit("/", 1)[-1])
    file = "<stdin>" if primary else location["file"]
    return {"file": source_name(file, root), "line": location["line"]}


def reference_metadata(node, root=None, docfile=None):
    """Ruby's `meta` of each structure node, normalized like adoctra's."""
    if isinstance(node, list):
        for child in node:
            reference_metadata(child, root, docfile)
        return
    if not isinstance(node, dict):
        return
    for key, value in node.items():
        if key != "meta":
            reference_metadata(value, root, docfile)
    meta = node.get("meta")
    if meta is None:
        return
    if "context" not in node:
        # A list item or table cell.
        node["meta"] = {"location": reference_location(meta.get("location"), root, docfile)}
        return
    context = node["context"]
    attributes = meta["attributes"]
    fields = {}
    if context in MEDIA:
        fields["target"] = attributes.get("target")
        if context == "image":
            fields["alt"] = attribute_text(meta.get("alt") or "")
    for name in ("attribution", "citetitle"):
        if context in ("quote", "verse") and attributes.get(name) is not None:
            fields[name] = attribute_text(attributes[name])
    if meta.get("reftext") is not None:
        fields["reftext"] = attribute_text(meta["reftext"])
    fields["title"] = attribute_text(meta["title"]) if meta.get("title") is not None else None
    fields["caption"] = None if context == "section" else (
        attribute_text(meta["caption"]) if meta.get("caption") is not None else None)
    if context != "preamble":
        fields["location"] = reference_location(meta.get("location"), root, docfile)
    node["meta"] = normal_metadata(context, meta["style"], meta["options"], attributes, fields)


def metadata_differences(expected, actual, path="", found=None, fields=None):
    """The metadata fields that differ, by node path: all but `location`, or
    only those in `fields`."""
    found = [] if found is None else found
    if isinstance(expected, dict) and isinstance(actual, dict):
        if "meta" in expected and "meta" in actual:
            names = sorted(name for name in set(expected["meta"]) | set(actual["meta"])
                           if (name in fields if fields is not None else name != "location")
                           and expected["meta"].get(name) != actual["meta"].get(name))
            if names:
                found.append(f"{path or 'document'}:{expected.get('context', 'item')}:{','.join(names)}")
        for key in expected:
            if key != "meta" and key in actual:
                metadata_differences(expected[key], actual[key], f"{path}.{key}" if path else key, found, fields)
    elif isinstance(expected, list) and isinstance(actual, list):
        for index, (x, y) in enumerate(zip(expected, actual)):
            metadata_differences(x, y, f"{path}[{index}]", found, fields)
    return found


def compare_locations(expected, actual):
    """The separate location channel: where each block, list item and table
    cell starts. Ruby's `source_location` is its reader cursor (with known
    quirks; one upstream test asserts an "incorrect lineno"), adoctra's the
    physical start of the node, so differences are frozen per entry in the
    baseline rather than failing the entry. None without a compared model."""
    if (expected is None or actual is None or "error" in expected or "error" in actual
            or structure.strip_meta(expected["structure"]) != structure.strip_meta(actual["structure"])):
        return None
    differences = metadata_differences(expected["structure"], actual["structure"], fields={"location"})
    return "DIFF" if differences else "PASS"


def parts_text(parts):
    """The text of semantic parts (raw markup dropped, references decoded)."""
    return "".join(attribute_text(part["value"]) if part.get("kind") == "raw" else part.get("value", "")
                   for part in parts if "value" in part)


def model_attributes(snapshot, root):
    """adoctra's attribute snapshot (`--json-attributes`) as text values."""
    result = {}
    for name, value in snapshot["values"].items():
        parts = snapshot.get("attribute_parts", {}).get(name)
        if parts:
            value = "".join(attribute_text(part["value"]) if part["kind"] == "raw" else part.get("value", "")
                            for part in parts)
        else:
            value = html_module.unescape(value)
        result[name] = localize_back(value, root)
    return result


def reference_model(model, root, docfile=None):
    """Ruby's model with the same attribute text and checkout paths; a
    structure outside the block-structure-v1 schema is an error, not a pass."""
    try:
        structure.validate_structure(structure.strip_meta(model["structure"]))
    except ValueError as error:
        return {"error": f"ValueError: {error}"}
    reference_metadata(model["structure"], root, docfile)
    attributes = {snapshot: {name: None if value is None else localize_back(attribute_text(value), root)
                             for name, value in values.items()}
                  for snapshot, values in model["attributes"].items()}
    footnotes = [dict(entry, text=attribute_text(entry["text"])) for entry in model.get("footnotes", [])]
    return {"structure": model["structure"], "attributes": attributes,
            "references": model.get("references", []), "footnotes": footnotes}


def localize_back(value, root):
    return value.replace(str(root), PLACEHOLDER) if root is not None else value


def shell(html):
    """The standalone document around the body: the `<html>` and `<body>`
    attributes and the `<head>` children (tag, attributes, text). Three
    differences are by design: the generator and http-equiv metas, the
    content of the default stylesheet (only its place is compared, embedded
    or linked, recognized by its opening comment) and Ruby's default webfonts
    link. None for a fragment."""
    parser = BodyParser()
    parser.feed(html)
    parser.close()
    documents = [n for n in parser.root.children if isinstance(n, Element) and n.tag == "html"]
    if not documents:
        return None
    result = {"html": dict(sorted(documents[0].attrs.items())), "head": [], "body": None}
    for node in documents[0].children:
        if not isinstance(node, Element):
            continue
        if node.tag == "body":
            result["body"] = dict(sorted(node.attrs.items()))
        if node.tag != "head":
            continue
        for child in node.children:
            if not isinstance(child, Element):
                continue
            attrs = dict(child.attrs)
            if child.tag == "meta" and ("http-equiv" in attrs or attrs.get("name") == "generator"):
                continue
            if child.tag == "link" and attrs.get("href", "").endswith(DEFAULT_WEBFONTS):
                continue
            text = "".join(c for c in child.children if isinstance(c, str)).strip()
            if child.tag == "style" and text.startswith(DEFAULT_STYLESHEET_HEADERS):
                result["head"].append(["style", {}, "<default stylesheet>"])
                continue
            href = attrs.get("href", "")
            if child.tag == "link" and attrs.get("rel") == "stylesheet" \
                    and href.rsplit("/", 1)[-1] in DEFAULT_STYLESHEETS:
                attrs["href"] = href[:len(href) - len(href.rsplit("/", 1)[-1])] + "<default stylesheet>"
            result["head"].append([child.tag, dict(sorted(attrs.items())), text])
    return result


def entry_id(source, options):
    data = json.dumps([source, options], sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(data.encode()).hexdigest()[:16]


def attribute_args(attributes):
    """Ruby's API attribute forms as CLI `-a` spellings, or None if unsupported."""
    if attributes is None:
        return []
    if isinstance(attributes, str):
        attributes = attributes.split()
    if isinstance(attributes, list):
        return [str(a) for a in attributes] if all(isinstance(a, str) for a in attributes) else None
    if not isinstance(attributes, dict):
        return None
    spelled = []
    for name, value in attributes.items():
        # Ruby's API: nil unsets a key hard, false softly (a document entry
        # may set it again), spelled `name!@` like Ruby's string form.
        if value is None:
            spelled.append(name if name.endswith("!") else f"{name}!")
        elif value is False:
            spelled.append(f"{name.removesuffix('!')}!@")
        elif value is True:
            spelled.append(name)
        elif isinstance(value, (str, int, float)) and not str(value).startswith("#<"):
            spelled.append(f"{name}={value}")
        else:
            return None
    return spelled


def classify(source, options):
    """(cli arguments, None) for a supported entry, else (None, reason)."""
    if source.startswith("#<"):
        return None, "source:non-string"
    backend = options.get("backend")
    if backend not in (None, *HTML_BACKENDS):
        return None, f"backend:{backend}"
    extra = sorted(set(options) - SUPPORTED_OPTIONS - PASSIVE_OPTIONS - OUTPUT_OPTIONS - {"backend"})
    if extra:
        return None, "option:" + ",".join(extra)
    safe = options.get("safe", "secure")
    # Ruby's Document#initialize: an unknown level name falls back to secure
    # (`value_for_name … rescue SECURE`); other integers stay as given.
    safe = SAFE.get(safe, "secure" if isinstance(safe, str) else None)
    if safe is None:
        return None, f"safe:{options.get('safe')}"
    output = []
    if options.keys() & OUTPUT_OPTIONS:
        for key, flag in (("to_dir", "-D"), ("to_file", "-o")):
            value = options.get(key)
            if value is True or value is None:
                continue  # Ruby's convert: `to_file: true` is no explicit file
            if not isinstance(value, str):
                return None, f"option:{key}:non-string"
            if value.startswith("/"):
                return None, f"option:{key}:outside-checkout"
            output += [flag, value]
    input_file = cli_input(source, options)
    attributes = options.get("attributes")
    if input_file and isinstance(attributes, dict):
        attributes = {k: v for k, v in attributes.items() if k not in INPUT_ATTRIBUTES}
    attributes = attribute_args(attributes)
    if attributes is None:
        return None, "attributes:non-string"
    backends = [a.split("=", 1)[1] for a in attributes if a.startswith("backend=")]
    # Below server mode the document itself may switch to another backend,
    # unless the host set one (an override wins).
    if not backends and backend is None and safe in ("unsafe", "safe"):
        for value in re.findall(r"^:backend:[ \t]*(\S*)", source, re.MULTILINE):
            if value not in HTML_BACKENDS:
                return None, f"backend:document:{value}"
    if any(b.removesuffix("@") not in HTML_BACKENDS for b in backends):
        return None, f"backend:{backends[-1]}"
    args = ["-S", safe]
    if options.get("base_dir"):
        args += ["-B", str(options["base_dir"])]
    if not (options.get("standalone") or options.get("header_footer")):
        args.append("-s")
    for attribute in attributes:
        args += ["-a", attribute]
    # Ruby's :backend and :doctype options are attribute overrides set after
    # the :attributes (Document#initialize), so they win: the CLI's last `-a`.
    if backend:
        args += ["-a", f"backend={backend}"]
    if options.get("doctype"):
        args += ["-a", f"doctype={options['doctype']}"]
    return args + output, None


def file_mode(options):
    """Whether an entry writes a file (Ruby's `:to_dir`/`:to_file`)."""
    return bool(options.keys() & OUTPUT_OPTIONS)


def cli_input(source, options):
    """The input of a file-output run: the checkout fixture the recorded
    `docfile` names when it holds exactly this source (like the test's
    convert_file), else None for standard input."""
    if not file_mode(options):
        return None
    docfile = (options.get("attributes") or {}).get("docfile") if isinstance(
        options.get("attributes"), dict) else None
    prefix = f"{PLACEHOLDER}/test/fixtures/"
    if not isinstance(docfile, str) or not docfile.startswith(prefix):
        return None
    path = FIXTURES / docfile[len(prefix):]
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None
    return docfile if text == source else None


def materialize(scratch):
    """A checkout stand-in for both sides: the vendored, unchanged upstream
    fixtures at test/fixtures under a temporary root, which replaces the
    recorded checkout path (the tests' paths, docdir and base_dir)."""
    import shutil
    root = Path(scratch) / "upstream"
    shutil.copytree(FIXTURES, root / "test" / "fixtures")
    return root


TEST_SERVER = ("127.0.0.1", 9876)


class TestServerHandler(socketserver.StreamRequestHandler):
    """Upstream test_helper.rb's `using_test_webserver`, byte for byte: a
    GET of a file under the test directory (`image/<ext>`, `.adoc` and
    extensionless as text/plain), `/name/asciidoctor` as JSON, else 404;
    no Content-Length, the connection closes after each response. Unlike
    Ruby's helper, paths outside the test directory are not served."""

    def handle(self):
        request = self.rfile.readline().decode("latin-1").rstrip("\r\n")
        match = re.fullmatch(r"GET (\S+) HTTP/1\.1", request)
        if not match:
            self.wfile.write(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Type: text/plain\r\n\r\n"
                             b"405 - Method not allowed\n")
            return
        resource = match[1]
        base = self.server.base_dir
        path = Path(os.path.normpath(f"{base}/{resource}"))
        if resource == "/name/asciidoctor":
            self.wfile.write(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n"
                             b'{"name": "asciidoctor"}\n')
        elif path.is_file() and path.is_relative_to(base):
            ext = path.suffix[1:]
            mimetype = ("text/plain" if ext == "adoc" else f"image/{ext}") if ext else "text/plain"
            self.wfile.write(f"HTTP/1.1 200 OK\r\nContent-Type: {mimetype}\r\n\r\n".encode())
            self.wfile.write(path.read_bytes())
        else:
            self.wfile.write(b"HTTP/1.1 404 File Not Found\r\nContent-Type: text/plain\r\n\r\n"
                             b"404 - Resource not found.\n")


@contextmanager
def test_server(root, address=TEST_SERVER):
    """The tests' web server at their fixed address (the recorded sources
    name it), serving `root/test`; yields its URL."""
    socketserver.ThreadingTCPServer.allow_reuse_address = True
    with socketserver.ThreadingTCPServer(address, TestServerHandler) as server:
        server.base_dir = (root / "test").resolve()
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield "http://%s:%d" % server.server_address
        finally:
            server.shutdown()


def localize(value, root):
    """`value` (an entry or CLI arguments) with the checkout placeholder
    replaced by the materialized root."""
    escaped = json.dumps(str(root))[1:-1]
    return json.loads(json.dumps(value, ensure_ascii=False).replace(PLACEHOLDER, escaped))


def fixture_digests(fixtures):
    return {str(f.relative_to(fixtures)): hashlib.sha256(f.read_bytes()).hexdigest()
            for f in sorted(fixtures.rglob("*")) if f.is_file()}


def read_recording(path):
    # Split on LF only: str.splitlines would also split at U+2028 and other
    # separators JSON leaves unescaped inside strings.
    return [json.loads(line) for line in path.read_text(encoding="utf-8").split("\n") if line]


def record(upstream, recording=None):
    """Write the inventory from a recording of the upstream suite. Without a
    recording, run the suite (each test file in its own process) with the
    recorder first; that executes the upstream test code."""
    tests = upstream / "test"
    files = sorted(tests.glob("*_test.rb"))
    if recording:
        rows = read_recording(recording)
    else:
        with tempfile.TemporaryDirectory() as scratch:
            out = Path(scratch) / "record.jsonl"
            for test in files:
                subprocess.run(["ruby", "-r", str(ROOT / "scripts/upstream_record.rb"), str(test)],
                               cwd=upstream, capture_output=True, timeout=1800,
                               env={**os.environ, "ASCIIDOCTOR_TEST_DIR": str(tests), "RECORD_OUT": str(out)})
                print(f"recorded {test.name}", flush=True)
            rows = read_recording(out)
    # The checkout's absolute path is machine-specific: mark it.
    rows = [json.loads(json.dumps(row, ensure_ascii=False).replace(
        json.dumps(str(upstream))[1:-1], "<UPSTREAM>")) for row in rows]
    entries = {}
    for row in rows:
        key = entry_id(row["source"], row["options"])
        entry = entries.setdefault(key, {"id": key, "source": row["source"], "options": row["options"],
                                         "occurrences": []})
        occurrence = {k: row[k] for k in ("file", "test_line", "test", "call_line")}
        if occurrence not in entry["occurrences"]:
            entry["occurrences"].append(occurrence)
    for entry in entries.values():
        args, reason = classify(entry["source"], entry["options"])
        entry["unsupported"] = reason
    inventory = {
        "asciidoctor": (ROOT / "compatibility/asciidoctor-version").read_text().strip(),
        "commit": COMMIT,
        "tests": {f.name: hashlib.sha256(f.read_bytes()).hexdigest() for f in files},
        "fixtures": fixture_digests(tests / "fixtures"),
        "entries": sorted(entries.values(), key=lambda e: (e["occurrences"][0]["file"],
                                                           e["occurrences"][0]["call_line"], e["id"])),
    }
    INVENTORY.parent.mkdir(parents=True, exist_ok=True)
    INVENTORY.write_text(json.dumps(inventory, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"{len(rows)} documents, {len(entries)} entries -> {INVENTORY}")


def body(html, standalone):
    result = projection(html)
    if standalone:
        # Only the three document shell tags differ (as in the asset audit);
        # their other attributes (a `max-width` style) are compared.
        for node in result:
            if isinstance(node, dict) and (node["tag"], node["attrs"].get("id")) in [
                    ("header", "header"), ("main", "content"), ("footer", "footer")]:
                node["tag"] = "div"
    return result


HTML_TAGS = set("""a abbr address area article aside audio b base bdi bdo blockquote body br button
canvas caption cite code col colgroup data datalist dd del details dfn dialog div dl dt em embed
fieldset figcaption figure footer form h1 h2 h3 h4 h5 h6 head header hr html i iframe img input ins
kbd label legend li link main map mark meta meter nav noscript object ol optgroup option output p
param picture pre progress q rp rt ruby s samp script section select small source span strong
style sub summary sup svg table tbody td template textarea tfoot th thead time title tr track u ul
var video wbr math""".split())
TAG = re.compile(r"</?([A-Za-z][\w:-]*)[^<>]*>")


def neutralize(html):
    """Raw passthrough markup that is no HTML element (`<html5>`) as text,
    the same on both sides, so the projection can compare the rest."""
    return TAG.sub(lambda m: m[0] if m[1].lower() in HTML_TAGS
                   else m[0].replace("<", "&lt;").replace(">", "&gt;"), html)


def raw_body(html):
    """The text between <body> and </body> (or the whole fragment), with
    whitespace collapsed: the comparison for HTML the projection rejects."""
    start, end = html.find("<body"), html.rfind("</body>")
    if start != -1 and end != -1:
        html = html[html.index(">", start) + 1:end]
    return " ".join(html.split())


def decoded(run):
    """Output read as bytes, not in text mode: universal newlines would turn
    a lone CR of adoctra's HTML into LF, while Ruby's JSON keeps it."""
    return subprocess.CompletedProcess(run.args, run.returncode, run.stdout.decode("utf-8", "replace"),
                                       run.stderr.decode("utf-8", "replace"))


def adoctra(binary, entry, args, root=None):
    run = decoded(subprocess.run([str(binary), "-", "--to", "html", "--stdout", *args],
                                 input=entry["source"].encode("utf-8"), capture_output=True, timeout=60,
                                 env={**os.environ, **ENVIRONMENT}))
    if run.returncode:
        return {"error": f"exit {run.returncode}: {run.stderr.strip()[-300:]}"}
    try:
        result = {"html": run.stdout, "diagnostics": cli_diagnostics(run.stderr, root)}
    except ValueError as error:
        return {"error": str(error)}
    # The same conversion as JSON: block structure and attribute snapshots.
    model = decoded(subprocess.run([str(binary), "-", *MODEL_OPTIONS, "--stdout", *args],
                                   input=entry["source"].encode("utf-8"), capture_output=True, timeout=60,
                                   env={**os.environ, **ENVIRONMENT}))
    result["model"] = adoctra_model(model.stdout, root)
    return result


# The JSON options that export the model (with the `--to json` conversion).
MODEL_OPTIONS = ("--to", "json", "--json-attributes", "--json-locations", "--json-references")


def adoctra_model(text, root=None):
    """The model from adoctra's JSON: the block structure, the attribute
    snapshots, the registered ids and the footnotes; an error entry when the
    JSON is missing or incomplete."""
    try:
        document = json.loads(text)
        return {"structure": structure.document_projection(
                    document, lambda block: adoctra_metadata(block, root)[1]),
                "references": document["references"],
                "footnotes": [{"index": entry["index"], "id": entry["id"], "text": parts_text(entry["parts"])}
                              for entry in document.get("footnotes", [])],
                "attributes": {snapshot: model_attributes(values, root)
                               for snapshot, values in document["attribute_snapshots"].items()}}
    except (ValueError, KeyError, TypeError) as error:
        return {"error": f"{type(error).__name__}: {error}"[:300]}


def tree_state(root):
    """Every file under the checkout with its contents."""
    return {path.relative_to(root).as_posix(): path.read_bytes()
            for path in root.rglob("*") if path.is_file()}


@contextmanager
def recording_writes(root):
    """Yields a dict that receives the files a run created or changed under
    `root` (relative path -> bytes); afterwards the tree is restored, so
    every run starts from the same checkout."""
    before = tree_state(root)
    dirs = {path for path in root.rglob("*") if path.is_dir()}
    written = {}
    try:
        yield written
    finally:
        after = tree_state(root)
        written.update({name: data for name, data in after.items() if before.get(name) != data})
        for name in after:
            if name not in before:
                (root / name).unlink()
            elif before[name] != after[name]:
                (root / name).write_bytes(before[name])
        for path in sorted((p for p in root.rglob("*") if p.is_dir() and p not in dirs), reverse=True):
            path.rmdir()


def run_cli(binary, entry, args, root):
    """One file-output entry on both sides, as a CLI run in the checkout:
    Ruby's Asciidoctor::Cli::Invoker (the reference script's `--invoke`)
    and adoctra, each with the same arguments and input, from a pristine
    tree. Returns (expected, actual) with exit code, error text, stdout,
    diagnostics and written files."""
    input_file = cli_input(entry["source"], entry["options"])
    target = localize(input_file, root) if input_file else "-"
    stdin = None if input_file else entry["source"]
    env = {**os.environ, **ENVIRONMENT}
    with recording_writes(root) as files:
        run = subprocess.run(["ruby", str(ROOT / "scripts/upstream-audit-reference.rb"), "--invoke", *args,
                              target], input=stdin or "", cwd=root, capture_output=True, text=True,
                             encoding="utf-8", errors="replace", env=env, timeout=120)
    if run.returncode:
        expected = {"error": f"reference runner failed: {run.stderr.strip()[-300:]}"}
    else:
        result = json.loads(run.stdout)
        expected = {"code": result["code"], "stdout": result["stdout"],
                    "failure": result["error"].split("\n")[0],
                    "diagnostics": reference_diagnostics(result["diagnostics"], root), "files": files,
                    "models": [reference_model(model, root, input_file and localize(input_file, root))
                               for model in result["models"]]}
    with recording_writes(root) as files:
        run = decoded(subprocess.run([str(binary), target, "--to", "html", *args],
                                     input=(stdin or "").encode("utf-8"), cwd=root,
                                     capture_output=True, env=env, timeout=60))
    stderr = run.stderr
    failure = ""
    if run.returncode:
        lines = stderr.removesuffix("\n").split("\n")
        failure = lines.pop().removeprefix("adoctra: ")
        stderr = "\n".join(lines) + ("\n" if lines else "")
    try:
        diagnostics = cli_diagnostics(stderr, root)
    except ValueError as error:
        return expected, {"error": str(error)}
    actual = {"code": run.returncode, "stdout": run.stdout, "failure": failure,
              "diagnostics": diagnostics, "files": files,
              # The HTML run converted nothing (it failed before writing, as
              # Ruby's invoker then keeps no document): no model either.
              "models": written_models(binary, target, stdin, args, root, env)
              if run.stdout or files else []}
    return expected, actual


def written_models(binary, target, stdin, args, root, env):
    """The model of the converted document: the same run as JSON (written to
    the same place, so `outfile` and `outdir` stay the same), read from
    standard output or the JSON file it wrote; none when the run wrote
    nothing."""
    with recording_writes(root) as files:
        run = decoded(subprocess.run([str(binary), target, *MODEL_OPTIONS, *args],
                                     input=(stdin or "").encode("utf-8"), cwd=root,
                                     capture_output=True, env=env, timeout=60))
    candidates = [run.stdout] if run.stdout else [
        data.decode("utf-8", "replace") for name, data in sorted(files.items()) if not name.endswith(".css")]
    return [adoctra_model(text, root) for text in candidates]


# Ruby's default stylesheet and adoctra's own, written by `copycss`: the
# names and contents differ by design, so only their presence is compared.
DEFAULT_STYLESHEETS = ("asciidoctor.css", "adoctra.css")


def compare_files(entry, args, expected, actual):
    if "error" in expected:
        return "RUBY_ERROR", [expected["error"]]
    if "error" in actual:
        return "ADOCTRA_ERROR", [actual["error"]]
    standalone = "-s" not in args
    reasons = []
    for key in ("code", "failure", "diagnostics"):
        if expected[key] != actual[key]:
            reasons.append(key)
    if len(expected["models"]) != len(actual["models"]):
        reasons.append(f"models:{len(expected['models'])} in Ruby, {len(actual['models'])} in adoctra")
    for theirs, ours in zip(expected["models"], actual["models"]):
        reasons += compare_model(theirs, ours)

    def page(html):
        try:
            return body(html, standalone), shell(html) if standalone else None
        except ValueError:
            return raw_body(html)
    if bool(expected["stdout"]) != bool(actual["stdout"]) or (
            expected["stdout"] and page(expected["stdout"]) != page(actual["stdout"])):
        reasons.append("stdout")

    def files(side):
        result = {}
        for name, data in side["files"].items():
            directory, _, base = name.rpartition("/")
            if base in DEFAULT_STYLESHEETS:
                result[f"{directory}/<default stylesheet>"] = None
            else:
                result[name] = data
        return result
    expected_files, actual_files = files(expected), files(actual)
    if expected_files.keys() != actual_files.keys():
        reasons.append("files:" + ",".join(sorted(expected_files.keys() ^ actual_files.keys())))
    for name in sorted(expected_files.keys() & actual_files.keys()):
        ours, theirs = actual_files[name], expected_files[name]
        if theirs is None or ours == theirs:
            continue
        if name.endswith(".css"):
            reasons.append(f"file:{name}")
            continue
        text = [data.decode("utf-8", "replace") for data in (theirs, ours)]
        if text[0].endswith("\n") != text[1].endswith("\n") or page(text[0]) != page(text[1]):
            reasons.append(f"file:{name}")
    return ("DIFF" if reasons else "PASS"), reasons


def compare(entry, expected, actual):
    if "error" in expected:
        return "RUBY_ERROR", [expected["error"]]
    if "error" in actual:
        return "ADOCTRA_ERROR", [actual["error"]]
    standalone = bool(entry["options"].get("standalone") or entry["options"].get("header_footer"))
    reasons = []
    projected = []
    for side in (expected, actual):
        try:
            projected.append(body(side["html"], standalone))
        except ValueError as error:
            projected.append(error)
    if any(isinstance(p, ValueError) for p in projected):
        try:
            projected = [body(neutralize(side["html"]), standalone) for side in (expected, actual)]
        except ValueError:
            pass
    if any(isinstance(p, ValueError) for p in projected):
        # Malformed HTML (raw passthrough markup): the projection is
        # undefined, so the documents must be byte-identical bodies instead.
        if not all(isinstance(p, ValueError) for p in projected):
            side = "reference" if isinstance(projected[0], ValueError) else "adoctra"
            return "DIFF", [f"projection:{side}:{projected[0] if side == 'reference' else projected[1]}"]
        if raw_body(expected["html"]) != raw_body(actual["html"]):
            reasons.append("raw-body")
    elif projected[0] != projected[1]:
        reasons.append("body")
    if standalone and not any(isinstance(p, ValueError) for p in projected) \
            and shell(expected["html"]) != shell(actual["html"]):
        reasons.append("shell")
    if expected["diagnostics"] != actual["diagnostics"]:
        reasons.append("diagnostics")
    reasons += compare_model(expected.get("model"), actual.get("model"))
    return ("DIFF" if reasons else "PASS"), reasons


def compare_model(expected, actual):
    """The block structure and the header/final attribute snapshots."""
    if expected is None and actual is None:
        return []
    if expected is None or actual is None or "error" in expected or "error" in actual:
        side = "reference" if expected is None or "error" in expected else "adoctra"
        detail = (expected if side == "reference" else actual) or {}
        return [f"model:{side}:{detail.get('error', 'missing')}"]
    reasons = []
    if structure.strip_meta(expected["structure"]) != structure.strip_meta(actual["structure"]):
        reasons.append("structure")
    else:
        differences = metadata_differences(expected["structure"], actual["structure"])
        if differences:
            reasons.append("metadata:" + ";".join(differences[:3]) + ("…" if len(differences) > 3 else ""))
    if expected.get("references") != actual.get("references"):
        reasons.append("references")
    if expected.get("footnotes") != actual.get("footnotes"):
        reasons.append("footnotes")
    for snapshot in ("header", "final"):
        ours, theirs = actual["attributes"].get(snapshot, {}), expected["attributes"].get(snapshot, {})
        changed = sorted(name for name in set(ours) | set(theirs) if ours.get(name) != theirs.get(name))
        if changed:
            reasons.append(f"attributes:{snapshot}:" + ",".join(changed[:5]) + ("…" if len(changed) > 5 else ""))
    return reasons


def load_inventory(path=INVENTORY, sources=SOURCES):
    """The inventory, after checking its provenance against the vendored,
    unchanged upstream test files: checksums and each test declaration."""
    inventory = json.loads(path.read_text(encoding="utf-8"))
    if inventory.get("commit") != COMMIT or inventory.get("asciidoctor") != (
            ROOT / "compatibility/asciidoctor-version").read_text().strip():
        raise ValueError("inventory commit/version mismatch")
    lines = {}
    for name, digest in inventory["tests"].items():
        data = (sources / name).read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise ValueError(f"upstream test checksum mismatch: {name}")
        lines[name] = data.decode("utf-8").split("\n")
    if fixture_digests(sources / "fixtures") != inventory["fixtures"]:
        raise ValueError("upstream fixture checksum mismatch")
    ids = set()
    for entry in inventory["entries"]:
        if entry["id"] in ids or entry["id"] != entry_id(entry["source"], entry["options"]):
            raise ValueError(f"invalid or duplicate entry id: {entry['id']}")
        ids.add(entry["id"])
        if entry["unsupported"] != classify(entry["source"], entry["options"])[1]:
            raise ValueError(f"stale classification: {entry['id']}")
        for occurrence in entry["occurrences"]:
            file, line, test = occurrence["file"], occurrence["test_line"], occurrence["test"]
            if not 0 < occurrence["call_line"] <= len(lines[file]):
                raise ValueError(f"call line outside {file}: {occurrence['call_line']}")
            if line is None and test is None:
                continue  # a call from helper code outside any test block
            declaration = lines[file][line - 1].strip() if line else ""
            if declaration not in (f"test '{test}' do", f'test "{test}" do'):
                raise ValueError(f"test declaration mismatch: {file}:{line} {test!r}")
    return inventory


def audit(binary, only, jobs):
    inventory = load_inventory()
    entries = [e for e in inventory["entries"] if not only or e["id"] in only]
    supported = [e for e in entries if not e["unsupported"] and not file_mode(e["options"])]
    writing = [e for e in entries if not e["unsupported"] and file_mode(e["options"])]
    with tempfile.TemporaryDirectory() as scratch:
        root = materialize(scratch)
        local = [localize({k: e[k] for k in ("id", "source", "options")}, root) for e in supported]
        batch = Path(scratch) / "entries.json"
        batch.write_text(json.dumps(local))
        with test_server(root):
            run = subprocess.run(["ruby", str(ROOT / "scripts/upstream-audit-reference.rb"), str(batch)],
                                 capture_output=True, text=True, encoding="utf-8", errors="replace",
                                 env={**os.environ, **ENVIRONMENT}, timeout=3600)
            if run.returncode:
                sys.exit(f"reference runner failed: {run.stderr.strip()[-2000:]}")
            references = {r["id"]: r for r in map(json.loads, filter(None, run.stdout.split("\n")))}
            docfiles = {e["id"]: e["options"].get("attributes", {}).get("docfile") for e in local}
            for reference in references.values():
                if "diagnostics" in reference:
                    reference["diagnostics"] = reference_diagnostics(reference["diagnostics"], root)
                if "model" in reference:
                    reference["model"] = reference_model(reference["model"], root, docfiles.get(reference["id"]))
            with ThreadPoolExecutor(jobs) as pool:
                actuals = dict(zip((e["id"] for e in local), pool.map(
                    lambda e: adoctra(binary, e, localize(classify(e["source"], e["options"])[0], root), root),
                    local)))
            # File output changes the checkout: one entry at a time.
            written = {}
            for entry in writing:
                args = localize(classify(entry["source"], entry["options"])[0], root)
                written[entry["id"]] = compare_files(entry, args, *run_cli(binary, entry, args, root))
    results = {}
    for entry in entries:
        if entry["unsupported"]:
            results[entry["id"]] = {"status": "UNSUPPORTED", "reasons": [entry["unsupported"]]}
            continue
        if entry["id"] in written:
            status, reasons = written[entry["id"]]
            results[entry["id"]] = {"status": status, "reasons": reasons}
            continue
        expected = references.get(entry["id"], {"error": "no reference output"})
        status, reasons = compare(entry, expected, actuals[entry["id"]])
        results[entry["id"]] = {"status": status, "reasons": reasons,
                                "locations": compare_locations(expected.get("model"), actuals[entry["id"]].get("model"))}
    return inventory, entries, results


def show(entry, binary):
    """Print both sides of one entry: diagnostics and a projection diff."""
    import difflib
    with tempfile.TemporaryDirectory() as scratch:
        root = materialize(scratch)
        local = localize({k: entry[k] for k in ("id", "source", "options")}, root)
        args = localize(classify(entry["source"], entry["options"])[0], root)
        if file_mode(entry["options"]):
            with test_server(root):
                expected, actual = run_cli(binary, entry, args, root)
            print("options:", json.dumps(entry["options"]), "cli:", " ".join(args),
                  "input:", cli_input(entry["source"], entry["options"]) or "-")
            print("status:", compare_files(entry, args, expected, actual))
            for name, side in (("ruby", expected), ("adoctra", actual)):
                summary = {k: v for k, v in side.items() if k != "files"}
                summary["files"] = {n: d.decode("utf-8", "replace")[-200:] for n, d in side.get("files", {}).items()}
                print(f"--- {name}:", json.dumps(summary, indent=1, ensure_ascii=False))
            return
        batch = Path(scratch) / "entries.json"
        batch.write_text(json.dumps([local]))
        with test_server(root):
            run = subprocess.run(["ruby", str(ROOT / "scripts/upstream-audit-reference.rb"), str(batch)],
                                 capture_output=True, text=True, env={**os.environ, **ENVIRONMENT})
            expected = json.loads(run.stdout.split("\n")[0])
            if "diagnostics" in expected:
                expected["diagnostics"] = reference_diagnostics(expected["diagnostics"], root)
            if "model" in expected:
                expected["model"] = reference_model(expected["model"], root,
                                                    local["options"].get("attributes", {}).get("docfile"))
            actual = adoctra(binary, local, args, root)
    for occurrence in entry["occurrences"]:
        print(f"{occurrence['file']}:{occurrence['test_line']} {occurrence['test']}")
    print("options:", json.dumps(entry["options"]), "cli:", " ".join(args))
    print("--- source\n" + entry["source"])
    standalone = bool(entry["options"].get("standalone") or entry["options"].get("header_footer"))
    for name, side in (("ruby", expected), ("adoctra", actual)):
        print(f"--- {name} diagnostics:", json.dumps(side.get("diagnostics", side.get("error"))))
    pretty = [json.dumps({"body": body(side["html"], standalone),
                          "shell": shell(side["html"]) if standalone else None},
                         indent=1, ensure_ascii=False).splitlines()
              for side in (expected, actual)]
    print("\n".join(difflib.unified_diff(*pretty, "ruby", "adoctra", lineterm="")))


def summary(entries, results):
    by_file = defaultdict(Counter)
    for entry in entries:
        by_file[entry["occurrences"][0]["file"]][results[entry["id"]]["status"]] += 1
    return {name: dict(sorted(counts.items())) for name, counts in sorted(by_file.items())}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--record", type=Path, metavar="UPSTREAM",
                        help="record the inventory from an upstream checkout of the pinned commit")
    parser.add_argument("--recording", type=Path,
                        help="with --record: build the inventory from this recorder JSONL instead of running the suite")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/adoctra")
    parser.add_argument("--report", type=Path, help="write statuses and per-file counts")
    parser.add_argument("--baseline", type=Path, help="require every status to equal this report's")
    parser.add_argument("--only", action="append", default=[], help="entry id (repeatable)")
    parser.add_argument("--jobs", type=int, default=8)
    parser.add_argument("--show", metavar="ID", help="print both sides of one entry")
    args = parser.parse_args()
    if args.record:
        record(args.record.resolve(), args.recording)
        return 0
    if args.show:
        entry = next(e for e in load_inventory()["entries"] if e["id"] == args.show)
        show(entry, args.binary)
        return 0
    if args.only and args.baseline:
        parser.error("--only cannot be combined with --baseline")
    inventory, entries, results = audit(args.binary, set(args.only), args.jobs)
    files = summary(entries, results)
    totals = dict(sorted(Counter(r["status"] for r in results.values()).items()))
    locations = dict(sorted(Counter(r["locations"] for r in results.values() if r.get("locations")).items()))
    for name, counts in files.items():
        print(f"{name:32} " + " ".join(f"{k}={v}" for k, v in counts.items()))
    print(json.dumps(totals))
    print("locations:", json.dumps(locations))
    report = {"profile": PROFILE,
              "asciidoctor": inventory["asciidoctor"], "commit": inventory["commit"],
              "totals": totals, "location_totals": locations, "files": files,
              "results": dict(sorted(results.items()))}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    if args.baseline:
        expected = json.loads(args.baseline.read_text(encoding="utf-8"))["results"]
        changed = [(key, expected.get(key, {}).get("status"), value["status"])
                   for key, value in results.items() if expected.get(key) != value]
        changed += [(key, value["status"], None) for key, value in expected.items() if key not in results]
        for key, before, after in changed:
            print(f"CHANGED {key}: {before} -> {after}")
        if changed:
            return 1
        print("baseline: no change")
    return 0


if __name__ == "__main__":
    sys.exit(main())
