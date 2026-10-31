#!/usr/bin/env python3
"""Deterministic fuzzing of adoctra against the pinned Asciidoctor.

Inputs are generated from a seed: mutations of corpus documents (line edits,
truncation, construct and inline-marker insertion, splices) and families of
deep nesting. Two stages:

* robust — the editor path in one process (`adoctra-wasm` example `fuzz`,
  1 MiB worker stack like wasm32): no panic, no stack overflow, main-source
  spans inside the source on char boundaries, bounded time;
* diff — the CLI (secure, embedded, showtitle) against a persistent Ruby
  worker, compared with the body DOM and diagnostics of the acceptance
  profile. A difference is minimized (line, then character deletion) while
  it persists.

Findings are reported, never baselined here: each confirmed one becomes a
regression test (or a documented known difference) in the regular corpora.
"""
import argparse
import json
import random
import re
import select
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(Path(__file__).resolve().parent))
import acceptance  # noqa: E402
from compatibility import projection  # noqa: E402

PROFILE = "fuzz-v1"

# Block-level constructs inserted as whole lines.
CONSTRUCTS = [
    "----", "-----", "====", "=====", "****", "|===", "|====", "....", "++++", "____", "--", "////",
    "```", "```ruby", "!===", ",===", ":===", "* item", "** item", "*** item", ". item", ".. item",
    "1. item", "a. item", "- item", "term:: def", "term::", "term;; def", "<1> callout", "+",
    "[source,ruby]", "[source]", "[quote]", "[verse]", "[NOTE]", "[TIP]", "[example]", "[sidebar]",
    "[%collapsible]", "[horizontal]", "[qanda]", "[cols=\"1,2\"]", "[%header]", "[#id.role]",
    "[[anchor]]", "[discrete]", "[partintro]", "[bibliography]", "[glossary]", "[appendix]",
    ".Title", ".A *title*", ":attr: value", ":attr!:", ":!attr:", ":icons: font", ":sectnums:",
    ":toc:", ":leveloffset: +1", ":compat-mode:", ":experimental:", ":hardbreaks-option:",
    "ifdef::attr[]", "ifndef::attr[]", "endif::[]", "ifeval::[1 < 2]", "ifdef::attr[inline text]",
    "include::missing.adoc[]", "include::{attr}.adoc[]", "== Section", "=== Sub", "= Title",
    "Title\n=====", "NOTE: note", "WARNING: careful", "image::a.png[]", "video::v.mp4[]",
    "toc::[]", "'''", "<<<", "// comment", "| a | b", "a| cell", "2+| span", "!x!y",
    "stem:[x]", "latexmath:[x^2]", "footnote:[note]", "[subs=+quotes]", "[subs=none]",
    "    indented literal", "> quote", "> > nested", "[.lead]", "[%hardbreaks]", "",
]
# Inline markers inserted inside a line.
MARKERS = [
    "*", "**", "_", "__", "`", "``", "#", "##", "^", "~", "+", "++", "+++", "$$", "[", "]", "{", "}",
    "<<", ">>", "\\", "::", ";;", "|", "!", "<", ">", "&", "&amp;", "((", "))", "(((", ")))", "[[",
    "]]", "{attr}", "{counter:c}", "{set:a:b}", "pass:[", "pass:q[", "link:", "https://e.org",
    "image:", "footnote:[", "kbd:[", "btn:[", "menu:A[", "xref:", "mailto:", "icon:x[", "'", "\"",
    "`'", "'`", "--", "...", "(C)", "->", "=>", " ", "Ж", "🙂", "\t", " +", "<1>", "//",
]
SLOW_NESTING = {"list", "olist"}
SLOW_DEPTH = 128
NESTING = ["example", "sidebar", "quote", "list", "olist", "dlist", "table", "inline", "section",
           "open-in-list", "include-cond"]


def seeds():
    """Small and real documents: the compatibility corpus and golden fixtures."""
    corpus = json.loads((ROOT / "compatibility/cases.json").read_text(encoding="utf-8"))
    out = [case["source"] for case in corpus["cases"] if "resources" not in case]
    out += [p.read_text(encoding="utf-8") for p in sorted((ROOT / "crates/adoctra-html/tests/golden").rglob("*.adoc"))]
    return [s for s in out if s.strip()]


