#!/usr/bin/env python3
"""Check pinned documents, diagnostics and block structure against Asciidoctor.

body-dom-diagnostics-v1 combines the existing body-dom-v1 projection with an
ordered list of warning/error severities and messages. It is not exact HTML,
an ASG/TCK profile, or a comparison of diagnostic source positions.
The separate block-structure-v1 manifest compares parser trees through JSON,
without rendering HTML; see structure.py for its limited schema.
inline-semantics-v1 uses a custom Ruby converter and the resolved JSON AST with
selected attribute snapshots; see semantics.py for its bounded schema.
diagnostic-positions-v1 compares ordered diagnostics with primary file/line
cursors, including explicit absence; see diagnostic_positions.py.
"""

import argparse
from collections import Counter
import difflib
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

import structure
import semantics
import diagnostic_positions

from compatibility import (
    CORPUS, ROOT, case_commands, classify, pretty, projection, validate_attributes,
    validate_resources,
)


PROFILE = "body-dom-diagnostics-v1"
DEFAULT_MANIFEST = CORPUS / "acceptance" / "cases.json"


def fixture_path(root, name):
    """Only regular files inside this corpus may be used as input or notices."""
    if (not isinstance(name, str) or not name
            or any(part in {"", ".", ".."} for part in name.split("/"))
            or any(char in name for char in "\\:\0")):
        raise ValueError(f"invalid fixture path: {name!r}")
    path = root / name
    if not path.resolve().is_relative_to(root.resolve()) or not path.is_file():
        raise ValueError(f"fixture is not a file inside corpus: {name}")
    return path


def load_corpus(path):
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if (not isinstance(manifest, dict)
            or not isinstance(manifest.get("files"), dict)
            or not isinstance(manifest.get("origins"), dict)
            or not isinstance(manifest.get("cases"), list)):
        raise ValueError("invalid acceptance manifest")
    if (manifest["profile"] not in {PROFILE, structure.PROFILE, semantics.PROFILE, diagnostic_positions.PROFILE}
            or manifest["asciidoctor"] != (CORPUS / "asciidoctor-version").read_text().strip()):
        raise ValueError("acceptance reference version/profile mismatch")
    if not manifest["files"] or not manifest["cases"] or not manifest["origins"]:
        raise ValueError("empty acceptance corpus")
    contents = {}
    for name, record in manifest["files"].items():
        if not isinstance(record, dict):
            raise ValueError(f"invalid fixture record: {name}")
        raw = fixture_path(path.parent, name).read_bytes()
        if hashlib.sha256(raw).hexdigest() != record["sha256"]:
            raise ValueError(f"fixture checksum mismatch: {name}")
        origin = manifest["origins"][record["origin"]]
        if not isinstance(origin, dict) or not all(
                isinstance(origin.get(key), str) and origin[key]
                for key in ("project", "revision", "source")):
            raise ValueError(f"incomplete provenance: {name}")
        notice = origin.get("notice")
        if notice:
            if not isinstance(notice, dict):
                raise ValueError(f"invalid notice: {name}")
            notice_bytes = fixture_path(path.parent, notice["path"]).read_bytes()
            if hashlib.sha256(notice_bytes).hexdigest() != notice["sha256"]:
                raise ValueError(f"notice checksum mismatch: {name}")
        contents[name] = raw.decode("utf-8")
    ids = set()
    cases = []
    for entry in manifest["cases"]:
        if not isinstance(entry, dict):
            raise ValueError("invalid acceptance case")
        case = entry.copy()
        identifier = case["id"]
        if not isinstance(identifier, str) or not re.fullmatch(r"[a-z0-9_-]+", identifier) or identifier in ids:
            raise ValueError(f"invalid or duplicate case id: {identifier}")
        ids.add(identifier)
        if (case["expect"] not in {"pass", "xfail"}
                or case.get("category") not in {"document", "diagnostics", "structure", "attributes", "substitutions", "include", "semantics"}
                or case["expect"] == "xfail" and not case.get("reason")):
            raise ValueError(f"invalid acceptance expectation: {identifier}")
        if "upstream" in case:
            upstream = case["upstream"]
            if (not isinstance(upstream, dict)
                    or not isinstance(upstream.get("line"), int)
                    or upstream["line"] < 1
                    or not isinstance(upstream.get("test"), str)
                    or not isinstance(upstream.get("adaptation"), str)
                    or not upstream["adaptation"]):
                raise ValueError(f"invalid upstream provenance: {identifier}")
            lines = contents[upstream["file"]].splitlines()
            if (upstream["line"] > len(lines)
                    or lines[upstream["line"] - 1].strip() != f"test '{upstream['test']}' do"):
                raise ValueError(f"upstream test location mismatch: {identifier}")
        case["source"] = contents[case["source"]]
        if "resources" in case:
            if not isinstance(case["resources"], dict):
                raise ValueError(f"invalid resources: {identifier}")
            case["resources"] = {name: contents[resource] for name, resource in case["resources"].items()}
        validate_resources(case)
        validate_attributes(case)
        if manifest['profile'] == semantics.PROFILE:
            semantics.validate_case(case)
        cases.append(case)
    return manifest, cases


