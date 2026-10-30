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
the asset audit) and the ordered warning/error texts. Both sides get the same
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

from acceptance import cli_diagnostics
from compatibility import projection

ROOT = Path(__file__).resolve().parent.parent
AUDIT = ROOT / "compatibility" / "upstream" / "audit"
INVENTORY = AUDIT / "inventory.json"
SOURCES = ROOT / "compatibility" / "upstream" / "sources" / "test"
COMMIT = "2c03d22eb9fe83d04ee452939581d1b842f6d8b4"
SUPPORTED_OPTIONS = {"standalone", "header_footer", "parse", "attributes", "safe", "doctype",
                     "input_mtime", "warnings", "failure_level", "linkcss_default", "base_dir"}
FIXTURES = SOURCES / "fixtures"
PLACEHOLDER = "<UPSTREAM>"
SAFE = {"unsafe": "unsafe", "safe": "safe", "server": "server", "secure": "secure",
        0: "unsafe", 1: "safe", 10: "server", 20: "secure"}
ENVIRONMENT = {"SOURCE_DATE_EPOCH": "1700000000", "TZ": "UTC"}


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
    if backend not in (None, "html5", "html"):
        return None, f"backend:{backend}"
    extra = sorted(set(options) - SUPPORTED_OPTIONS - {"backend"})
    if extra:
        return None, "option:" + ",".join(extra)
    safe = SAFE.get(options.get("safe", "secure"))
    if safe is None:
        return None, f"safe:{options.get('safe')}"
    attributes = attribute_args(options.get("attributes"))
    if attributes is None:
        return None, "attributes:non-string"
    backends = [a.split("=", 1)[1] for a in attributes if a.startswith("backend=")]
    # Below server mode the document itself may switch to another backend,
    # unless the host set one (an override wins).
    if not backends and backend is None and safe in ("unsafe", "safe"):
        for value in re.findall(r"^:backend:[ \t]*(\S*)", source, re.MULTILINE):
            if value not in ("html5", "html"):
                return None, f"backend:document:{value}"
    if any(b not in ("html5", "html") for b in backends):
        return None, f"backend:{backends[-1]}"
    args = ["-S", safe]
    if options.get("base_dir"):
        args += ["-B", str(options["base_dir"])]
    if not (options.get("standalone") or options.get("header_footer")):
        args.append("-s")
    if options.get("doctype"):
        args += ["-a", f"doctype={options['doctype']}"]
    for attribute in attributes:
        args += ["-a", attribute]
    return args, None


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


def adoctra(binary, entry, args):
    run = subprocess.run([str(binary), "-", "--to", "html", "--stdout", *args], input=entry["source"],
                         text=True, encoding="utf-8", errors="replace", capture_output=True, timeout=60,
                         env={**os.environ, **ENVIRONMENT})
    if run.returncode:
        return {"error": f"exit {run.returncode}: {run.stderr.strip()[-300:]}"}
    try:
        return {"html": run.stdout, "diagnostics": cli_diagnostics(run.stderr, multiline=True)}
    except ValueError as error:
        return {"error": str(error)}


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
    if expected["diagnostics"] != actual["diagnostics"]:
        reasons.append("diagnostics")
    return ("DIFF" if reasons else "PASS"), reasons


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
    supported = [e for e in entries if not e["unsupported"]]
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
            with ThreadPoolExecutor(jobs) as pool:
                actuals = dict(zip((e["id"] for e in local), pool.map(
                    lambda e: adoctra(binary, e, localize(classify(e["source"], e["options"])[0], root)),
                    local)))
    results = {}
    for entry in entries:
        if entry["unsupported"]:
            results[entry["id"]] = {"status": "UNSUPPORTED", "reasons": [entry["unsupported"]]}
            continue
        expected = references.get(entry["id"], {"error": "no reference output"})
        status, reasons = compare(entry, expected, actuals[entry["id"]])
        results[entry["id"]] = {"status": status, "reasons": reasons}
    return inventory, entries, results


def show(entry, binary):
    """Print both sides of one entry: diagnostics and a projection diff."""
    import difflib
    with tempfile.TemporaryDirectory() as scratch:
        root = materialize(scratch)
        local = localize({k: entry[k] for k in ("id", "source", "options")}, root)
        args = localize(classify(entry["source"], entry["options"])[0], root)
        batch = Path(scratch) / "entries.json"
        batch.write_text(json.dumps([local]))
        with test_server(root):
            run = subprocess.run(["ruby", str(ROOT / "scripts/upstream-audit-reference.rb"), str(batch)],
                                 capture_output=True, text=True, env={**os.environ, **ENVIRONMENT})
            expected = json.loads(run.stdout.split("\n")[0])
            actual = adoctra(binary, local, args)
    for occurrence in entry["occurrences"]:
        print(f"{occurrence['file']}:{occurrence['test_line']} {occurrence['test']}")
    print("options:", json.dumps(entry["options"]), "cli:", " ".join(args))
    print("--- source\n" + entry["source"])
    standalone = bool(entry["options"].get("standalone") or entry["options"].get("header_footer"))
    for name, side in (("ruby", expected), ("adoctra", actual)):
        print(f"--- {name} diagnostics:", json.dumps(side.get("diagnostics", side.get("error"))))
    pretty = [json.dumps(body(side["html"], standalone), indent=1, ensure_ascii=False).splitlines()
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
    for name, counts in files.items():
        print(f"{name:32} " + " ".join(f"{k}={v}" for k, v in counts.items()))
    print(json.dumps(totals))
    report = {"profile": "upstream-audit-body-dom-diagnostics-v1",
              "asciidoctor": inventory["asciidoctor"], "commit": inventory["commit"],
              "totals": totals, "files": files, "results": dict(sorted(results.items()))}
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
