#!/usr/bin/env python3
"""visual-v1: compare how Ruby's and adoctra's standalone pages *look*.

adoctra keeps modern semantic markup on purpose, so markup is never compared
here; the rendered result must be as close as possible to Asciidoctor's page
with its default stylesheet. For every case both processors render the same
fixture file (safe mode, `reproducible`, web fonts off), and a pinned headless
Chrome (scripts/visual/measure.mjs) measures each page at every viewport:

* text: every rendered word with its box, computed text style and effective
  colours (text and background composited over white, compared as #rrggbb). Words are aligned by text (difflib), then compared: style
  properties exactly, the left edge `x` and the vertical step `dy` between
  consecutive aligned words within GEOMETRY_TOLERANCE pixels. Missing/extra
  words are text differences.
* pixels: full-page screenshots compared with pixelmatch's YIQ distance
  (threshold 0.1, no anti-aliasing detection) on a white canvas of the larger
  size; the share of different pixels must not exceed PIXEL_LIMIT percent.

Generated content (::before/::marker, CSS icons, list numbers) is invisible to
the text comparison and covered only by pixels. Rendering depends on Chrome,
playwright-core and the installed fonts, so a baseline records that
environment and is only comparable in the same one. The profile does not
claim language, DOM or diagnostic compatibility; see docs/visual.adoc.
"""

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import difflib
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "compatibility"
DRIVER = ROOT / "scripts" / "visual"
DEFAULT_MANIFEST = CORPUS / "visual" / "cases.json"
PROFILE = "visual-v1"
STYLE = ["font-family", "font-size", "font-weight", "font-style", "color",
         "text-decoration-line", "text-transform", "letter-spacing", "font-variant-caps",
         "background-color"]
GEOMETRY_TOLERANCE = 1.0
PIXEL_THRESHOLD = 0.1
PIXEL_LIMIT = 0.5
EXAMPLES = 3


def load_manifest(path):
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if (not isinstance(manifest, dict) or manifest.get("profile") != PROFILE
            or manifest.get("asciidoctor") != (CORPUS / "asciidoctor-version").read_text().strip()):
        raise ValueError("visual manifest profile/version mismatch")
    viewports = manifest.get("viewports")
    if (not isinstance(viewports, list) or not viewports
            or len({v.get("name") for v in viewports}) != len(viewports)
            or not all(isinstance(v.get("width"), int) and isinstance(v.get("height"), int)
                       and re.fullmatch(r"[a-z0-9]+", v.get("name", "")) for v in viewports)):
        raise ValueError("invalid visual viewports")
    ids = set()
    for case in manifest.get("cases") or []:
        identifier = case.get("id")
        if not isinstance(identifier, str) or not re.fullmatch(r"[a-z0-9-]+", identifier) or identifier in ids:
            raise ValueError(f"invalid or duplicate case id: {identifier!r}")
        ids.add(identifier)
        source = case.get("source")
        if (not isinstance(source, str) or any(part in {"", ".", ".."} for part in source.split("/"))
                or not (ROOT / source).resolve().is_relative_to(ROOT) or not (ROOT / source).is_file()):
            raise ValueError(f"invalid visual source: {source!r}")
        attributes = case.get("attributes", {})
        if not isinstance(attributes, dict) or not all(
                isinstance(k, str) and re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_-]*!?", k) and isinstance(v, str)
                for k, v in attributes.items()):
            raise ValueError(f"invalid visual attributes: {identifier}")
        resources = case.get("resources", {})
        if not isinstance(resources, dict) or not all(
                safe_name(name) and isinstance(path, str) and safe_name(path) and (ROOT / path).is_file()
                and (ROOT / path).resolve().is_relative_to(ROOT) for name, path in resources.items()) \
                or "main.adoc" in resources:
            raise ValueError(f"invalid visual resources: {identifier}")
        if not isinstance(case.get("embedded", False), bool):
            raise ValueError(f"invalid visual embedded flag: {identifier}")
    if not ids:
        raise ValueError("empty visual corpus")
    return manifest


def safe_name(name):
    """A relative slash path without empty, `.` or `..` segments."""
    return (isinstance(name, str) and bool(name) and "\\" not in name and "\0" not in name
            and all(part not in {"", ".", ".."} for part in name.split("/")))


def case_id(path):
    """A stable id from a repository path below a fixture directory."""
    stem = path.with_suffix("").as_posix()
    return re.sub(r"[^a-z0-9]+", "-", stem.lower()).strip("-")


def with_base(html, directory):
    """Resolve relative resources (images, SVG) against the fixture directory."""
    base = f'<base href="{directory.resolve().as_uri()}/">'
    head = re.search(r"<head[^>]*>", html)
    if not head:
        raise ValueError("standalone page without <head>")
    return html[:head.end()] + base + html[head.end():]