def invoke(command, source):
    # Bytes, not text mode: universal newlines would turn a lone CR into LF.
    raw = subprocess.run(command, input=source.encode("utf-8"), capture_output=True, timeout=30)
    result = subprocess.CompletedProcess(raw.args, raw.returncode, raw.stdout.decode("utf-8"),
                                         raw.stderr.decode("utf-8"))
    if result.returncode:
        raise RuntimeError(f"{' '.join(map(str, command))}: {result.stderr.strip()}")
    return result


def diagnostic_text(message, root):
    # A temporary fixture directory is the sole platform-dependent portion of
    # diagnostic text. Preserve resource paths and everything after the root.
    return message.replace(str(root), "<fixture>") if root is not None else message


def cli_diagnostics(stderr, root=None, multiline=False):
    """Diagnostics from the CLI's stderr. With `multiline`, a line without
    the prefix continues the previous message (a message with a line feed,
    which Ruby keeps too); otherwise it is unrecognized output."""
    diagnostics = []
    lines = []
    raw = (stderr.removesuffix("\n").split("\n") if stderr else []) if multiline else stderr.splitlines()
    for line in raw:
        if multiline and lines and not line.startswith("adoctra: "):
            lines[-1] += "\n" + line
        else:
            lines.append(line)
    for line in lines:
        # Default conversion diagnostics have file/line or no cursor. Editor
        # mode adds columns/byte offsets. Reject any unrecognized output.
        match = re.fullmatch(r"adoctra: (warning|error|info): (.*)", line, re.DOTALL)
        if match is None:
            match = re.fullmatch(r"adoctra: [^\n]+?:[1-9]\d*: (warning|error|info): (.*)", line,
                                 re.DOTALL)
        if match is None:
            match = re.fullmatch(
                r"adoctra: [^\n]+?:\d+:\d+: (warning|error|info) at \d+\.\.\d+: (.*)", line,
                re.DOTALL,
            )
        if not match:
            raise ValueError(f"unrecognized adoctra diagnostic: {line!r}")
        if match[1] != "info":
            diagnostics.append({"severity": match[1], "message": diagnostic_text(match[2], root)})
    return diagnostics


def reference_report(result, root=None, profile=PROFILE, attribute_names=None):
    if result.stderr:
        raise ValueError(f"unexpected reference stderr: {result.stderr.strip()}")
    report = json.loads(result.stdout)
    if profile == diagnostic_positions.PROFILE:
        return diagnostic_positions.reference_projection(report, root)
    if profile == semantics.PROFILE:
        if not isinstance(report, dict) or set(report) != {'semantics'}:
            raise ValueError('invalid semantic report')
        return {'semantics': semantics.reference_projection(report['semantics'], attribute_names)}
    if profile == structure.PROFILE:
        return project_report(report, profile)
    if not isinstance(report, dict) or set(report) != {"html", "diagnostics"} or not isinstance(report["html"], str):
        raise ValueError("invalid reference report")
    validate_diagnostics(report["diagnostics"])
    for diagnostic in report["diagnostics"]:
        diagnostic["message"] = diagnostic_text(diagnostic["message"], root)
    projection(report["html"])  # Never store malformed reference HTML.
    return report


def validate_diagnostics(diagnostics):
    if not isinstance(diagnostics, list) or not all(
        isinstance(d, dict) and set(d) == {"severity", "message"}
        and d["severity"] in {"warning", "error", "fatal", "unknown"}
        and isinstance(d["message"], str) for d in diagnostics
    ):
        raise ValueError("invalid diagnostic report")


def project_report(report, profile=PROFILE, attribute_names=None):
    if profile == diagnostic_positions.PROFILE:
        return diagnostic_positions.validate(report)
    if profile == semantics.PROFILE:
        if not isinstance(report, dict) or set(report) != {'semantics'}:
            raise ValueError('invalid semantic report')
        semantics.validate(report['semantics'], attribute_names)
        return report
    if profile == structure.PROFILE:
        if not isinstance(report, dict) or set(report) != {"structure"}:
            raise ValueError("invalid structure report")
        structure.validate_structure(report["structure"])
        if report["structure"]["context"] != "document":
            raise ValueError("structure report must contain a document")
        return report
    if not isinstance(report, dict) or set(report) != {"html", "diagnostics"} or not isinstance(report["html"], str):
        raise ValueError("invalid stored reference report")
    validate_diagnostics(report["diagnostics"])
    return {"body": projection(report["html"]), "diagnostics": report["diagnostics"]}


