#!/usr/bin/env python3
"""rouge-listings-v1: server-side Rouge highlighting in real documents.

Every document of the acceptance corpus converts with `source-highlighter=rouge`
plus the attributes of each configuration below. The `<code>` content of every
highlighted listing (`<pre class="rouge …">`) must equal pinned Ruby's byte for
byte, in document order. Out of the profile: the `<pre>` attributes (adoctra
keeps a block's id and roles there, by design), the theme stylesheet in the
head (golden `rouge/` tests) and the surrounding DOM and diagnostics (the
acceptance profile, with each document's own attributes).

Ruby's listings are stored under compatibility/rouge-listings/, one file per
configuration, with a digest of each case's inputs: the offline run compares
adoctra with them, `--verify-reference` re-runs Ruby and compares with the
stored files, `--update-reference` rewrites them. A missing or stale
reference, a listing count mismatch and any differing listing fail.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import difflib
import hashlib
import json
from pathlib import Path
import subprocess
import sys

import acceptance
from compatibility import (CORPUS, ROOT, case_commands, load_reference_cache, ruby_environment,
                           store_reference_cache, verification_key)


PROFILE = "rouge-listings-v1"
MANIFEST = CORPUS / "acceptance" / "cases.json"
REFERENCE_DIR = CORPUS / "rouge-listings"
# Local record of cases `--verify-reference` already checked with Ruby: a key
# covers the Ruby environment, the configured case and its stored reference.
REFERENCE_CACHE = ROOT / "target/reference-cache/rouge-listings.json"
CONFIGURATIONS = {
    "default": {},
    "style": {"rouge-css": "style", "rouge-style": "monokai"},
    "linenums": {"source-linenums-option": ""},
    "linenums-inline": {"source-linenums-option": "", "rouge-linenums-mode": "inline"},
}
PRE = '<pre class="rouge'


def listings(html):
    """The `<code …>…` content of each rouge listing, without the `<pre>`."""
    result = []
    rest = html
    while (start := rest.find(PRE)) >= 0:
        code = rest.find("<code", start)
        end = rest.find("</code>", code)
        if code < 0 or end < 0:
            raise ValueError("unterminated rouge listing")
        result.append(rest[code:end])
        rest = rest[end:]
    return result


def configured(case, configuration):
    case = dict(case)
    case["attributes"] = {**case.get("attributes", {}), "source-highlighter": "rouge",
                          **CONFIGURATIONS[configuration]}
    return case


def inputs_digest(case):
    inputs = {key: case[key] for key in ("source", "attributes", "resources", "binary_resources") if key in case}
    payload = json.dumps(inputs, ensure_ascii=False, sort_keys=True)
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def run(command, source):
    result = subprocess.run(command, input=source.encode("utf-8"), capture_output=True, timeout=300)
    if result.returncode:
        raise RuntimeError(f"{command[0]} exited {result.returncode}: "
                           f"{result.stderr.decode('utf-8', 'replace').strip()}")
    return listings(result.stdout.decode("utf-8"))


def convert(case, binary, ruby):
    """Ruby's (`ruby`) or adoctra's listings of a configured case."""
    with case_commands(case, binary) as (reference, actual):
        return run(reference if ruby else actual, case["source"])


def reference_path(configuration):
    return REFERENCE_DIR / f"{configuration}.json"


def versions():
    return {"asciidoctor": (CORPUS / "asciidoctor-version").read_text().strip(),
            "rouge": (CORPUS / "rouge-version").read_text().strip()}


def load_reference(configuration):
    path = reference_path(configuration)
    if not path.is_file():
        raise ValueError(f"missing reference: {path} (run --update-reference)")
    data = json.loads(path.read_text(encoding="utf-8"))
    expected = {"profile": PROFILE, "configuration": CONFIGURATIONS[configuration], **versions()}
    if {key: data.get(key) for key in expected} != expected:
        raise ValueError(f"reference profile, configuration or versions mismatch: {path}")
    return data["cases"]


def compare(identifier, digest, reference, actual):
    """The status of one case and, for a difference, its first diff lines."""
    entry = reference.get(identifier)
    if entry is None or entry["inputs"] != digest:
        return "STALE", [f"reference of {identifier} does not match its inputs (run --update-reference)"]
    expected = entry["listings"]
    if expected == actual:
        return "PASS", []
    if len(expected) != len(actual):
        return "DIFF", [f"{len(expected)} listings in Ruby, {len(actual)} in adoctra"]
    index = next(i for i, (x, y) in enumerate(zip(expected, actual)) if x != y)
    diff = difflib.unified_diff(expected[index].splitlines(True), actual[index].splitlines(True),
                                f"ruby listing {index + 1}", f"adoctra listing {index + 1}")
    return "DIFF", list(diff)[:40]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/adoctra")
    parser.add_argument("--configuration", action="append", choices=sorted(CONFIGURATIONS),
                        help="run only this configuration (repeatable)")
    parser.add_argument("--case", action="append", help="run only this case (repeatable)")
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--json-report", type=Path)
    actions = parser.add_mutually_exclusive_group()
    actions.add_argument("--verify-reference", action="store_true",
                         help="re-run Ruby and compare with the stored reference")
    actions.add_argument("--update-reference", action="store_true",
                         help="re-run Ruby and rewrite the stored reference")
    parser.add_argument("--no-reference-cache", action="store_true",
                        help="with --verify-reference, run Ruby for every case instead of skipping cases "
                             "already verified with the same environment, inputs and reference")
    args = parser.parse_args(argv)

    _, cases = acceptance.load_corpus(MANIFEST)
    if args.case:
        unknown = set(args.case) - {case["id"] for case in cases}
        if unknown:
            parser.error(f"unknown case: {', '.join(sorted(unknown))}")
        if args.update_reference:
            parser.error("--update-reference rewrites whole configurations; do not combine it with --case")
        cases = [case for case in cases if case["id"] in args.case]
    ruby = args.verify_reference or args.update_reference
    use_cache = args.verify_reference and not args.no_reference_cache
    environment = ruby_environment() if use_cache else None
    cached = load_reference_cache(REFERENCE_CACHE) if use_cache else set()
    verified = set()
    hits = checked = 0
    report = {"profile": PROFILE, "configurations": {}}
    failed = False
    for configuration in args.configuration or list(CONFIGURATIONS):
        jobs = [configured(case, configuration) for case in cases]
        reference = None if args.update_reference else load_reference(configuration)

        def key(case):
            # The stored entry itself is part of the key: a changed reference
            # (or input digest inside it) runs Ruby again.
            entry = reference.get(case["id"]) if reference is not None else None
            return verification_key(environment, case, [PROFILE, configuration], entry)

        def side(case):
            if use_cache and key(case) in cached:
                return None  # verified earlier with this environment and reference
            return convert(case, args.binary, ruby)
        with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
            results = list(pool.map(side, jobs))
        checked += len(jobs)
        if args.update_reference:
            data = {"profile": PROFILE, **versions(), "configuration": CONFIGURATIONS[configuration],
                    "cases": {case["id"]: {"inputs": inputs_digest(case), "listings": found}
                              for case, found in zip(jobs, results)}}
            REFERENCE_DIR.mkdir(parents=True, exist_ok=True)
            reference_path(configuration).write_text(
                json.dumps(data, ensure_ascii=False, indent=1, sort_keys=True) + "\n", encoding="utf-8")
            count = sum(map(len, results))
            print(f"{configuration}: {count} listings in {len(jobs)} documents written")
            report["configurations"][configuration] = {"listings": count}
            continue
        counts = {}
        listings_total = 0
        for case, found in zip(jobs, results):
            if found is None:
                hits += 1
                verified.add(key(case))
                found = reference[case["id"]]["listings"]
            status, detail = compare(case["id"], inputs_digest(case), reference, found)
            if use_cache and status == "PASS":
                verified.add(key(case))
            counts[status] = counts.get(status, 0) + 1
            listings_total += len(found)
            if status != "PASS":
                failed = True
                print(f"{status} {configuration} {case['id']}")
                print("".join(line if line.endswith("\n") else line + "\n" for line in detail), end="")
        side = "Ruby" if ruby else "adoctra"
        print(f"{configuration}: {listings_total} {side} listings, {json.dumps(counts, sort_keys=True)}")
        report["configurations"][configuration] = {"listings": listings_total, "counts": counts}
    if use_cache:
        # A partial run keeps the other cases' keys; a full one prunes stale keys.
        store_reference_cache(cached | verified if args.case or args.configuration else verified, REFERENCE_CACHE)
        print(f"Ruby re-run for {checked - hits} configured cases; {hits} unchanged since their last verification.")
    if args.json_report:
        args.json_report.parent.mkdir(parents=True, exist_ok=True)
        args.json_report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
