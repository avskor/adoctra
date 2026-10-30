#!/usr/bin/env python3
"""Compare the complete rendered body with pinned Asciidoctor fixtures.

body-dom-v1 is an explicit HTML projection, not an ASG or exact-HTML oracle.
Only known presentation wrappers are reconciled; unknown markup survives.
"""

import argparse
from collections import Counter
from contextlib import contextmanager
from dataclasses import dataclass, field
import difflib
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "compatibility"
VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"}
INLINE = {"p", "a", "strong", "em", "code", "mark", "sup", "sub", "span", "cite", "h1", "h2", "h3", "h4", "h5", "h6"}
LIST_BLOCKS = {"ul", "ol", "dl", "p", "listing", "literal", "pre", "div", "aside", "figure", "blockquote", "table", "hr", "details",
               "quote", "verse", "admonition"}


@dataclass
class Element:
    tag: str
    attrs: dict = field(default_factory=dict)
    children: list = field(default_factory=list)


class BodyParser(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.root = Element("root")
        self.stack = [self.root]

    def handle_starttag(self, tag, attrs):
        node = Element(tag, dict(attrs))
        self.stack[-1].children.append(node)
        if tag not in VOID:
            self.stack.append(node)

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)
        if tag not in VOID:
            self.handle_endtag(tag)

    def handle_endtag(self, tag):
        if len(self.stack) == 1 or self.stack[-1].tag != tag:
            raise ValueError(f"unbalanced HTML closing tag: {tag}")
        self.stack.pop()

    def handle_data(self, data):
        self.stack[-1].children.append(data)

    def handle_comment(self, data):
        # Raw comments in passthrough content are observable output too.
        self.stack[-1].children.append(Element("#comment", {"text": data}))


def compact(children, prose=False, preserve=False):
    result = []
    for child in children:
        if isinstance(child, str):
            if not preserve:
                child = re.sub(r"[ \t\r\n\f]+", " ", child)
            if not child or (not prose and not preserve and child == " "):
                continue
            if result and isinstance(result[-1], str):
                result[-1] += child
            else:
                result.append(child)
        else:
            result.append(child)
    return result


def trim_edges(children):
    if children and isinstance(children[0], str):
        children[0] = children[0].lstrip(" ")
    if children and isinstance(children[-1], str):
        children[-1] = children[-1].rstrip(" ")
    return [child for child in children if child != ""]


def hdlist(node):
    attrs = dict(node.attrs)
    classes = [c for c in attrs.get("class", "").split() if c != "hdlist"]
    attrs.pop("class", None)
    if classes:
        attrs["class"] = " ".join(classes)
    items = []
    for table in (c for c in node.children if isinstance(c, Element) and c.tag == "table"):
        rows = [r for c in table.children if isinstance(c, Element)
                for r in ([c] if c.tag == "tr" else c.children) if isinstance(r, Element) and r.tag == "tr"]
        for row in rows:
            for cell in (c for c in row.children if isinstance(c, Element) and c.tag == "td"):
                cell_classes = cell.attrs.get("class", "").split()
                if "hdlist1" in cell_classes:
                    dt_attrs = {"class": "strong"} if "strong" in cell_classes else {}
                    term = []
                    for child in cell.children:
                        if isinstance(child, Element) and child.tag == "br":
                            items.append(Element("dt", dict(dt_attrs), term))
                            term = []
                        else:
                            term.append(child)
                    items.append(Element("dt", dict(dt_attrs), term))
                else:
                    items.append(Element("dd", {}, cell.children))
    return Element("dl", attrs, items)


def colist_table(node):
    """Ruby's callout list with icons is a layout table in div.colist: each
    row is an item, its first cell the icon, the second the text. Project it
    to the shared ol > li > (colist-icon, colist-text) form."""
    items = []
    for table in (c for c in node.children if isinstance(c, Element) and c.tag == "table"):
        rows = [r for c in table.children if isinstance(c, Element)
                for r in ([c] if c.tag == "tr" else c.children) if isinstance(r, Element) and r.tag == "tr"]
        for row in rows:
            cells = [c for c in row.children if isinstance(c, Element) and c.tag == "td"]
            if len(cells) != 2:
                return node
            items.append(Element("li", {}, [Element("colist-icon", {}, cells[0].children),
                                            Element("colist-text", {}, cells[1].children)]))
    return Element("ol", node.attrs, items)


