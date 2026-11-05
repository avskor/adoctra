#!/usr/bin/env python3
"""A stand-in for a diagram tool in diagram-v1: records how it was invoked
(its name, arguments, stdin and working directory, temporary paths
normalized) as a JSON line in $FAKE_DIAGRAM_LOG and writes a fixed result
where the tool would, so Ruby and adoctra can be compared without the
real tool installed.

A code containing FAKE-FAIL exits 1 with a message on stderr; FAKE-EMPTY
exits 0 without writing anything; FAKE-HTML prints an HTML error page on
stdout instead of an image.
"""

import json
import os
import re
import struct
import sys
import zlib

SVG = b'<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><rect width="40" height="30"/></svg>'
# Ruby's Dir::Tmpname (and adoctra's): <prefix><YYYYMMDD>-<pid>-<base36><suffix>.
TMPNAME = re.compile(r"^(.*?)\d{8}-\d+-[0-9a-z]+(.*)$")


def png(width=21, height=13):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    raw = b"".join(b"\x00" + b"\x00" * width for _ in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def gif(width=17, height=11):
    return b"GIF89a" + struct.pack("<HH", width, height) + b"\x00\x00\x00;"


PDF = b"%PDF-1.4\n1 0 obj<</Type/Page/MediaBox[0 0 50 40]>>endobj\n%%EOF\n"


def content(path_or_format):
    ext = path_or_format.rsplit(".", 1)[-1].lower()
    return {"svg": SVG, "png": png(), "gif": gif(), "pdf": PDF, "jpeg": b"\xff\xd8fake",
            "txt": b"fake text\n", "dot": b"digraph{a->b}\n"}.get(ext, SVG)


def main():
    tool = os.environ.get("FAKE_TOOL_NAME") or os.path.basename(sys.argv[0])
    args = sys.argv[1:]
    tmp = os.path.realpath(os.environ.get("TMPDIR", "/tmp"))
    stdin = "" if sys.stdin.isatty() else sys.stdin.buffer.read().decode("utf-8", "replace")

    def normalize(text):
        def tmp_path(match):
            name = os.path.basename(match.group(0))
            m = TMPNAME.match(name)
            return f"<tmp>/{m.group(1)}*{m.group(2)}" if m else f"<tmp>/{name}"
        return re.sub(re.escape(tmp) + r"/[^\s\"',]*", tmp_path, text)

    inputs = {}
    for arg in args:
        at = arg.find(tmp + "/")
        if at >= 0 and os.path.isfile(arg[at:]) and os.path.getsize(arg[at:]):
            with open(arg[at:], "rb") as file:
                inputs[normalize(arg[at:])] = file.read().decode("utf-8", "replace")
    entry = {"tool": tool, "args": [normalize(a) for a in args], "stdin": normalize(stdin),
             "inputs": inputs, "cwd": os.getcwd()}
    with open(os.environ["FAKE_DIAGRAM_LOG"], "a", encoding="utf-8") as log:
        log.write(json.dumps(entry, ensure_ascii=False, sort_keys=True) + "\n")

    code = stdin + "".join(inputs.values())
    if "FAKE-FAIL" in code:
        sys.stderr.write(f"{tool}: fake failure\n")
        return 1
    if "FAKE-EMPTY" in code:
        return 0
    if "FAKE-HTML" in code:
        sys.stdout.write("<div>\nERROR: fake <b>bad</b> input\n</div>\n")
        return 0

    outputs = []
    for arg in args:
        at = arg.find(tmp + "/")
        if at >= 0:
            path = arg[at:]
            if not os.path.isfile(path) or not os.path.getsize(path):
                outputs.append(path)
    if tool == "lilypond":
        out = args[args.index("-o") + 1]
        outputs = [f"{out}.cropped.{args[args.index('-f') + 1]}"]
    elif tool == "pdflatex":
        source = args[-1]
        outputs = [os.path.join(args[args.index("-output-directory") + 1],
                                os.path.splitext(os.path.basename(source))[0] + ".pdf")]
    elif tool in ("python3", "python"):
        m = re.search(r'filename="([^"]+)",outformat="([^"]+)"', stdin)
        outputs = [f"{m.group(1)}.{m.group(2)}"] if m else []
    for path in outputs:
        data = b"digraph{a->b}\n" if tool in ("erd", "erd-go") else content(path)
        with open(path, "wb") as file:
            file.write(data)

    if tool == "vl2vg":
        sys.stdout.write('{"$schema": "vega", "converted": true}\n')
    elif tool == "gnuplot":
        term = re.match(r"set term (\w+)", stdin)
        sys.stdout.buffer.write(content(term.group(1) if term and term.group(1) != "dumb" else "txt"))
    elif tool == "vg2png":
        sys.stdout.buffer.write(png())
    elif not outputs:
        sys.stdout.buffer.write(SVG)
    return 0


if __name__ == "__main__":
    sys.exit(main())
