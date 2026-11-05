#!/usr/bin/env python3
"""Diagram profile `diagram-v1`: `adoctra -r asciidoctor-diagram` against
Ruby's `asciidoctor -r asciidoctor-diagram` (the pinned asciidoctor-diagram,
compatibility/asciidoctor-diagram-version) for the scenarios in
compatibility/diagram/cases.json.

Each case materializes its files in two fresh directories and runs both
commands there in safe mode (`-S safe`, adoctra's default) with embedded
output to `out.html`. Compared:

* the HTML in the body-dom-v1 projection (adoctra's modern markup mapped);
* the diagnostics as (severity, file, line, message), in order;
* every file the run created (images, `.asciidoctor/diagram` metadata),
  byte for byte;
* the requests a diagram server received (a local fake server: method,
  path, content type and decoded code).

Both sides use the same tools: Graphviz `dot` on PATH, PlantUML from the
asciidoctor-diagram-plantuml gem's JARs (`DIAGRAM_PLANTUML_CLASSPATH`, which
adoctra runs through `java`), and mermaid-cli given with `--mmdc` (a path
substituted for `{mmdc}`). A case whose tool is missing is SKIP, never PASS.
A Ruby failure (non-zero exit) is RUBY_ERROR unless the case freezes it as
XFAIL with adoctra's complete actual result.
"""

import argparse
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parent))
from compatibility import projection  # noqa: E402
from cli_matrix import stderr_entries  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "compatibility" / "diagram"
CASES = CORPUS / "cases.json"
REPORT = CORPUS / "report.json"
PROFILE = "diagram-v1"

SERVER_SVG = (b'<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30">'
              b'<rect width="40" height="30"/></svg>')
PLANTUML_ALPHABET = str.maketrans(
    "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789")


def decode_request(path):
    """The code in a GET path: `/puml/...` is a PlantUML server (raw deflate,
    PlantUML's alphabet), anything else Kroki (zlib)."""
    data = path.rsplit("/", 1)[-1]
    if path.split("/")[1] == "puml":
        raw = base64.urlsafe_b64decode(data.translate(PLANTUML_ALPHABET))
        return zlib.decompress(raw, -15).decode("utf-8")
    return zlib.decompress(base64.urlsafe_b64decode(data)).decode("utf-8")


class FakeServer:
    """A diagram server answering every request with one SVG (or 400 under
    `/fail/`) and recording what it received."""

    def __init__(self):
        self.log = []
        server = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def reply(self, entry):
                server.log.append(entry)
                if "/fail/" in self.path:
                    self.send_response(400)
                    self.end_headers()
                    return
                self.send_response(200)
                self.send_header("Content-Type", "image/svg+xml")
                self.end_headers()
                self.wfile.write(SERVER_SVG)

            def do_GET(self):
                prefix = self.path.rsplit("/", 1)[0]
                try:
                    code = decode_request(self.path)
                except Exception as error:  # a malformed request is data too
                    code = f"<undecodable: {error}>"
                self.reply({"method": "GET", "path": prefix, "code": code})

            def do_POST(self):
                length = int(self.headers.get("Content-Length", 0))
                code = self.rfile.read(length).decode("utf-8")
                self.reply({"method": "POST", "path": self.path,
                            "type": self.headers.get("Content-Type"), "code": code})

        self.httpd = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.httpd.server_address[1]}"
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    def take(self):
        log, self.log = self.log, []
        return log

    def close(self):
        self.httpd.shutdown()


def plantuml_classpath():
    """The asciidoctor-diagram-plantuml and -batik JARs Ruby loads."""
    jars = []
    for gem in ("asciidoctor-diagram-plantuml", "asciidoctor-diagram-batik"):
        result = subprocess.run(["gem", "contents", gem], capture_output=True, text=True)
        jars += sorted(line for line in result.stdout.splitlines() if line.endswith(".jar"))
    return ":".join(jars)


def available(tools, args):
    missing = []
    for tool in tools:
        if tool == "mmdc":
            ok = bool(args.mmdc)
        elif tool == "plantuml":
            ok = bool(args.classpath) and shutil.which("java") is not None
        elif tool == "server":
            ok = True
        else:
            ok = shutil.which(tool) is not None
        if not ok:
            missing.append(tool)
    return missing


def files_view(root, inputs):
    view = {}
    for path in sorted(root.rglob("*")):
        name = path.relative_to(root).as_posix()
        if path.is_file() and name not in inputs and name != "out.html":
            view[name] = hashlib.sha256(path.read_bytes()).hexdigest()
    return view