def check_case(case, binary, root, *, update=False, verify=False, record=False, profile=PROFILE):
    attribute_names = case.get('semantic_attributes')
    reference_path = root / "reference" / f"{case['id']}.json"
    baseline_path = root / "baseline" / f"{case['id']}.json"
    with case_commands(case, binary) as (reference_command, actual_command):
        fixture = Path(actual_command[1]).parent if "resources" in case else None
        if profile in {structure.PROFILE, semantics.PROFILE}:
            actual_command[actual_command.index("--to") + 1] = "json"
        if profile == semantics.PROFILE:
            semantics.validate_case(case)
            actual_command.append('--json-attributes')
            for name in attribute_names:
                reference_command.extend(['--semantic-attribute', name])
        if update or verify:
            reference_command.insert(2, {structure.PROFILE: '--structure', semantics.PROFILE: '--semantics',
                                         diagnostic_positions.PROFILE: '--diagnostic-positions'}.get(profile, '--report'))
            reference = reference_report(invoke(reference_command, case["source"]), fixture, profile, attribute_names)
            if update:
                reference_path.parent.mkdir(exist_ok=True)
                reference_path.write_text(pretty(reference), encoding="utf-8")
            elif reference != json.loads(reference_path.read_text(encoding="utf-8")):
                raise ValueError(f"reference drift: {case['id']}; review before --update-reference")
        if update:
            return None
        expected = project_report(json.loads(reference_path.read_text(encoding="utf-8")), profile, attribute_names)
        output = invoke(actual_command, case["source"])
        diagnostics = cli_diagnostics(output.stderr, fixture)
        if profile == diagnostic_positions.PROFILE:
            actual = diagnostic_positions.cli_projection(output.stderr, fixture)
        elif profile == semantics.PROFILE:
            if diagnostics:
                raise ValueError('diagnostics are outside inline-semantics-v1')
            actual = {'semantics': semantics.document_projection(json.loads(output.stdout), attribute_names)}
        elif profile == structure.PROFILE:
            actual = {"structure": structure.document_projection(json.loads(output.stdout))}
        else:
            actual = {"body": projection(output.stdout), "diagnostics": diagnostics}
    baseline = None
    if case["expect"] == "xfail":
        if record:
            if actual == expected:
                raise ValueError(f"{case['id']} is fixed; change its expectation to pass")
            baseline_path.parent.mkdir(exist_ok=True)
            baseline_path.write_text(pretty(actual), encoding="utf-8")
        baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
        if profile in {structure.PROFILE, semantics.PROFILE, diagnostic_positions.PROFILE}:
            project_report(baseline, profile, attribute_names)
        else:
            if (not isinstance(baseline, dict) or set(baseline) != {"body", "diagnostics"}
                    or not isinstance(baseline["body"], list)):
                raise ValueError(f"invalid baseline: {case['id']}")
            validate_diagnostics(baseline["diagnostics"])
    status = classify(case, actual, expected, baseline)
    result = {
        "id": case["id"], "category": case["category"], "status": status,
        "checks": {key: "PASS" if actual[key] == expected[key] else "DIFF" for key in expected},
        "reason": case.get("reason"),
    }
    print(f"{status:5} {case['id']}" + (f" — {case['reason']}" if case.get("reason") else ""))
    if status == "FAIL":
        comparison = baseline if baseline is not None else expected
        print("".join(difflib.unified_diff(pretty(comparison).splitlines(True), pretty(actual).splitlines(True),
                                        fromfile="baseline" if baseline is not None else "asciidoctor",
                                        tofile="adoctra")))
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/adoctra")
    parser.add_argument("--case", action="append", help="run only this case (repeatable)")
    parser.add_argument("--json-report", type=Path)
    actions = parser.add_mutually_exclusive_group()
    actions.add_argument("--verify-reference", action="store_true")
    actions.add_argument("--update-reference", action="store_true")
    actions.add_argument("--record-known-differences", action="store_true")
    args = parser.parse_args(argv)
    manifest, cases = load_corpus(args.manifest)
    if args.case:
        missing = set(args.case) - {case["id"] for case in cases}
        if missing:
            raise ValueError(f"unknown acceptance cases: {', '.join(sorted(missing))}")
        cases = [case for case in cases if case["id"] in args.case]
    results = []
    for case in cases:
        try:
            result = check_case(case, args.binary, args.manifest.parent, update=args.update_reference,
                                verify=args.verify_reference, record=args.record_known_differences,
                                profile=manifest["profile"])
        except (OSError, ValueError, TypeError, RuntimeError, KeyError, subprocess.TimeoutExpired) as error:
            raise RuntimeError(f"{case['id']}: {error}") from error
        if result:
            results.append(result)
    if args.update_reference:
        print(f"Updated {len(cases)} acceptance references using Asciidoctor {manifest['asciidoctor']}.")
        return 0
    counts = dict(Counter(result["status"] for result in results))
    categories = {category: dict(Counter(r["status"] for r in results if r["category"] == category))
                  for category in sorted({r["category"] for r in results})}
    print(json.dumps({"counts": counts, "categories": categories}, sort_keys=True))
    if args.json_report:
        args.json_report.write_text(pretty({
            "profile": manifest["profile"], "asciidoctor": manifest["asciidoctor"],
            "counts": counts, "categories": categories, "cases": results,
        }), encoding="utf-8")
    return int(any(r["status"] in {"FAIL", "XPASS"} for r in results))


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, TypeError, RuntimeError, KeyError, subprocess.TimeoutExpired) as error:
        print(f"acceptance harness error: {error}", file=sys.stderr)
        sys.exit(2)