def colist_icons(node):
    """adoctra's callout list with icons: each item's icon <span> and text
    <div> (laid out like Ruby's table by the stylesheet) in the shared form."""
    attrs = dict(node.attrs)
    attrs["class"] = " ".join(c for c in attrs["class"].split() if c != "icons")
    items = []
    for item in node.children:
        if isinstance(item, Element) and item.tag == "li":
            parts = []
            for child in item.children:
                if isinstance(child, Element) and child.tag == "span" and child.attrs == {"class": "colist-icon"}:
                    parts.append(Element("colist-icon", {}, child.children))
                elif isinstance(child, Element) and child.tag == "div" and not child.attrs:
                    parts.append(Element("colist-text", {}, child.children))
                else:
                    parts.append(child)
            item = Element("li", item.attrs, parts)
        items.append(item)
    return Element("ol", attrs, items)


def admonition(node):
    """Ruby's admonitionblock table and adoctra's <aside class="admonition">
    project to one admonition node: type, id, roles, label, then the body."""
    attrs = {k: v for k, v in node.attrs.items() if k in {"id"}}
    if node.tag == "aside":
        attrs["type"] = node.attrs.get("data-type", "")
        roles = [c for c in node.attrs.get("class", "").split() if c != "admonition"]
        children = [c for c in node.children if not (isinstance(c, str) and not c.strip())]
        label = []
        if children and isinstance(children[0], Element) and children[0].tag == "p" \
                and children[0].attrs.get("class") == "admonition-title":
            label, children = children[0].children, children[1:]
        # The content wrapper is Ruby's content cell.
        if len(children) == 1 and isinstance(children[0], Element) and children[0].tag == "div" \
                and children[0].attrs.get("class") == "admonition-content":
            children = children[0].children
    else:
        classes = node.attrs.get("class", "").split()
        attrs["type"] = classes[1] if len(classes) > 1 else ""
        roles = classes[2:]
        cells = [cell for table in node.children if isinstance(table, Element) and table.tag == "table"
                 for row in (r for c in table.children if isinstance(c, Element)
                             for r in ([c] if c.tag == "tr" else c.children)
                             if isinstance(r, Element) and r.tag == "tr")
                 for cell in row.children if isinstance(cell, Element) and cell.tag == "td"]
        icon = next((c for c in cells if c.attrs.get("class") == "icon"), None)
        content = next((c for c in cells if c.attrs.get("class") == "content"), None)
        if icon is None or content is None:
            return node
        label = icon.children
        titles = [c for c in label if not (isinstance(c, str) and not c.strip())]
        if len(titles) == 1 and isinstance(titles[0], Element) and titles[0].tag == "div" \
                and titles[0].attrs.get("class") == "title":
            label = titles[0].children
        children = content.children
    if roles:
        attrs["class"] = " ".join(roles)
    return Element("admonition", attrs, [Element("admonition-label", {}, label)] + list(children))


def element(tag, attrs, children):
    return {"tag": tag, "attrs": dict(sorted(attrs.items())), "children": children}


