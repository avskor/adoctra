#!/usr/bin/env python3
"""Check that .docx files are valid packages Word-compatible readers accept.

Standard library only. For every file:

* every part is well-formed XML (or a media type: images, SVG included,
  are the document's own files copied byte for byte, so a malformed SVG
  is the author's, and Word opens such a package with the PNG fallback);
* every part is declared in [Content_Types].xml (Default or Override) and
  every Override names an existing part;
* every internal relationship target exists, relationship ids are unique;
* the main document has a w:body with a final w:sectPr, and every pStyle,
  rStyle and tblStyle it uses is defined in styles.xml.

With --render, each file is also converted to PDF by headless LibreOffice
(`soffice`, or the path in $SOFFICE); a failed or empty conversion is an
error. Exit status 1 lists every problem.

With --corpus, every .adoc under the given directories is first converted
by the CLI (--cli, default target/debug/adoctra) as both an article and a
book; a failed conversion is a problem too. CI runs this on the golden and
acceptance corpora.

    python3 scripts/docx_check.py out/*.docx [--render]
    python3 scripts/docx_check.py --corpus crates/adoctra-html/tests/golden
"""

import argparse
import concurrent.futures
import os
import posixpath
import shutil
import subprocess
import sys
import tempfile
import zipfile
from xml.etree import ElementTree