def mutate(rng, source, pool):
    lines = source.split("\n")
    for _ in range(rng.randint(1, 5)):
        op = rng.randrange(10)
        at = rng.randrange(len(lines) + 1)
        if op == 0 and lines:
            del lines[min(at, len(lines) - 1)]
        elif op == 1 and lines:
            i = min(at, len(lines) - 1)
            lines.insert(i, lines[i])
        elif op == 2 and len(lines) > 1:
            i, j = rng.randrange(len(lines)), rng.randrange(len(lines))
            lines[i], lines[j] = lines[j], lines[i]
        elif op in (3, 4):
            lines[at:at] = rng.choice(CONSTRUCTS).split("\n")
        elif op in (5, 6) and lines:
            i = min(at, len(lines) - 1)
            text = lines[i]
            k = rng.randint(0, len(text))
            lines[i] = text[:k] + rng.choice(MARKERS) + text[k:]
        elif op == 7:
            other = rng.choice(pool).split("\n")
            a = rng.randrange(len(other))
            lines[at:at] = other[a:a + rng.randint(1, 8)]
        elif op == 8 and lines:
            i = min(at, len(lines) - 1)
            lines[i] = " " * rng.randint(1, 4) + lines[i]
        elif op == 9:
            lines.insert(at, "")
    text = "\n".join(lines)
    if rng.random() < 0.15 and text:
        text = text[:rng.randrange(len(text) + 1)]
    return text


def nesting(family, depth):
    if family == "example":
        return "\n".join("=" * (4 + i) for i in range(depth)) + "\ncore\n" + "\n".join("=" * (3 + depth - i) for i in range(depth)) + "\n"
    if family == "sidebar":
        return "\n".join("*" * (4 + i) for i in range(depth)) + "\ncore\n" + "\n".join("*" * (3 + depth - i) for i in range(depth)) + "\n"
    if family == "quote":
        return "\n".join("_" * (4 + i) for i in range(depth)) + "\ncore\n" + "\n".join("_" * (3 + depth - i) for i in range(depth)) + "\n"
    if family == "list":
        return "".join("*" * (i + 1) + f" item {i}\n" for i in range(depth))
    if family == "olist":
        return "".join("." * (i + 1) + f" item {i}\n" for i in range(depth))
    if family == "dlist":
        return "".join(f"term {i}" + ":" * (2 + i % 3) + " def\n" if i % 4 != 3 else f"term {i};; def\n" for i in range(depth))
    if family == "table":
        out = "|===\na|\n"
        for i in range(1, depth):
            out += "[cols=1]\n" + ("!===" if i % 2 else "|===") + "\na" + ("!" if i % 2 else "|") + "\n"
        return out + "core\n"
    if family == "inline":
        marks = ["*", "_", "`", "#", "^", "~"]
        return "".join(marks[i % 6] for i in range(depth)) + "x" + "".join(marks[i % 6] for i in reversed(range(depth))) + "\n"
    if family == "section":
        return "".join("=" * min(i + 2, 6) + f" Section {i}\n\ntext\n\n" for i in range(depth))
    if family == "open-in-list":
        return "".join(f"{'*' * (i % 5 + 1)} item\n+\n--\n" for i in range(depth)) + "core\n" + "--\n" * depth
    if family == "include-cond":
        return "".join(f"ifdef::a{i}[]\n" for i in range(depth)) + "core\n" + "endif::[]\n" * depth
    raise ValueError(family)


def generate(seed, count, depths):
    rng = random.Random(seed)
    pool = seeds()
    cases = []
    for family in NESTING:
        for depth in depths:
            # Deep lists read in time growing faster than the square of
            # their depth (like Ruby's): their depth is capped, not their
            # stack use, which no longer grows with nesting.
            if family in SLOW_NESTING and depth > SLOW_DEPTH:
                continue
            cases.append((f"nest-{family}-{depth}", nesting(family, depth)))
    for n in range(count):
        base = rng.choice(pool)
        cases.append((f"mut-{seed}-{n}", mutate(rng, base, pool)))
    return cases