def normalize(node, verbatim=False, block_kind=None):
    if isinstance(node, str):
        return [node]
    if node.tag == "section":
        # adoctra's semantic <section> for sections and the preamble is the same
        # structure as Ruby's <div>; the sectN/sectionbody wrappers drop below.
        node = Element("div", node.attrs, node.children)
    if node.tag == "nav":
        # adoctra's TOC <nav> is Ruby's <div id="toc">.
        node = Element("div", node.attrs, node.children)
    if node.tag == "figure" and {"imageblock", "videoblock", "audioblock"} & set(node.attrs.get("class", "").split()):
        # adoctra's media <figure> is Ruby's imageblock/videoblock/audioblock
        # <div>, whose player sits in a content wrapper that drops below.
        node = Element("div", node.attrs, node.children)
    if node.tag == "aside" and "sidebarblock" in node.attrs.get("class", "").split():
        # adoctra's sidebar <aside> is Ruby's sidebarblock <div>, whose
        # children sit in a content wrapper that drops below.
        node = Element("div", node.attrs, node.children)
    if (node.tag == "aside" and "admonition" in node.attrs.get("class", "").split()) \
            or (node.tag == "div" and node.attrs.get("class", "").split()[:1] == ["admonitionblock"]):
        node = admonition(node)
    if node.tag == "figcaption" and node.attrs.get("class") == "title":
        # A media <figcaption> is Ruby's title <div> (trailing for images,
        # leading for video and audio).
        node = Element("div", node.attrs, node.children)
    if node.tag == "div" and "hdlist" in node.attrs.get("class", "").split():
        # Ruby's horizontal description list is a layout table; adoctra's is a
        # <dl class="hdlist">. Rows become terms (split at <br>) and descriptions.
        node = hdlist(node)
    if node.tag == "dl" and "hdlist" in node.attrs.get("class", "").split():
        attrs = {k: v for k, v in node.attrs.items() if k != "style"}
        classes = [c for c in attrs.get("class", "").split() if c != "hdlist"]
        attrs.pop("class", None)
        if classes:
            attrs["class"] = " ".join(classes)
        node = Element("dl", attrs, node.children)
    if node.tag == "iframe" and node.attrs.get("frameborder") == "0":
        # Ruby's obsolete frameborder="0" is adoctra's stylesheet border.
        node = Element("iframe", {k: v for k, v in node.attrs.items() if k != "frameborder"}, node.children)
    if node.tag == "div" and "colist" in node.attrs.get("class", "").split() \
            and any(isinstance(c, Element) and c.tag == "table" for c in node.children):
        node = colist_table(node)
    if node.tag == "ol" and {"colist", "icons"} <= set(node.attrs.get("class", "").split()):
        node = colist_icons(node)
    if node.tag == "ol" and "colist" in node.attrs.get("class", "").split():
        # adoctra's callout list is Ruby's ordered list inside div.colist.
        attrs = dict(node.attrs)
        classes = [c for c in attrs["class"].split() if c not in {"colist", "arabic"}]
        attrs.pop("class")
        if classes:
            attrs["class"] = " ".join(classes)
        node = Element("ol", attrs, node.children)
    tag, attrs = node.tag, dict(node.attrs)
    classes = attrs.get("class", "").split()
    wrapper = None
    # Quote and verse blocks: Ruby's div/attribution layout and adoctra's
    # figure/figcaption idiom project to the same named nodes.
    kind = next((c for c in classes if c in {"quoteblock", "verseblock"}), None)
    if tag in {"div", "figure"} and kind:
        tag = "quote" if kind == "quoteblock" else "verse"
        classes.remove(kind)
        block_kind = tag if tag == "verse" else None
        if classes:
            attrs["class"] = " ".join(classes)
        else:
            attrs.pop("class", None)
    elif tag == "figcaption" or (tag == "div" and classes == ["attribution"]):
        tag = "attribution"
        attrs.pop("class", None)
        classes = []
    if tag == "div":
        wrapper = next((c for c in classes if c in {
            "paragraph", "ulist", "olist", "dlist", "qlist", "colist", "listingblock", "literalblock",
            "content", "sectionbody", "preamble",
        } or re.fullmatch(r"sect[1-6]", c)), None)
        if wrapper in {"listingblock", "literalblock"}:
            block_kind = "listing" if wrapper == "listingblock" else "literal"

    if tag == "pre" and block_kind == "verse":
        classes = [c for c in classes if c not in {"content", "verse"}]
        block_kind = "pre"
    if tag == "pre":
        if "literalblock" in classes:
            block_kind = "literal"
            classes.remove("literalblock")
        elif "listing" in classes:
            block_kind = "listing"
            classes.remove("listing")
        elif "highlight" in classes:
            block_kind = "listing"
        tag = block_kind or "pre"
        verbatim = True
        if classes:
            attrs["class"] = " ".join(classes)
        else:
            attrs.pop("class", None)

    children = []
    for child in node.children:
        children.extend(normalize(child, verbatim, block_kind))

    if node.tag == "li":
        # Ruby wraps the principal item text in <p>; adoctra writes it directly.
        children = [c for i, c in enumerate(children) if not (
            isinstance(c, str) and not c.strip() and (
                i == 0 or i == len(children) - 1
                or isinstance(children[i - 1], dict) and children[i - 1]["tag"] in LIST_BLOCKS
                or isinstance(children[i + 1], dict) and children[i + 1]["tag"] in LIST_BLOCKS
            )
        )]
        if children and isinstance(children[0], dict) and children[0]["tag"] == "p" and not children[0]["attrs"]:
            children = children[0]["children"] + children[1:]

    if tag == "blockquote" and any(isinstance(c, str) and c.strip() for c in children):
        # A quoted paragraph's simple content is prose.
        children = trim_edges(compact(children, True))
    is_prose = tag in INLINE or tag in {"li", "attribution"} or "title" in classes
    children = compact(children, is_prose, verbatim)
    if tag == "li":
        # A line break before an attached block is layout, not item text.
        for i, child in enumerate(children):
            if isinstance(child, dict) and child["tag"] in LIST_BLOCKS:
                if i and isinstance(children[i - 1], str):
                    children[i - 1] = children[i - 1].rstrip(" ")
    if tag in {"p", "li", "dt", "attribution", "h1", "h2", "h3", "h4", "h5", "h6"} or "title" in classes:
        children = trim_edges(children)

    if wrapper:
        classes.remove(wrapper)
        # Asciidoctor's default ordered-list style is presentation metadata.
        if wrapper in {"olist", "colist"} and "arabic" in classes:
            classes.remove("arabic")
        attrs.pop("class", None)
        if classes:
            attrs["class"] = " ".join(classes)
        if attrs:
            # Transfer wrapper id/roles only if its principal element is unambiguous.
            targets = [c for c in children if isinstance(c, dict) and c["attrs"].get("class") != "title"]
            if len(targets) != 1:
                return [element(node.tag, node.attrs, children)]
            target = targets[0]
            for key, value in attrs.items():
                if key == "class" and key in target["attrs"]:
                    merged = target["attrs"][key].split()
                    merged += [c for c in value.split() if c not in merged]
                    target["attrs"][key] = " ".join(merged)
                elif key in target["attrs"]:
                    raise ValueError(f"ambiguous wrapper attribute {key}")
                else:
                    target["attrs"][key] = value
        return children
    if tag == "ol" and "arabic" in classes:
        # The default style is presentation metadata, beside roles too
        # (adoctra puts a list's roles on the <ol>, Ruby on its wrapper).
        rest = [c for c in classes if c != "arabic"]
        attrs.pop("class")
        if rest:
            attrs["class"] = " ".join(rest)
    return [element(tag, attrs, children)]