W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
CT = "{http://schemas.openxmlformats.org/package/2006/content-types}"
REL = "{http://schemas.openxmlformats.org/package/2006/relationships}"
OFFICE_DOCUMENT = (
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
)
STYLES = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles"
BINARY = {"png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "emf", "wmf", "bin", "svg"}


def rels_name(part):
    head, tail = posixpath.split(part)
    return posixpath.join(head, "_rels", tail + ".rels")


def resolve(source, target):
    if target.startswith("/"):
        return target[1:]
    return posixpath.normpath(posixpath.join(posixpath.dirname(source), target))


def check(path):
    problems = []
    try:
        zf = zipfile.ZipFile(path)
    except zipfile.BadZipFile as e:
        return [f"not a zip file: {e}"]
    names = [n for n in zf.namelist() if not n.endswith("/")]
    trees = {}
    for name in names:
        ext = name.rsplit(".", 1)[-1].lower()
        if ext in BINARY:
            continue
        try:
            trees[name] = ElementTree.fromstring(zf.read(name))
        except ElementTree.ParseError as e:
            problems.append(f"{name}: malformed XML: {e}")
    types = trees.get("[Content_Types].xml")
    if types is None:
        return problems + ["no [Content_Types].xml"]
    defaults = {d.get("Extension").lower() for d in types.iter(CT + "Default")}
    overrides = {o.get("PartName").lstrip("/") for o in types.iter(CT + "Override")}
    for name in names:
        if name == "[Content_Types].xml":
            continue
        if name not in overrides and name.rsplit(".", 1)[-1].lower() not in defaults:
            problems.append(f"{name}: no content type")
    for part in sorted(overrides - set(names)):
        problems.append(f"Override for missing part {part}")
    main = None
    for name in names:
        if not name.endswith(".rels") or name not in trees:
            continue
        source = name.replace("_rels/", "")[: -len(".rels")]
        if name == "_rels/.rels":
            source = ""
        ids = set()
        for rel in trees[name].iter(REL + "Relationship"):
            rid = rel.get("Id")
            if rid in ids:
                problems.append(f"{name}: duplicate relationship id {rid}")
            ids.add(rid)
            if rel.get("TargetMode") == "External":
                continue
            target = resolve(source, rel.get("Target"))
            if target not in names:
                problems.append(f"{name}: {rid} targets missing part {target}")
            if source == "" and rel.get("Type") == OFFICE_DOCUMENT:
                main = target
    if main is None or main not in trees:
        return problems + ["no main document part"]
    body = trees[main].find(W + "body")
    if body is None:
        return problems + [f"{main}: no w:body"]
    if len(body) == 0 or body[-1].tag != W + "sectPr":
        problems.append(f"{main}: w:body does not end with w:sectPr")
    styles_part = None
    rels = trees.get(rels_name(main))
    if rels is not None:
        for rel in rels.iter(REL + "Relationship"):
            if rel.get("Type") == STYLES:
                styles_part = resolve(main, rel.get("Target"))
    defined = set()
    if styles_part in trees:
        defined = {s.get(W + "styleId") for s in trees[styles_part].iter(W + "style")}
    for tag in ("pStyle", "rStyle", "tblStyle"):
        for el in trees[main].iter(W + tag):
            if el.get(W + "val") not in defined:
                problems.append(f"{main}: undefined {tag} {el.get(W + 'val')}")
    return problems


def render(path, soffice):
    with tempfile.TemporaryDirectory() as out:
        src = shutil.copy(path, out)
        result = subprocess.run(
            [soffice, "--headless", "--convert-to", "pdf", "--outdir", out, src],
            capture_output=True,
            text=True,
            timeout=180,
        )
        pdf = os.path.splitext(src)[0] + ".pdf"
        if result.returncode != 0 or not os.path.exists(pdf) or os.path.getsize(pdf) == 0:
            return [f"LibreOffice conversion failed: {result.stderr.strip() or result.stdout.strip()}"]
    return []


DOCTYPES = ("article", "book")


def corpus_inputs(dirs):
    return sorted(
        os.path.join(root, name)
        for top in dirs
        for root, _, files in os.walk(top)
        for name in files
        if name.endswith(".adoc")
    )


def convert(cli, source, doctype, out):
    """Convert one input; returns the problems of the conversion itself."""
    try:
        result = subprocess.run(
            [cli, source, "--to", "docx", "-a", f"doctype={doctype}", "-o", out, "-S", "unsafe"],
            capture_output=True,
            text=True,
            timeout=120,
        )
    except subprocess.TimeoutExpired:
        return ["conversion timed out"]
    if result.returncode != 0 or not os.path.exists(out):
        lines = result.stderr.strip().splitlines()
        return [f"conversion failed ({result.returncode}): {lines[-1] if lines else ''}"]
    return []


def check_corpus(cli, dirs, render_with=None):
    """Convert and check every input of `dirs`; returns (count, problems)."""
    jobs = [(src, dt) for src in corpus_inputs(dirs) for dt in DOCTYPES]
    with tempfile.TemporaryDirectory() as out:

        def one(job):
            index, (source, doctype) = job
            target = os.path.join(out, f"{index}.docx")
            problems = convert(cli, source, doctype, target) or check(target)
            if render_with and not problems:
                problems = render(target, render_with)
            return [f"{source} ({doctype}): {p}" for p in problems]

        with concurrent.futures.ThreadPoolExecutor(os.cpu_count() or 2) as pool:
            found = [p for ps in pool.map(one, enumerate(jobs)) for p in ps]
    return len(jobs), found


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("files", nargs="*")
    parser.add_argument("--render", action="store_true", help="convert to PDF with LibreOffice")
    parser.add_argument("--corpus", nargs="+", metavar="DIR", help="convert every .adoc under DIR")
    parser.add_argument("--cli", default=os.path.join("target", "debug", "adoctra"))
    args = parser.parse_args()
    if not args.files and not args.corpus:
        parser.error("give .docx files or --corpus DIR")
    soffice = os.environ.get("SOFFICE") or shutil.which("soffice") or shutil.which("libreoffice")
    if args.render and not soffice:
        parser.error("--render needs soffice on PATH or $SOFFICE")
    failed = False
    for path in args.files:
        problems = check(path)
        if args.render and not problems:
            problems = render(path, soffice)
        for problem in problems:
            print(f"{path}: {problem}")
        failed |= bool(problems)
        if not problems:
            print(f"{path}: ok")
    if args.corpus:
        count, problems = check_corpus(args.cli, args.corpus, soffice if args.render else None)
        for problem in problems:
            print(problem)
        print(f"corpus: {count} packages, {len(problems)} problems")
        failed |= bool(problems)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
