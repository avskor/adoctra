#!/usr/bin/env python3
"""Import every page of a pinned Antora documentation component.

The acceptance corpus keeps unmodified upstream files under
compatibility/acceptance/sources/<origin>/<upstream path>. This script takes
a checkout of the origin's pinned revision and adds each page under
`docs/modules/*/pages/` that the manifests do not have yet, together with the
files it includes (Antora resource ids `example$`, `partial$`, `page$`, with
an optional `module:` prefix, resolved transitively). It writes the files,
their SHA-256 records and one case per page to the body, structure and
semantic manifests and to the visual manifest, with the host attributes the
origin's existing cases use (the resolved `antora.yml` attributes).

Includes Antora cannot resolve either (attribute references, missing files)
stay unregistered: both processors then report the same missing include.
Images are not imported (they stay references, as for the other pages).

    python3 scripts/import_docs.py --origin asciidoc-lang-0.3.0 --checkout DIR --prefix asciidoc-lang

References are not generated here; run acceptance.py --update-reference
--case ... for the new ids and review the results.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parent.parent
ACCEPTANCE = ROOT / "compatibility" / "acceptance"
VISUAL = ROOT / "compatibility" / "visual" / "cases.json"
MANIFESTS = {"body": "cases.json", "structure": "structure.json", "semantics": "semantics.json"}
CASE_PREFIX = {"body": "", "structure": "structure-", "semantics": "semantic-"}
FAMILIES = {"example": "examples", "partial": "partials", "page": "pages"}
INCLUDE = re.compile(r"^include::([^\[\s]+)\[", re.M)
RESOURCE_ID = re.compile(r"(?:([\w-]+):)?(example|partial|page)\$(.+)")


def resolve(docs, module, target):
    """The file an Antora resource id names, or None when Antora's resolver
    would not find one (attribute references, plain paths outside the page)."""
    match = RESOURCE_ID.fullmatch(target)
    if not match or "{" in target or ".." in match[3].split("/"):
        return None
    path = docs / "modules" / (match[1] or module) / FAMILIES[match[2]] / match[3]
    return path if path.is_file() else None


def resources(docs, module, text, found=None):
    """Resource id -> file for every include of `text`, transitively."""
    found = {} if found is None else found
    for target in INCLUDE.findall(text):
        if target in found:
            continue
        path = resolve(docs, module, target)
        if path is not None:
            found[target] = path
            resources(docs, module, path.read_text(encoding="utf-8"), found)
    return found


def slug(text):
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def case_id(prefix, module, page):
    stem = page.with_suffix("").as_posix()
    return slug(f"{prefix}-{module}-{stem}" if module != "ROOT" else f"{prefix}-{stem}")


def dump(path, data, indent):
    path.write_text(json.dumps(data, indent=indent, ensure_ascii=False) + "\n", encoding="utf-8")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--origin", required=True, help="origin key in the acceptance manifests")
    parser.add_argument("--checkout", type=Path, required=True, help="checkout of the pinned revision")
    parser.add_argument("--prefix", required=True, help="case id prefix")
    args = parser.parse_args(argv)

    manifests = {kind: json.loads((ACCEPTANCE / name).read_text(encoding="utf-8"))
                 for kind, name in MANIFESTS.items()}
    visual = json.loads(VISUAL.read_text(encoding="utf-8"))
    origin = manifests["body"]["origins"][args.origin]
    head = subprocess.run(["git", "-C", str(args.checkout), "rev-parse", "HEAD"],
                          capture_output=True, text=True, check=True).stdout.strip()
    if head != origin["revision"]:
        raise SystemExit(f"checkout is at {head}, origin pins {origin['revision']}")
    existing = [c for c in manifests["body"]["cases"]
                if manifests["body"]["files"][c["source"]]["origin"] == args.origin
                and "/pages/" in c["source"]]
    if not existing:
        raise SystemExit("the origin has no page case to take host attributes from")
    attributes = existing[0]["attributes"]
    if any(c["attributes"] != attributes for c in existing):
        raise SystemExit("the origin's pages use different host attributes")

    docs = args.checkout / "docs"
    added = 0
    for page in sorted(docs.glob("modules/*/pages/**/*.adoc")):
        module = page.relative_to(docs / "modules").parts[0]
        relative = page.relative_to(docs / "modules" / module / "pages")
        source = f"sources/{args.origin}/{page.relative_to(args.checkout).as_posix()}"
        # A page another manifest already has keeps its id there.
        identifier = next((c["id"] for c in manifests["body"]["cases"] if c["source"] == source),
                          case_id(args.prefix, module, relative))
        files = {page: None, **{path: target for target, path in
                                resources(docs, module, page.read_text(encoding="utf-8")).items()}}
        names = {}
        for path in files:
            upstream = path.relative_to(args.checkout).as_posix()
            name = f"sources/{args.origin}/{upstream}"
            target = ACCEPTANCE / name
            data = path.read_bytes()
            if target.exists() and target.read_bytes() != data:
                raise SystemExit(f"{name} differs from the checkout")
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
            names[path] = (name, {"sha256": hashlib.sha256(data).hexdigest(),
                                  "origin": args.origin, "upstream_path": upstream})
        case = {"id": identifier, "category": "document", "source": source, "attributes": attributes}
        resource_map = {target: names[path][0] for path, target in files.items() if target is not None}
        if resource_map:
            case["profile"] = "include-body-dom-v1"
            case["resources"] = resource_map
        case["expect"] = "pass"
        is_new = False
        for kind, manifest in manifests.items():
            if any(c["source"] == source for c in manifest["cases"]):
                continue
            is_new = True
            entry = dict(case, id=CASE_PREFIX[kind] + identifier)
            if kind == "semantics":
                entry["semantic_attributes"] = []
            manifest["cases"].append(entry)
            for name, record in names.values():
                manifest["files"].setdefault(name, record)
        visual_source = f"compatibility/acceptance/{source}"
        if not any(c["source"] == visual_source for c in visual["cases"]):
            is_new = True
            entry = {"id": f"acceptance-{identifier}", "source": visual_source, "attributes": attributes}
            if resource_map:
                entry["resources"] = {t: f"compatibility/acceptance/{n}" for t, n in resource_map.items()}
            visual["cases"].append(entry)
        added += is_new
    for kind, manifest in manifests.items():
        dump(ACCEPTANCE / MANIFESTS[kind], manifest, 2)
    dump(VISUAL, visual, 1)
    print(f"{args.origin}: {added} pages added")


if __name__ == "__main__":
    main()