def embedded_page(fragment, stylesheet):
    """An embedded fragment in a minimal page with its processor's stylesheet,
    as a host page embedding the output would include it."""
    return ('<!DOCTYPE html>\n<html lang="en">\n<head>\n<meta charset="UTF-8">\n'
            '<meta name="viewport" content="width=device-width, initial-scale=1.0">\n'
            f"<style>\n{stylesheet}\n</style>\n</head>\n<body>\n{fragment}</body>\n</html>\n")


STYLESHEETS = {}


def stylesheet(side):
    """Ruby's default stylesheet (from the pinned gem) or adoctra.css."""
    if side not in STYLESHEETS:
        if side == "ruby":
            STYLESHEETS[side] = subprocess.run(
                ["ruby", str(ROOT / "scripts/asciidoctor-reference.rb"), "--visual-stylesheet"],
                capture_output=True, text=True, check=True).stdout
        else:
            STYLESHEETS[side] = (ROOT / "crates/adoctra-html/assets/adoctra.css").read_text(encoding="utf-8")
    return STYLESHEETS[side]


def render(case, binary, work):
    source = ROOT / case["source"]
    if case.get("resources"):
        # Include resources under their manifest names next to main.adoc, as
        # in the acceptance corpus.
        tree = work / f"{case['id']}-tree"
        for name, path in {"main.adoc": case["source"], **case["resources"]}.items():
            target = tree / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes((ROOT / path).read_bytes())
        source = tree / "main.adoc"
    embedded = case.get("embedded", False)
    attributes = case.get("attributes", {})
    ruby = ["ruby", str(ROOT / "scripts/asciidoctor-reference.rb"), "--visual"]
    ours = [str(binary), str(source), "--to", "html", "--stdout", "-a", "reproducible", "-a", "webfonts!"]
    if embedded:
        ruby.append("--embedded")
        ours.append("-s")
    for name, value in sorted(attributes.items()):
        spelling = name if name.endswith("!") else f"{name}={value}"
        ruby += ["--attribute", spelling]
        ours += ["-a", spelling]
    ruby.append(str(source))
    pair = {"id": case["id"]}
    for side, command in (("ruby", ruby), ("adoctra", ours)):
        run = subprocess.run(command, capture_output=True, text=True, cwd=source.parent)
        if run.returncode != 0:
            raise RuntimeError(f"{case['id']}: {side} failed ({run.returncode}): {run.stderr.strip()[:500]}")
        html = embedded_page(run.stdout, stylesheet(side)) if embedded else run.stdout
        target = work / f"{case['id']}-{side}.html"
        target.write_text(with_base(html, source.parent), encoding="utf-8")
        pair[side] = str(target)
    return pair


COLOR = re.compile(r"rgba?\(([\d.]+),\s*([\d.]+),\s*([\d.]+)(?:,\s*([\d.]+))?\)"
                   r"|color\(srgb\s+([-\d.e]+)\s+([-\d.e]+)\s+([-\d.e]+)(?:\s*/\s*([\d.]+))?\)")


def parse_color(value):
    """A computed CSS colour as (r, g, b, alpha) with 0-255 channels."""
    match = COLOR.fullmatch(value.strip())
    if not match:
        raise ValueError(f"unsupported computed colour: {value!r}")
    if match.group(1) is not None:
        r, g, b, a = match.group(1, 2, 3, 4)
        return float(r), float(g), float(b), 1.0 if a is None else float(a)
    r, g, b, a = match.group(5, 6, 7, 8)
    return 255 * float(r), 255 * float(g), 255 * float(b), 1.0 if a is None else float(a)


def over(color, base):
    r, g, b, a = color
    return tuple(a * c + (1 - a) * d for c, d in zip((r, g, b), base))


def hex_color(rgb):
    return "#" + "".join(f"{min(255, max(0, round(c))):02x}" for c in rgb)


def painted(text, backgrounds):
    """The text and background colours as seen on screen (over white)."""
    base = (255.0, 255.0, 255.0)
    for layer in reversed([part for part in backgrounds.split(" | ") if part]):
        base = over(parse_color(layer), base)
    return hex_color(over(parse_color(text), base)), hex_color(base)


def words(page):
    """Expand the driver's interned styles: (text, x, y, style tuple, context)."""
    styles = []
    color = STYLE.index("color")
    for entry in page["styles"]:
        values = list(entry[:len(STYLE)])
        values[color], values[-1] = painted(values[color], values[-1])
        styles.append((tuple(values), entry[len(STYLE)]))
    return [(text, x, y, *styles[s]) for text, x, y, _w, _h, s in page["words"]]