def run_side(command, case, root, env, server):
    source = case["source"].replace("{server}", server.url).replace("{mmdc}", env["MMDC"])
    files = {"d.adoc": source, **case.get("files", {})}
    root.mkdir(parents=True)
    for name, content in files.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text(content, encoding="utf-8")
    server.take()
    result = subprocess.run(command + ["-S", "safe", "-s", "-o", "out.html", *case.get("args", []), "d.adoc"],
                            cwd=root, capture_output=True, env=env, timeout=300)
    out = root / "out.html"
    html = out.read_text(encoding="utf-8") if out.exists() else None
    return {
        "code": result.returncode,
        # Ruby writes an inline macro's text unescaped (an error message
        # with `<stdin>`): both sides get a browser's tree.
        "html": projection(html, lenient=True) if html is not None else None,
        "diagnostics": stderr_entries(result.stderr.decode("utf-8", "replace"),
                                      command[0] == "asciidoctor"),
        "files": files_view(root, files),
        "requests": server.take(),
    }


def check(case, args, server):
    missing = available(case.get("requires", []), args)
    if missing:
        return "SKIP", {"missing": missing}
    env = {**os.environ, "DIAGRAM_PLANTUML_CLASSPATH": args.classpath, "MMDC": args.mmdc or ""}
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        ruby = run_side(["asciidoctor", "-r", "asciidoctor-diagram"], case, scratch / "r", env, server)
        ours = run_side([str(args.binary), "-r", "asciidoctor-diagram"], case, scratch / "o", env, server)
    # Ruby's absolute fixture paths differ only by the side's directory.
    text = json.dumps(ours).replace(str(scratch / "o"), "<root>")
    ours = json.loads(text)
    ruby = json.loads(json.dumps(ruby).replace(str(scratch / "r"), "<root>"))
    xfail = case.get("xfail")
    if ruby["code"] != 0 and not xfail:
        return "RUBY_ERROR", {"ruby": ruby, "adoctra": ours}
    if xfail:
        if ruby == ours:
            return "XPASS", {"adoctra": ours}
        return ("XFAIL", {}) if ours == xfail["actual"] else ("DRIFT", {"adoctra": ours})
    return ("PASS", {}) if ruby == ours else ("DIFF", {"ruby": ruby, "adoctra": ours})


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--binary", type=Path, default=ROOT / "target" / "debug" / "adoctra")
    parser.add_argument("--mmdc", default=os.environ.get("ADOCTRA_MMDC", ""),
                        help="mermaid-cli executable (cases needing it are SKIP without)")
    parser.add_argument("--case", action="append", help="run only these cases")
    parser.add_argument("--freeze", action="store_true",
                        help="store adoctra's actual result for the XFAIL cases")
    parser.add_argument("--report", action="store_true", help=f"write {REPORT.relative_to(ROOT)}")
    args = parser.parse_args(argv)
    args.classpath = plantuml_classpath()
    manifest = json.loads(CASES.read_text(encoding="utf-8"))
    if manifest["profile"] != PROFILE or manifest["asciidoctor_diagram"] != \
            (ROOT / "compatibility" / "asciidoctor-diagram-version").read_text().strip():
        raise SystemExit("diagram manifest profile/version mismatch")
    server = FakeServer()
    statuses = {}
    failed = False
    try:
        for case in manifest["cases"]:
            if args.case and case["name"] not in args.case:
                continue
            status, details = check(case, args, server)
            if args.freeze and status in {"XFAIL", "DRIFT"}:
                case["xfail"]["actual"] = details.get("adoctra", case["xfail"]["actual"])
                status = "XFAIL"
            statuses[case["name"]] = status
            print(f"{status:10} {case['name']}")
            if status in {"DIFF", "DRIFT", "XPASS", "RUBY_ERROR"}:
                failed = True
                print(json.dumps(details, ensure_ascii=False, indent=1))
    finally:
        server.close()
    if args.freeze:
        CASES.write_text(json.dumps(manifest, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    summary = {}
    for status in statuses.values():
        summary[status] = summary.get(status, 0) + 1
    print(" ".join(f"{k}={v}" for k, v in sorted(summary.items())))
    if args.report and not args.case:
        REPORT.write_text(json.dumps({"profile": PROFILE, "asciidoctor_diagram": manifest["asciidoctor_diagram"],
                                      "summary": dict(sorted(summary.items())), "cases": statuses},
                                     ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