class Driver:
    """The robustness driver; restarted after an abort (stack overflow)."""

    def __init__(self, binary, stack):
        self.command = [str(binary), "--stack", str(stack)]
        self.proc = None

    def run(self, cases, timeout):
        results, pending = {}, list(cases)
        while pending:
            proc = subprocess.Popen(self.command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE)
            index = 0
            try:
                for case_id, source in pending:
                    data = source.encode("utf-8")
                    proc.stdin.write(f"{case_id} {len(data)}\n".encode() + data)
                    proc.stdin.flush()
                    ready, _, _ = select.select([proc.stdout], [], [], timeout)
                    line = proc.stdout.readline() if ready else b""
                    if not ready:
                        results[case_id] = {"id": case_id, "timeout": timeout}
                        proc.kill()
                        index += 1
                        break
                    if not line:
                        stderr = proc.stderr.read().decode("utf-8", "replace").strip().splitlines()
                        results[case_id] = {"id": case_id, "abort": stderr[-1] if stderr else f"exit {proc.wait()}"}
                        index += 1
                        break
                    results[case_id] = json.loads(line)
                    index += 1
                proc.stdin.close()
            except BrokenPipeError:
                stderr = proc.stderr.read().decode("utf-8", "replace").strip().splitlines()
                results[pending[index][0]] = {"id": pending[index][0], "abort": stderr[-1] if stderr else "broken pipe"}
                index += 1
            proc.kill()
            proc.wait()
            pending = pending[index:]
        return results


class Ruby:
    """A persistent reference worker, restarted after a timeout."""

    def __init__(self):
        self.proc = None

    def start(self):
        self.proc = subprocess.Popen(["ruby", str(ROOT / "scripts/asciidoctor-batch.rb")], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, text=True, encoding="utf-8")

    def convert(self, source, timeout):
        if self.proc is None or self.proc.poll() is not None:
            self.start()
        self.proc.stdin.write(json.dumps({"source": source}) + "\n")
        self.proc.stdin.flush()
        ready, _, _ = select.select([self.proc.stdout], [], [], timeout)
        if not ready:
            self.proc.kill()
            self.proc = None
            return {"error": f"timeout after {timeout}s"}
        line = self.proc.stdout.readline()
        if not line:
            self.proc = None
            return {"error": "worker exited"}
        return json.loads(line)


