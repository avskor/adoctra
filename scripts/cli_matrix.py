#!/usr/bin/env python3
"""CLI matrix: the observable behaviour of the `adoctra` command against
Ruby's `asciidoctor` command (scripts/ruby-cli.rb, the pinned gem) for the
scenarios in compatibility/cli/cases.json.

Each case materializes its files in a fresh directory and runs both commands
there with the same arguments (and standard input). Compared, in profile
`cli-v1`:

* the exit status;
* standard output: HTML in the body-dom-v1 projection (standalone shells
  mapped), Ruby's usage/version text and adoctra's own by kind only;
* standard error as (kind, file basename, line, text) entries: logged
  messages by severity, other lines by text, without the program name and
  Ruby's `Use --trace` hint, and the timings report without its numbers;
* every file the run created or changed (HTML projected plus its final LF,
  default stylesheets by presence, other files byte for byte).

A Ruby crash (an uncaught exception with a backtrace) is RUBY_ERROR.

adoctra's own presentation (program name, `file:line: severity:` instead of
Ruby's `SEVERITY: file: line N:`) and its intentional defaults (safe mode,
modern HTML) are normalized or kept out of the scenarios; everything else
must match. A changed status fails `--baseline`.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from upstream_audit import ENVIRONMENT, body, raw_body, recording_writes, DEFAULT_STYLESHEETS  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
CASES = ROOT / "compatibility" / "cli" / "cases.json"
PROFILE = "cli-v1"

RUBY_LOG = re.compile(r"asciidoctor: (INFO|WARNING|ERROR|FATAL|DEBUG): (?:(.+?): line (\d+): )?(.*)", re.S)
OURS_LOG = re.compile(r"adoctra: (?:(.+?):(\d+): )?(info|warning|error|fatal): (.*)", re.S)
TIMING = re.compile(r"(Input file: |\s+Time to [^:]+: |\s+Total time [^:]+: )(.*)")


def run(command, case, cwd):
    # `stdin_hex` feeds bytes that are not UTF-8 text (a UTF-16 BOM, bad bytes).
    stdin = bytes.fromhex(case["stdin_hex"]) if "stdin_hex" in case else (case.get("stdin") or "").encode()
    with recording_writes(cwd) as files:
        result = subprocess.run(command + case["args"], input=stdin, cwd=cwd,
                                capture_output=True, env={**os.environ, **ENVIRONMENT}, timeout=120)
    for name in [*case.get("files", {}), *case.get("binary_files", {})]:
        files.pop(name, None)
    return {"code": result.returncode, "stdout": result.stdout.decode("utf-8", "replace"),
            "stderr": result.stderr.decode("utf-8", "replace"), "files": files}


def stderr_entries(text, ruby):
    entries = []
    # The usage text itself differs by design.
    usage = "Usage: asciidoctor" if ruby else "Usage: adoctra"
    if text.startswith(usage):
        return [["usage"]]
    for line in text.split("\n"):
        if not line or line.strip() == "Use --trace to show backtrace":
            continue
        if match := TIMING.fullmatch(line):
            entries.append(["timing", match[1].strip()])
            continue
        if ruby and (match := RUBY_LOG.fullmatch(line)):
            severity, file, number, text = match[1].lower(), match[2], match[3], match[4]
            severity = "warning" if severity == "warning" else severity
        elif not ruby and (match := OURS_LOG.fullmatch(line)):
            file, number, severity, text = match[1], match[2], match[3], match[4]
        else:
            entries.append(["message", re.sub(r"^(asciidoctor|adoctra): ", "", line)])
            continue
        if file in ("-", "<stdin>"):
            file = "<stdin>"
        elif file:
            file = file.rsplit("/", 1)[-1]
        entries.append([severity, file, int(number) if number else None, text])
    return entries


def stdout_view(text, ruby):
    if not text:
        return ""
    if text.startswith("Usage: asciidoctor" if ruby else "Usage: adoctra"):
        return "<usage>"
    if (ruby and re.match(r"Asciidoctor \d", text)) or (not ruby and re.match(r"adoctra \d", text)):
        return "<version>"
    try:
        return body(text, "<html" in text)
    except ValueError:
        return raw_body(text)


def html_content(data):
    """Converted output under a name without an HTML extension (`-o dir/`,
    `outfilesuffix=.xml`): UTF-8 text starting with markup. Other files
    (stylesheets, images) still compare byte for byte."""
    try:
        return data.decode("utf-8").lstrip().startswith("<")
    except UnicodeDecodeError:
        return False


def files_view(files):
    view = {}
    for name, data in files.items():
        directory, _, base = name.rpartition("/")
        if base in DEFAULT_STYLESHEETS:
            view[f"{directory}/<default stylesheet>"] = None
        elif base.endswith((".html", ".htm", ".xhtml")) or html_content(data):
            text = data.decode("utf-8", "replace")
            view[name] = [stdout_view(text, False) if not text.startswith("Usage") else text,
                          text.endswith("\n")]
        else:
            view[name] = data.hex()
    return view


def project(side, ruby):
    return {"code": side["code"], "stdout": stdout_view(side["stdout"], ruby),
            "stderr": stderr_entries(side["stderr"], ruby), "files": files_view(side["files"])}


def run_case(case, binary):
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch) / "case"
        for name, content in case.get("files", {}).items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        for name, encoded in case.get("binary_files", {}).items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(bytes.fromhex(encoded))
        root.mkdir(exist_ok=True)
        for name in case.get("dirs", []):
            (root / name).mkdir(parents=True, exist_ok=True)
        reference = run(["ruby", str(ROOT / "scripts/ruby-cli.rb")], case, root)
        expected = project(reference, True)
        actual = project(run([str(binary)], case, root), False)
    # An uncaught Ruby exception (a backtrace) is a reference failure, never
    # a PASS.
    if re.search(r"^\tfrom |\(\w+(::\w+)*\)$", reference["stderr"], re.M):
        first = reference["stderr"].split("\n")[0]
        exception = re.search(r": (.*) \((\w+(?:::\w+)*)\)$", first)
        reason = f"{exception[2]}: {exception[1]}" if exception else "ruby exception"
        return "RUBY_ERROR", [reason], expected, actual
    reasons = [key for key in ("code", "stdout", "stderr", "files") if expected[key] != actual[key]]
    return ("DIFF" if reasons else "PASS"), reasons, expected, actual


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/adoctra")
    parser.add_argument("--cases", type=Path, default=CASES)
    parser.add_argument("--case", action="append", default=[], help="run only this case id")
    parser.add_argument("--show", action="store_true", help="print both projections of differing cases")
    parser.add_argument("--report", type=Path)
    parser.add_argument("--baseline", type=Path, help="require every status to equal this report's")
    args = parser.parse_args()
    manifest = json.loads(args.cases.read_text(encoding="utf-8"))
    if manifest.get("profile") != PROFILE:
        sys.exit(f"unexpected profile in {args.cases}")
    ids = [case["id"] for case in manifest["cases"]]
    if len(ids) != len(set(ids)):
        sys.exit("duplicate case ids")
    for case in manifest["cases"]:
        if set(case.get("files", {})) & set(case.get("binary_files", {})):
            sys.exit(f"{case['id']}: text and binary file names overlap")
        if "stdin" in case and "stdin_hex" in case:
            sys.exit(f"{case['id']}: stdin and stdin_hex are exclusive")
    results = {}
    for case in manifest["cases"]:
        if args.case and case["id"] not in args.case:
            continue
        status, reasons, expected, actual = run_case(case, args.binary)
        results[case["id"]] = {"status": status, "reasons": reasons}
        print(f"{status:5} {case['id']}" + (f"  {','.join(reasons)}" if reasons else ""))
        if args.show and status == "DIFF":
            for key in reasons:
                print(f"  ruby    {key}: {json.dumps(expected[key], ensure_ascii=False)[:1500]}")
                print(f"  adoctra {key}: {json.dumps(actual[key], ensure_ascii=False)[:1500]}")
    totals = {}
    for result in results.values():
        totals[result["status"]] = totals.get(result["status"], 0) + 1
    print(json.dumps(dict(sorted(totals.items()))))
    report = {"profile": PROFILE, "totals": dict(sorted(totals.items())), "results": results}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    if args.baseline:
        expected = json.loads(args.baseline.read_text(encoding="utf-8"))["results"]
        changed = [key for key in set(expected) | set(results) if expected.get(key) != results.get(key)]
        for key in sorted(changed):
            print(f"CHANGED {key}: {expected.get(key)} -> {results.get(key)}")
        return 1 if changed else 0
    return 0 if all(r["status"] == "PASS" for r in results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