def compare(ruby, ours, tolerance=GEOMETRY_TOLERANCE):
    """Align two word lists and count their visual differences."""
    matcher = difflib.SequenceMatcher(None, [w[0] for w in ruby], [w[0] for w in ours], autojunk=False)
    pairs = [(block.a + k, block.b + k) for block in matcher.get_matching_blocks() for k in range(block.size)]
    missing = [ruby[i][0] for i in sorted(set(range(len(ruby))) - {i for i, _ in pairs})]
    extra = [ours[j][0] for j in sorted(set(range(len(ours))) - {j for _, j in pairs})]
    style = Counter()
    examples = {}
    dx = Counter()
    dy = Counter()
    shifted = []
    previous = None
    for i, j in pairs:
        a, b = ruby[i], ours[j]
        for name, left, right in zip(STYLE, a[3], b[3]):
            if left != right:
                key = (name, left, right)
                style[key] += 1
                examples.setdefault(key, {"text": a[0], "ruby": a[4], "adoctra": b[4]})
        if abs(a[1] - b[1]) > tolerance:
            dx[round(b[1] - a[1])] += 1
        if previous is not None:
            step = (b[2] - ours[previous[1]][2]) - (a[2] - ruby[previous[0]][2])
            if abs(step) > tolerance:
                dy[round(step)] += 1
                if len(shifted) < EXAMPLES:
                    shifted.append({"text": a[0], "dy": round(step), "ruby": a[4], "adoctra": b[4]})
        previous = (i, j)
    return {
        "words": {"ruby": len(ruby), "adoctra": len(ours), "aligned": len(pairs)},
        "text": {"missing": len(missing), "extra": len(extra),
                 "examples": {"missing": missing[:EXAMPLES], "extra": extra[:EXAMPLES]}},
        "style": [[*key, count, examples[key]] for key, count in sorted(style.items())],
        "geometry": {"x": sum(dx.values()), "dy": sum(dy.values()),
                     "examples": {"dx": [[d, n] for d, n in dx.most_common(EXAMPLES)],
                                  "dy": [[d, n] for d, n in dy.most_common(EXAMPLES)],
                                  "shifted": shifted}},
    }


def measurement(record):
    """One case/viewport result from a driver record."""
    if "error" in record:
        return {"status": "error", "error": record["error"]}
    result = compare(words(record["ruby"]), words(record["adoctra"]))
    pixels = record["pixels"]
    result["pixels"] = round(100 * pixels["different"] / pixels["total"], 2)
    result["size"] = {side: [record[side]["width"], record[side]["height"]] for side in ("ruby", "adoctra")}
    result["blocked"] = {side: record[side]["blocked"] for side in ("ruby", "adoctra")}
    result["errors"] = {side: record[side]["errors"] for side in ("ruby", "adoctra")}
    same = (not result["text"]["missing"] and not result["text"]["extra"] and not result["style"]
            and not result["geometry"]["x"] and not result["geometry"]["dy"] and result["pixels"] <= PIXEL_LIMIT)
    result["status"] = "match" if same else "differs"
    return result


def signature(result):
    """The part of a result that a baseline freezes (examples are for people)."""
    if result["status"] == "error":
        return {"status": "error", "error": result["error"]}
    return {
        "status": result["status"],
        "text": [result["text"]["missing"], result["text"]["extra"]],
        "style": [entry[:4] for entry in result["style"]],
        "geometry": [result["geometry"]["x"], result["geometry"]["dy"]],
        "pixels": result["pixels"],
    }


def no_worse(current, baseline):
    """Every frozen difference shrank or stayed (an improvement, not drift)."""
    if "error" in (current["status"], baseline["status"]):
        return current["status"] != "error"
    before = {tuple(entry[:3]): entry[3] for entry in baseline["style"]}
    return (all(entry[3] <= before.get(tuple(entry[:3]), 0) for entry in current["style"])
            and all(a <= b for a, b in zip(current["text"] + current["geometry"], baseline["text"] + baseline["geometry"]))
            and current["pixels"] <= baseline["pixels"])


def check(report, baseline):
    """Return (key, kind) for every frozen observation that is not its baseline."""
    problems = []
    if baseline["environment"] != report["environment"]:
        return [("environment", "mismatch")]
    expected = baseline["cases"]
    for key in sorted(set(expected) | set(report["cases"])):
        if key not in report["cases"]:
            problems.append((key, "missing"))
        elif key not in expected:
            problems.append((key, "new"))
        else:
            current = report["cases"][key]
            if current != expected[key]:
                problems.append((key, "improved" if no_worse(current, expected[key]) else "drift"))
    return problems


