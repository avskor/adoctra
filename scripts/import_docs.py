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
import posixpath
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


ATTRIBUTE_ENTRY = re.compile(r"^:(!?)([\w-]+)(!?):(?:[ \t]+(.*))?$")


class Attributes:
    """Attribute values while the includes are read: the host attributes (a
    hard one locked, a soft `@` default overridable) and the document's own
    entries in reading order, as Asciidoctor applies them."""

    def __init__(self, host):
        self.values, self.locked = {}, set()
        for name, value in host.items():
            if name.endswith("!"):
                self.locked.add(name.rstrip("!").lower())
                continue
            key = name.rstrip("@").lower()
            self.values[key] = value.removesuffix("@")
            if not value.endswith("@"):
                self.locked.add(key)

    def read(self, line):
        match = ATTRIBUTE_ENTRY.match(line)
        if not match:
            return
        name = match[2].lower()
        if name in self.locked:
            return
        if match[1] or match[3]:
            self.values.pop(name, None)
        else:
            self.values[name] = expand(match[4] or "", self) or ""


def expand(target, attributes):
    """`target` with `{name}` references replaced, or None when one is not
    set. `attributes` is an Attributes or a host attribute dict."""
    if not isinstance(attributes, Attributes):
        attributes = Attributes(attributes)
    missing = []

    def value(match):
        name = match[1].lower()
        if name not in attributes.values:
            missing.append(name)
            return match[0]
        return attributes.values[name]

    expanded = re.sub(r"\{([\w-]+)\}", value, target)
    return None if missing else expanded


def resolve(docs, module, key, collect=()):
    """The file both processors read for the include fixture name `key`
    (already joined to the including file's directory), or None.

    An Antora resource id resolves in its family; a module-qualified id
    (`mod:example$x`) is a URI to Asciidoctor, which neither processor
    reads. A plain path is relative to the module directory, the fixture
    root (`partials/…` with Debezium's `partialsdir`). `collect` maps
    directories an Antora collector copies into the component
    (`(src, into)`, both relative to it), e.g. JUnit's `src/test` into
    `modules/ROOT/examples`."""
    # Ruby's UriSniffRx: a scheme makes the target a URI.
    if key.startswith("/") or ".." in key.split("/") or re.match(r"[A-Za-z][A-Za-z0-9.+-]+:", key):
        return None
    # An id nested below an included file's directory keeps its own meaning.
    nested = re.search(r"(?:^|/)((?:[\w-]+:)?(?:example|partial|page)\$.+)$", key)
    match = RESOURCE_ID.fullmatch(nested[1]) if nested else None
    if match:
        if match[1]:
            return None
        path = docs / "modules" / module / FAMILIES[match[2]] / match[3]
    else:
        path = docs / "modules" / module / key
    if path.is_file():
        return path
    virtual = path.relative_to(docs).as_posix()
    for src, into in collect:
        if virtual.startswith(into.rstrip("/") + "/"):
            collected = docs / src / virtual[len(into.rstrip("/")) + 1:]
            if collected.is_file():
                return collected
    return None


MAX_INCLUDE_DEPTH = 64  # Asciidoctor's default max-include-depth


def resources(docs, module, text, attributes, base="", found=None, collect=(), depth=0):
    """Fixture name -> file for every include of `text` (whose fixture name
    lives in directory `base`), transitively, reading attribute entries in
    order so a target like `frag-{context}.adoc` expands as Asciidoctor would."""
    found = {} if found is None else found
    if not isinstance(attributes, Attributes):
        attributes = Attributes(attributes)
    for line in text.splitlines():
        attributes.read(line)
        include = INCLUDE.match(line)
        if not include:
            continue
        expanded = expand(include[1], attributes)
        if expanded is None:
            continue
        key = posixpath.normpath(posixpath.join(base, expanded)) if base else expanded
        path = found.get(key) or resolve(docs, module, key, collect)
        if path is not None:
            found[key] = path
            if depth < MAX_INCLUDE_DEPTH:
                resources(docs, module, path.read_text(encoding="utf-8"), attributes,
                          posixpath.dirname(key), found, collect, depth + 1)
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
    parser.add_argument("--component", default="docs", help="the Antora component directory")
    parser.add_argument("--collect", action="append", default=[], metavar="SRC=INTO",
                        help="a directory the component's Antora collector copies (relative to it)")
    parser.add_argument("--refresh", action="store_true",
                        help="also recompute the resources of pages the manifests already have")
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

    docs = args.checkout / args.component
    added = 0
    refreshed = set()
    for page in sorted(docs.glob("modules/*/pages/**/*.adoc")):
        module = page.relative_to(docs / "modules").parts[0]
        relative = page.relative_to(docs / "modules" / module / "pages")
        source = f"sources/{args.origin}/{page.relative_to(args.checkout).as_posix()}"
        # A page another manifest already has keeps its id there.
        identifier = next((c["id"] for c in manifests["body"]["cases"] if c["source"] == source),
                          case_id(args.prefix, module, relative))
        collect = [tuple(entry.split("=", 1)) for entry in args.collect]
        found = resources(docs, module, page.read_text(encoding="utf-8"), attributes, collect=collect)
        names = {}
        for path in [page, *found.values()]:
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
        resource_map = {key: names[path][0] for key, path in found.items()}
        if resource_map:
            case["profile"] = "include-body-dom-v1"
            case["resources"] = resource_map
        case["expect"] = "pass"
        is_new = False
        for kind, manifest in manifests.items():
            present = next((c for c in manifest["cases"] if c["source"] == source), None)
            if present is not None:
                if args.refresh and present.get("resources", {}) != resource_map:
                    refreshed.add(present["id"])
                    present.pop("resources", None)
                    present.pop("profile", None)
                    if resource_map:
                        present["resources"] = resource_map
                        present["profile"] = "include-body-dom-v1"
                    # Keep the key order of a new case.
                    order = ["id", "category", "source", "attributes", "profile", "resources", "expect"]
                    ordered = {key: present[key] for key in order if key in present}
                    ordered.update({key: value for key, value in present.items() if key not in ordered})
                    present.clear()
                    present.update(ordered)
                    for name, record in names.values():
                        manifest["files"].setdefault(name, record)
                continue
            is_new = True
            entry = dict(case, id=CASE_PREFIX[kind] + identifier)
            if kind == "semantics":
                entry["semantic_attributes"] = []
            manifest["cases"].append(entry)
            for name, record in names.values():
                manifest["files"].setdefault(name, record)
        visual_source = f"compatibility/acceptance/{source}"
        shown = next((c for c in visual["cases"] if c["source"] == visual_source), None)
        visual_map = {t: f"compatibility/acceptance/{n}" for t, n in resource_map.items()}
        if shown is not None and args.refresh and shown.get("resources", {}) != visual_map:
            refreshed.add(shown["id"])
            shown.pop("resources", None)
            if visual_map:
                shown["resources"] = visual_map
        if shown is None:
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
    if refreshed:
        print("refreshed: " + " ".join(sorted(refreshed)))


if __name__ == "__main__":
    main()