def projection(html):
    parser = BodyParser()
    parser.feed(html)
    parser.close()
    if len(parser.stack) != 1:
        raise ValueError("unclosed HTML element")
    roots = parser.root.children
    documents = [n for n in roots if isinstance(n, Element) and n.tag == "html"]
    if documents:
        bodies = [n for n in documents[0].children if isinstance(n, Element) and n.tag == "body"]
        if len(bodies) != 1:
            raise ValueError("expected exactly one body")
        roots = bodies[0].children
    return compact([result for node in roots for result in normalize(node)])


def run(command, source):
    result = subprocess.run(command, input=source, text=True, encoding="utf-8", capture_output=True, timeout=30)
    if result.returncode:
        raise RuntimeError(f"{' '.join(map(str, command))}: {result.stderr.strip()}")
    return result.stdout


def validate_resources(case):
    binaries = case.get("binary_resources", {})
    if not isinstance(binaries, dict):
        raise ValueError("binary_resources must be a hex-encoded resource map")
    if "resources" not in case:
        if case.get("profile", "body-dom-v1") != "body-dom-v1" or binaries:
            raise ValueError("include profile requires resources")
        return
    if case.get("profile") != "include-body-dom-v1" or not isinstance(case["resources"], dict):
        raise ValueError("resources require include-body-dom-v1")
    if set(binaries) & set(case["resources"]):
        raise ValueError("text and binary fixture resource names overlap")
    for name, text in {**case["resources"], **binaries}.items():
        if (not isinstance(name, str) or not name or name == "main.adoc"
                or name.startswith("/") or any(part in {"", ".", ".."} for part in name.split("/"))
                or any(char in name for char in "\\:\0") or not isinstance(text, str)):
            raise ValueError(f"invalid fixture resource: {name!r}")
        if name in binaries and re.fullmatch(r"(?:[0-9a-fA-F]{2})*", text) is None:
            raise ValueError(f"invalid binary fixture hex: {name!r}")


def validate_attributes(case):
    """Host attributes, passed to both processors as `name=value`."""
    attributes = case.get("attributes", {})
    if not isinstance(attributes, dict) or not all(
            isinstance(name, str) and re.fullmatch(r"(?:!?[A-Za-z0-9_][A-Za-z0-9_-]*|[A-Za-z0-9_][A-Za-z0-9_-]*!)@?", name)
            and isinstance(value, str) for name, value in attributes.items()):
        raise ValueError(f"invalid host attributes: {case.get('id')}")


@contextmanager
def case_commands(case, binary):
    validate_resources(case)
    validate_attributes(case)
    reference = ["ruby", str(ROOT / "scripts/asciidoctor-reference.rb"), "--showtitle"]
    # Like the reference: secure mode, safe mode for include fixtures.
    actual = [str(binary.resolve()), "-", "--to", "html", "--embedded", "-a", "showtitle",
              "-S", "secure", "--stdout"]
    for name, value in case.get("attributes", {}).items():
        reference += ["--attribute", f"{name}={value}"]
        actual += ["-a", f"{name}={value}"]
    if "resources" not in case:
        yield reference, actual
        return
    with tempfile.TemporaryDirectory(prefix="adoctra-include-") as directory:
        root = Path(directory)
        (root / "main.adoc").write_text(case["source"], encoding="utf-8", newline="")
        for name, text in case["resources"].items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8", newline="")
        for name, encoded in case.get("binary_resources", {}).items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(bytes.fromhex(encoded))
        reference += ["--include-root", directory]
        actual[1] = str(root / "main.adoc")
        actual[actual.index("secure")] = "safe"
        yield reference, actual