def environment(browser):
    fonts = subprocess.run(["fc-list", "--format", "%{family}|%{style}|%{file}\n"],
                           capture_output=True, text=True, check=True).stdout
    lock = json.loads((DRIVER / "package-lock.json").read_text())
    ruby = subprocess.run(["ruby", "-e", "print RUBY_VERSION"], capture_output=True, text=True, check=True).stdout
    return {
        "browser": browser,
        "playwright-core": lock["packages"]["node_modules/playwright-core"]["version"],
        "fonts": hashlib.sha256("".join(sorted(fonts.splitlines(keepends=True))).encode()).hexdigest(),
        "ruby": ruby,
        "geometry-tolerance": GEOMETRY_TOLERANCE,
        "pixel-threshold": PIXEL_THRESHOLD,
        "pixel-limit": PIXEL_LIMIT,
    }


def summary(report):
    """Corpus-wide table of the most frequent differences, per viewport."""
    lines = []
    for viewport in report["viewports"]:
        results = {k: v for k, v in report["cases"].items() if k.endswith(f"@{viewport}")}
        statuses = Counter(r["status"] for r in results.values())
        lines.append(f"{viewport}: {len(results)} cases, " + ", ".join(f"{n} {s}" for s, n in sorted(statuses.items())))
        measured = [r for r in results.values() if r["status"] != "error"]
        if not measured:
            continue
        aligned = sum(r["words"]["aligned"] for r in measured)
        styled = Counter()
        cases = Counter()
        for r in measured:
            for name, left, right, count, _ in r["style"]:
                styled[(name, left, right)] += count
                cases[(name, left, right)] += 1
        lines.append(f"  words aligned {aligned}, missing {sum(r['text']['missing'] for r in measured)}, "
                     f"extra {sum(r['text']['extra'] for r in measured)}; geometry x {sum(r['geometry']['x'] for r in measured)}, "
                     f"dy {sum(r['geometry']['dy'] for r in measured)}; mean pixels "
                     f"{sum(r['pixels'] for r in measured) / len(measured):.2f}%")
        for (name, left, right), count in styled.most_common(12):
            lines.append(f"  {count:6} words / {cases[(name, left, right)]:3} cases  {name}: {left} -> {right}")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/adoctra")
    parser.add_argument("--baseline", type=Path, help="compare with a recorded report (same environment)")
    parser.add_argument("--record", type=Path, help="write the frozen baseline")
    parser.add_argument("--report", type=Path, help="write the full report with examples")
    parser.add_argument("--screenshots", type=Path, help="keep screenshots and diff images here")
    parser.add_argument("--only", action="append", default=[], help="case id (repeatable)")
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    if args.only and (args.baseline or args.record):
        parser.error("--only cannot be combined with --baseline/--record")
    manifest = load_manifest(args.manifest)
    cases = [c for c in manifest["cases"] if not args.only or c["id"] in args.only]
    if args.only and len(cases) != len(set(args.only)):
        parser.error("unknown --only case id")
    if not (DRIVER / "node_modules" / "playwright-core").is_dir():
        sys.exit("run `npm ci --ignore-scripts` in scripts/visual first")
    with tempfile.TemporaryDirectory(prefix="adoctra-visual-") as scratch:
        work = Path(scratch)
        shots = args.screenshots or work / "shots"
        shots.mkdir(parents=True, exist_ok=True)
        with ThreadPoolExecutor(args.jobs) as pool:
            pairs = list(pool.map(lambda case: render(case, args.binary, work), cases))
        job = work / "job.json"
        job.write_text(json.dumps({"pairs": pairs, "viewports": manifest["viewports"], "out": str(shots),
                                   "threshold": PIXEL_THRESHOLD, "jobs": args.jobs}))
        run = subprocess.run(["node", str(DRIVER / "measure.mjs"), str(job)], capture_output=True, text=True)
        if run.returncode != 0:
            sys.exit(f"browser driver failed: {run.stderr.strip()}")
        records = [json.loads(line) for line in run.stdout.splitlines()]
    browser = records.pop()["browser"]
    report = {
        "profile": PROFILE,
        "asciidoctor": manifest["asciidoctor"],
        "environment": environment(browser),
        "viewports": [v["name"] for v in manifest["viewports"]],
        "cases": {f"{r['id']}@{r['viewport']}": measurement(r) for r in sorted(records, key=lambda r: (r["id"], r["viewport"]))},
    }
    print(summary(report))
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    frozen = {key: report[key] for key in ("profile", "asciidoctor", "environment", "viewports")}
    frozen["cases"] = {key: signature(result) for key, result in report["cases"].items()}
    if args.record:
        args.record.write_text(json.dumps(frozen, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
        print(f"recorded {len(frozen['cases'])} observations in {args.record}")
    if args.baseline:
        problems = check(frozen, json.loads(args.baseline.read_text(encoding="utf-8")))
        for key, kind in problems:
            print(f"{kind.upper()}: {key}")
        if problems:
            sys.exit(1)
        print("baseline: no drift")
    if any(r["status"] == "error" for r in report["cases"].values()):
        sys.exit(1)


if __name__ == "__main__":
    main()