def adoctra(binary, source, timeout):
    command = [str(binary), "-", "--to", "html", "--embedded", "-a", "showtitle", "-S", "secure", "--stdout"]
    try:
        # Bytes, not text mode: universal newlines would turn a lone CR into LF.
        run = subprocess.run(command, input=source.encode("utf-8"), capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return {"error": f"timeout after {timeout}s"}
    stdout, stderr = run.stdout.decode("utf-8"), run.stderr.decode("utf-8")
    if run.returncode not in (0, 1):
        return {"error": f"exit {run.returncode}: {stderr.strip().splitlines()[-1] if stderr.strip() else ''}"}
    try:
        return {"html": stdout, "diagnostics": acceptance.cli_diagnostics(stderr)}
    except ValueError as error:
        return {"error": str(error)}


def compare(ruby, ours):
    """None when equal; else a short description of the first difference."""
    if "error" in ruby:
        return "RUBY_ERROR", ruby["error"]
    if "error" in ours:
        return "ADOCTRA_ERROR", ours["error"]
    try:
        expected = {"body": projection(ruby["html"]), "diagnostics": ruby["diagnostics"]}
    except ValueError as error:
        return "RUBY_ERROR", f"projection: {error}"
    try:
        actual = {"body": projection(ours["html"]), "diagnostics": ours["diagnostics"]}
    except ValueError as error:
        return "ADOCTRA_ERROR", f"projection: {error}"
    if expected == actual:
        return "PASS", None
    if expected["body"] != actual["body"]:
        return "DIFF", "body"
    return "DIFF", "diagnostics"


def minimize(source, still, budget=400):
    """Line-wise then character-wise deletion while `still(candidate)` holds."""
    calls = 0
    lines = source.split("\n")
    chunk = max(1, len(lines) // 2)
    while chunk >= 1 and calls < budget:
        i, changed = 0, False
        while i < len(lines) and calls < budget:
            candidate = lines[:i] + lines[i + chunk:]
            calls += 1
            if candidate and still("\n".join(candidate)):
                lines, changed = candidate, True
            else:
                i += chunk
        if not changed:
            chunk //= 2
    text = "\n".join(lines)
    i = 0
    while i < len(text) and calls < budget:
        candidate = text[:i] + text[i + 1:]
        calls += 1
        if candidate.strip() and still(candidate):
            text = candidate
        else:
            i += 1
    return text


def robust_stage(args, cases):
    binary = ROOT / "target/debug/examples/fuzz"
    subprocess.run(["cargo", "build", "-q", "-p", "adoctra-wasm", "--example", "fuzz"], cwd=ROOT, check=True)
    results = Driver(binary, args.stack).run(cases, args.timeout)
    sizes = {case_id: len(source.encode()) for case_id, source in cases}
    findings = []
    for case_id, result in results.items():
        kind = next((k for k in ("panic", "abort", "timeout") if k in result), None)
        if kind is None and result.get("violations"):
            kind = "span"
        # Slow: beyond 0.5 s or 1 ms per byte, whichever is larger.
        if kind is None and result["parse_ms"] + result["tokens_ms"] > max(500.0, sizes[case_id] / 1000 * 1000):
            kind = "slow"
        if kind:
            findings.append({"id": case_id, "kind": kind, "detail": result})
    return results, findings


def diff_stage(args, cases):
    binary = ROOT / "target/debug/adoctra"
    subprocess.run(["cargo", "build", "-q", "-p", "adoctra-cli"], cwd=ROOT, check=True)
    ruby = Ruby()
    statuses, findings, seen = {}, [], set()
    for case_id, source in cases[:args.diff_count]:
        status, detail = compare(ruby.convert(source, args.timeout), adoctra(binary, source, args.timeout))
        statuses[status] = statuses.get(status, 0) + 1
        if status == "PASS":
            continue
        finding = {"id": case_id, "kind": status, "detail": detail}
        if status in ("DIFF", "ADOCTRA_ERROR") and args.minimize:
            def still(candidate, want=(status, detail)):
                return compare(ruby.convert(candidate, args.timeout), adoctra(binary, candidate, args.timeout)) == want
            small = minimize(source, still)
            if small in seen:
                continue
            seen.add(small)
            finding["minimized"] = small
        findings.append(finding)
        print(f"{status:13} {case_id} {detail or ''}", flush=True)
    return statuses, findings


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--count", type=int, default=2000, help="mutated inputs")
    parser.add_argument("--depths", default="8,64,128,2048", help="nesting depths")
    parser.add_argument("--stage", choices=["robust", "diff", "both"], default="both")
    parser.add_argument("--diff-count", type=int, default=500, help="inputs compared with Ruby")
    parser.add_argument("--minimize", action="store_true")
    parser.add_argument("--stack", type=int, default=1 << 20)
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument("--report", type=Path, default=ROOT / "target/fuzz/report.json")
    args = parser.parse_args()
    cases = generate(args.seed, args.count, [int(d) for d in args.depths.split(",")])
    report = {"profile": PROFILE, "seed": args.seed, "count": len(cases), "stack": args.stack}
    started = time.time()
    if args.stage in ("robust", "both"):
        results, findings = robust_stage(args, cases)
        report["robust"] = {"inputs": len(results), "findings": findings}
        print(f"robust: {len(results)} inputs, {len(findings)} findings", flush=True)
        for finding in findings:
            print(f"  {finding['kind']:8} {finding['id']} {json.dumps(finding['detail'], ensure_ascii=False)[:200]}")
    if args.stage in ("diff", "both"):
        statuses, findings = diff_stage(args, cases)
        report["diff"] = {"statuses": statuses, "findings": findings}
        print(f"diff: {json.dumps(statuses, sort_keys=True)}", flush=True)
    report["seconds"] = round(time.time() - started, 1)
    report["cases"] = {case_id: source for case_id, source in cases}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    findings = report.get("robust", {}).get("findings", []) + report.get("diff", {}).get("findings", [])
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
