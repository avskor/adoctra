import base64
import json
import os
import subprocess
from pathlib import Path
import tempfile
import types
import unittest
import urllib.request
import zlib

import diagram


def kroki_path(code):
    return base64.urlsafe_b64encode(zlib.compress(code.encode(), 9)).decode()


class DecodeTests(unittest.TestCase):
    def test_kroki_paths_are_zlib_base64(self):
        self.assertEqual(diagram.decode_request("/graphviz/svg/" + kroki_path("digraph{a}")), "digraph{a}")

    def test_plantuml_paths_use_plantumls_alphabet(self):
        compressor = zlib.compressobj(9, zlib.DEFLATED, -15)
        raw = compressor.compress(b"@startuml\nA -> B\n@enduml") + compressor.flush()
        data = base64.urlsafe_b64encode(raw).decode().translate(str.maketrans(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"))
        self.assertEqual(diagram.decode_request("/puml/svg/" + data), "@startuml\nA -> B\n@enduml")


class ServerTests(unittest.TestCase):
    def test_requests_are_recorded_and_failures_answered(self):
        server = diagram.FakeServer()
        try:
            body = urllib.request.urlopen(f"{server.url}/graphviz/svg/{kroki_path('x')}").read()
            self.assertEqual(body, diagram.SERVER_SVG)
            request = urllib.request.Request(f"{server.url}/plantuml/svg", data=b"code",
                                             headers={"Content-Type": "text/plain; charset=utf-8"})
            urllib.request.urlopen(request).read()
            with self.assertRaises(urllib.error.HTTPError):
                urllib.request.urlopen(f"{server.url}/fail/graphviz/svg/{kroki_path('y')}")
            self.assertEqual(server.take(), [
                {"method": "GET", "path": "/graphviz/svg", "code": "x"},
                {"method": "POST", "path": "/plantuml/svg", "type": "text/plain; charset=utf-8", "code": "code"},
                {"method": "GET", "path": "/fail/graphviz/svg", "code": "y"},
            ])
            self.assertEqual(server.take(), [])
        finally:
            server.close()


class CaseTests(unittest.TestCase):
    def test_missing_tools_skip_instead_of_passing(self):
        args = types.SimpleNamespace(mmdc="", classpath="", binary=Path("/nonexistent"))
        status, details = diagram.check({"name": "m", "source": "", "requires": ["mmdc"]}, args, None)
        self.assertEqual((status, details), ("SKIP", {"missing": ["mmdc"]}))
        self.assertEqual(diagram.available(["plantuml", "server"], args), ["plantuml"])

    def test_created_files_compare_by_digest(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            (root / "d.adoc").write_text("x")
            (root / "out.html").write_text("<p>")
            (root / ".asciidoctor" / "diagram").mkdir(parents=True)
            (root / ".asciidoctor" / "diagram" / "a.png.cache").write_text("{}")
            (root / "a.png").write_bytes(b"\x89PNG")
            view = diagram.files_view(root, {"d.adoc": "x"})
            self.assertEqual(sorted(view), [".asciidoctor/diagram/a.png.cache", "a.png"])
            self.assertEqual(len(view["a.png"]), 64)

    def test_fake_tools_record_their_invocation_and_write_results(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch) / "r"
            root.mkdir()
            env, log = diagram.fake_tools({"fake_tools": ["d2", "gnuplot"]}, root, dict(os.environ))
            tmp = Path(env["TMPDIR"])
            target = tmp / "d220261105-42-abc.svg"
            subprocess.run(["d2", "--browser", "false", "-", str(target)], input=b"a -> b",
                           env=env, cwd=root, check=True)
            self.assertTrue(target.read_bytes().startswith(b"<svg"))
            out = subprocess.run(["gnuplot"], input=b"set term dumb\nplot x\n", env=env, cwd=root,
                                 capture_output=True, check=True).stdout
            self.assertEqual(out, b"fake text\n")
            failed = subprocess.run(["d2", "-"], input=b"FAKE-FAIL", env=env, cwd=root, capture_output=True)
            self.assertEqual((failed.returncode, failed.stderr), (1, b"d2: fake failure\n"))
            entries = [json.loads(line) for line in log.read_text().splitlines()]
            self.assertEqual(entries[0], {"tool": "d2", "args": ["--browser", "false", "-", "<tmp>/d2*.svg"],
                                          "stdin": "a -> b", "inputs": {}, "cwd": str(root)})
            self.assertEqual([e["tool"] for e in entries], ["d2", "gnuplot", "d2"])
            # Without fake tools only TMPDIR is set.
            other = Path(scratch) / "o"
            other.mkdir()
            env, log = diagram.fake_tools({}, other, {})
            self.assertEqual((sorted(env), log), (["TMPDIR"], None))

    def test_manifest_is_complete(self):
        manifest = json.loads(diagram.CASES.read_text(encoding="utf-8"))
        self.assertEqual(manifest["profile"], diagram.PROFILE)
        self.assertEqual(manifest["asciidoctor_diagram"],
                         (diagram.ROOT / "compatibility" / "asciidoctor-diagram-version").read_text().strip())
        names = [case["name"] for case in manifest["cases"]]
        self.assertEqual(len(names), len(set(names)))
        for case in manifest["cases"]:
            self.assertTrue(case["source"], case["name"])
            self.assertTrue(set(case.get("requires", [])) <= {
                "dot", "plantuml", "mmdc", "server", "nomnoml", "smcat", "vl2vg", "vg2svg",
                "wavedrom-cli", "bytefield-svg", "dbml-renderer"})
            if "xfail" in case:
                # An expected failure freezes adoctra's complete result.
                self.assertTrue(case["xfail"]["reason"])
                self.assertIsNotNone(case["xfail"]["actual"], case["name"])
        report = json.loads(diagram.REPORT.read_text(encoding="utf-8"))
        self.assertEqual(sorted(report["cases"]), sorted(names))


if __name__ == "__main__":
    unittest.main()