def load_manifest():
    manifest = json.loads((CORPUS / "cases.json").read_text(encoding="utf-8"))
    version = (CORPUS / "asciidoctor-version").read_text().strip()
    if manifest["asciidoctor"] != version or manifest["profile"] != "body-dom-v1":
        raise ValueError("manifest reference version/profile mismatch")
    ids = set()
    if not manifest["cases"]:
        raise ValueError("empty compatibility corpus")
    for case in manifest["cases"]:
        if not re.fullmatch(r"[a-z0-9_-]+", case["id"]) or case["id"] in ids:
            raise ValueError(f"invalid or duplicate case id: {case['id']}")
        ids.add(case["id"])
        validate_resources(case)
        validate_attributes(case)
        if case["expect"] not in {"pass", "xfail"} or not isinstance(case["source"], str):
            raise ValueError(f"invalid case: {case['id']}")
        if case["expect"] == "xfail" and not case.get("reason"):
            raise ValueError(f"xfail needs a reason: {case['id']}")
    return manifest


def classify(case, actual, expected, baseline=None):
    if actual == expected:
        return "PASS" if case["expect"] == "pass" else "XPASS"
    if case["expect"] == "xfail" and actual == baseline:
        return "XFAIL"
    return "FAIL"


def pretty(value):
    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/adoctra")
    actions = parser.add_mutually_exclusive_group()
    actions.add_argument("--verify-reference", action="store_true")
    actions.add_argument("--update-reference", action="store_true")
    actions.add_argument("--record-known-differences", action="store_true",
                         help="explicitly accept current output of documented xfails; review the diff")
    parser.add_argument("--json-report", type=Path)
    args = parser.parse_args()
    manifest = load_manifest()
    results = []
    for case in manifest["cases"]:
        reference_path = CORPUS / "reference" / (case["id"] + ".html")
        baseline_path = CORPUS / "baseline" / (case["id"] + ".json")
        with case_commands(case, args.binary) as (reference_command, actual_command):
            if args.verify_reference or args.update_reference:
                reference = run(reference_command, case["source"])
                if args.update_reference:
                    projection(reference)  # Reject malformed fixtures before storing them.
                    reference_path.parent.mkdir(exist_ok=True)
                    reference_path.write_text(reference, encoding="utf-8")
                elif reference != reference_path.read_text(encoding="utf-8"):
                    raise ValueError(f"reference drift: {case['id']}; review before --update-reference")
            if args.update_reference:
                continue
            expected = projection(reference_path.read_text(encoding="utf-8"))
            actual = projection(run(actual_command, case["source"]))
        baseline = None
        if case["expect"] == "xfail":
            if args.record_known_differences:
                if actual == expected:
                    raise ValueError(f"{case['id']} is fixed; change its expectation to pass")
                baseline_path.parent.mkdir(exist_ok=True)
                baseline_path.write_text(pretty(actual), encoding="utf-8")
            baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
        status = classify(case, actual, expected, baseline)
        results.append({"id": case["id"], "category": case["category"], "status": status,
                        "reason": case.get("reason")})
        print(f"{status:5} {case['id']}" + (f" — {case['reason']}" if case.get("reason") else ""))
        if status == "FAIL":
            comparison = baseline if baseline is not None else expected
            print("".join(difflib.unified_diff(pretty(comparison).splitlines(True), pretty(actual).splitlines(True),
                                            fromfile="baseline" if baseline is not None else "asciidoctor",
                                            tofile="adoctra")))
    if args.update_reference:
        print(f"Updated {len(manifest['cases'])} references using Asciidoctor {manifest['asciidoctor']}.")
        return 0
    counts = dict(Counter(r["status"] for r in results))
    print(json.dumps(counts, sort_keys=True))
    if args.json_report:
        args.json_report.write_text(pretty({"profile": manifest["profile"], "asciidoctor": manifest["asciidoctor"],
                                           "counts": counts, "cases": results}), encoding="utf-8")
    return int(any(r["status"] in {"FAIL", "XPASS"} for r in results))


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.TimeoutExpired) as error:
        print(f"compatibility harness error: {error}", file=sys.stderr)
        sys.exit(2)
