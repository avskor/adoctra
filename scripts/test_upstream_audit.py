import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import upstream_audit
from acceptance import cli_diagnostics
from upstream_audit import (attribute_args, classify, compare, entry_id, load_inventory, localize,
                            materialize, raw_body)


class OutputTests(unittest.TestCase):
    def test_cli_output_is_read_as_bytes_so_a_lone_cr_survives(self):
        done = subprocess.CompletedProcess([], 0, b'<pre>a\rb</pre>', b'')
        with patch("upstream_audit.subprocess.run", return_value=done) as run:
            result = upstream_audit.adoctra(Path("/bin/adoctra"), {"source": "x"}, [])
        self.assertEqual(result["html"], "<pre>a\rb</pre>")
        self.assertEqual(run.call_args.kwargs["input"], b"x")
        self.assertNotIn("text", run.call_args.kwargs)


class ClassifyTests(unittest.TestCase):
    def test_ruby_attribute_forms_become_cli_spellings(self):
        self.assertEqual(attribute_args({"a": "1", "b": None, "c": False, "d": True, "e!": None}),
                         ["a=1", "b!", "c!@", "d", "e!"])
        self.assertEqual(attribute_args("a=1 b"), ["a=1", "b"])
        self.assertEqual(attribute_args(["a=1"]), ["a=1"])
        self.assertIsNone(attribute_args({"a": "#<Object>"}))
        self.assertIsNone(attribute_args({"a": ["x"]}))

    def test_supported_entries_map_their_options(self):
        args, reason = classify("x", {"standalone": False, "safe": "safe", "doctype": "book",
                                      "attributes": {"a": "1"}})
        self.assertIsNone(reason)
        self.assertEqual(args, ["-S", "safe", "-s", "-a", "doctype=book", "-a", "a=1"])
        # Document.new defaults to secure mode; a standalone document has no -s.
        self.assertEqual(classify("x", {"standalone": True})[0], ["-S", "secure"])
        self.assertEqual(classify("x", {"safe": 10})[0][:2], ["-S", "server"])

    def test_unsupported_entries_keep_their_reason(self):
        for options, reason in [
                ({"backend": "docbook"}, "backend:docbook"),
                ({"attributes": {"backend": "docbook5"}}, "backend:docbook5"),
                ({"base_dir": "/x", "to_file": False}, "option:to_file:non-string"),
                ({"to_dir": "/tmp/out"}, "option:to_dir:outside-checkout"),
                ({"safe": 1.5}, "safe:1.5")]:
            self.assertEqual(classify("x", options), (None, reason))
        # Ruby's Document#initialize: an unknown level name is secure.
        self.assertEqual(classify("x", {"safe": "foo"})[0], ["-S", "secure", "-s"])
        # File output inside the checkout runs as the CLI's -D/-o.
        self.assertEqual(classify("x", {"to_dir": "<UPSTREAM>/test/test_output"}),
                         (["-S", "secure", "-s", "-D", "<UPSTREAM>/test/test_output"], None))
        self.assertEqual(classify("= T\n:backend: docbook\n", {"safe": "safe"}),
                         (None, "backend:document:docbook"))
        # Not when the host sets the backend or the mode locks it.
        for options in [{"safe": "safe", "backend": "html5"}, {}, {"safe": "server"}]:
            self.assertIsNone(classify("= T\n:backend: docbook\n", options)[1], options)
        self.assertIsNone(classify(":backend: html5\n", {"safe": "safe"})[1])
        for options, reason in []:
            self.assertEqual(classify("x", options), (None, reason))
        # The audit serves the tests' web server itself.
        self.assertIsNone(classify("include::http://127.0.0.1:9876/a[]", {})[1])
        self.assertEqual(classify("#<IO>", {})[1], "source:non-string")

    def test_checkout_paths_run_against_the_vendored_fixtures(self):
        options = {"safe": "safe", "base_dir": "<UPSTREAM>/test",
                   "attributes": {"docdir": "<UPSTREAM>/test/fixtures"}}
        args, reason = classify("include::<UPSTREAM>/test/fixtures/basic.adoc[]", options)
        self.assertIsNone(reason)
        self.assertEqual(args[:4], ["-S", "safe", "-B", "<UPSTREAM>/test"])
        with tempfile.TemporaryDirectory() as scratch:
            root = materialize(scratch)
            self.assertTrue((root / "test/fixtures/basic.adoc").is_file())
            self.assertEqual(localize(args, root)[3], f"{root}/test")
            entry = localize({"source": "<UPSTREAM>/a", "options": options}, root)
            self.assertEqual(entry["source"], f"{root}/a")
            self.assertEqual(entry["options"]["attributes"]["docdir"], f"{root}/test/fixtures")

    def test_the_test_web_server_answers_like_upstreams_helper(self):
        import socket
        with tempfile.TemporaryDirectory() as scratch:
            root = materialize(scratch)
            with upstream_audit.test_server(root, ("127.0.0.1", 0)) as url:
                host, port = url.removeprefix("http://").split(":")

                def get(line):
                    with socket.create_connection((host, int(port))) as connection:
                        connection.sendall(line.encode() + b"\r\n")
                        return b"".join(iter(lambda: connection.recv(4096), b""))

                head, _, data = get("GET /fixtures/dot.gif HTTP/1.1").partition(b"\r\n\r\n")
                self.assertEqual(head, b"HTTP/1.1 200 OK\r\nContent-Type: image/gif")
                self.assertEqual(data, (root / "test/fixtures/dot.gif").read_bytes())
                self.assertIn(b"Content-Type: text/plain\r\n\r\n= Document Title",
                              get("GET /fixtures/basic.adoc HTTP/1.1"))
                self.assertTrue(get("GET /name/asciidoctor HTTP/1.1").endswith(
                    b'application/json\r\n\r\n{"name": "asciidoctor"}\n'))
                for missing in ("/no_such_file", "/../../etc/passwd"):
                    self.assertTrue(get(f"GET {missing} HTTP/1.1").startswith(b"HTTP/1.1 404"))
                self.assertTrue(get("POST / HTTP/1.1").startswith(b"HTTP/1.1 405"))

    def test_multiline_messages_only_when_requested(self):
        stderr = "adoctra: -:1: error: include file not found: a +\nhttp://b\nadoctra: warning: w\n"
        self.assertEqual(cli_diagnostics(stderr, multiline=True),
                         [{"severity": "error", "message": "include file not found: a +\nhttp://b"},
                          {"severity": "warning", "message": "w"}])
        with self.assertRaises(ValueError):
            cli_diagnostics(stderr)
        self.assertEqual(cli_diagnostics("", multiline=True), [])

    def test_entry_ids_are_stable_and_option_order_independent(self):
        self.assertEqual(entry_id("x", {"a": 1, "b": 2}), entry_id("x", {"b": 2, "a": 1}))
        self.assertNotEqual(entry_id("x", {}), entry_id("y", {}))


class CompareTests(unittest.TestCase):
    entry = {"options": {"standalone": False}}

    def test_equal_projection_and_diagnostics_pass(self):
        side = {"html": "<div class=\"paragraph\"><p>a</p></div>", "diagnostics": []}
        self.assertEqual(compare(self.entry, side, {"html": "<p>a</p>", "diagnostics": []}), ("PASS", []))

    def test_body_and_diagnostic_differences_are_named(self):
        expected = {"html": "<p>a</p>", "diagnostics": [{"severity": "warning", "message": "w"}]}
        self.assertEqual(compare(self.entry, expected, {"html": "<p>b</p>", "diagnostics": []}),
                         ("DIFF", ["body", "diagnostics"]))

    def test_errors_are_never_passes(self):
        self.assertEqual(compare(self.entry, {"error": "boom"}, {"html": "", "diagnostics": []})[0],
                         "RUBY_ERROR")
        self.assertEqual(compare(self.entry, {"html": "", "diagnostics": []}, {"error": "x"})[0],
                         "ADOCTRA_ERROR")

    def test_unknown_raw_tags_are_text_for_the_projection(self):
        ruby = {"html": '<div class="paragraph"><p><strong><html5></strong></p></div>', "diagnostics": []}
        ours = {"html": "<p><strong><html5></strong></p>", "diagnostics": []}
        self.assertEqual(compare(self.entry, ruby, ours), ("PASS", []))
        other = {"html": "<p><strong><html4></strong></p>", "diagnostics": []}
        self.assertEqual(compare(self.entry, ruby, other), ("DIFF", ["body"]))

    def test_unprojectable_html_compares_raw_bodies(self):
        broken = "<p><strong><em></strong></p>"
        same = {"html": broken, "diagnostics": []}
        self.assertEqual(compare(self.entry, same, dict(same)), ("PASS", []))
        self.assertEqual(compare(self.entry, same, {"html": broken + "y", "diagnostics": []}),
                         ("DIFF", ["raw-body"]))
        status, reasons = compare(self.entry, same, {"html": "<p>a</p>", "diagnostics": []})
        self.assertEqual(status, "DIFF")
        self.assertTrue(reasons[0].startswith("projection:reference:"))
        self.assertEqual(raw_body("<html><body class=\"x\">\n a  b\n</body></html>"), "a b")

    def test_standalone_shell_tags_are_mapped(self):
        entry = {"options": {"standalone": True}}
        ruby = {"html": "<html><body><div id=\"content\"><p>a</p></div></body></html>", "diagnostics": []}
        ours = {"html": "<html><body><main id=\"content\"><p>a</p></main></body></html>", "diagnostics": []}
        self.assertEqual(compare(entry, ruby, ours), ("PASS", []))
        styled = {k: v.replace('id="content"', 'id="content" style="max-width: 9em;"') for k, v in ruby.items()
                  if k == "html"} | {"diagnostics": []}
        ours_styled = {"html": ours["html"].replace('id="content"', 'id="content" style="max-width: 9em;"'),
                       "diagnostics": []}
        self.assertEqual(compare(entry, styled, ours_styled), ("PASS", []))
        self.assertEqual(compare(entry, styled, ours)[0], "DIFF")


class InventoryTests(unittest.TestCase):
    def test_committed_inventory_provenance_and_baseline(self):
        inventory = load_inventory()
        self.assertTrue(inventory["entries"])
        report = json.loads((upstream_audit.AUDIT / "report.json").read_text(encoding="utf-8"))
        self.assertEqual(set(report["results"]), {e["id"] for e in inventory["entries"]})
        for entry in inventory["entries"]:
            if entry["unsupported"]:
                self.assertEqual(report["results"][entry["id"]]["status"], "UNSUPPORTED")

    def test_tampered_sources_or_declarations_are_rejected(self):
        inventory = json.loads(upstream_audit.INVENTORY.read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory() as scratch:
            sources = Path(scratch)
            for name in inventory["tests"]:
                (sources / name).write_bytes((upstream_audit.SOURCES / name).read_bytes())
            shutil.copytree(upstream_audit.FIXTURES, sources / "fixtures")
            path = sources / "inventory.json"
            first = inventory["entries"][0]["occurrences"][0]
            first["test"] = first["test"] + " (changed)"
            path.write_text(json.dumps(inventory))
            with self.assertRaises(ValueError):
                load_inventory(path, sources)
            first["test"] = first["test"].removesuffix(" (changed)")
            path.write_text(json.dumps(inventory))
            load_inventory(path, sources)
            fixture = sources / "fixtures" / "basic.adoc"
            original = fixture.read_bytes()
            fixture.write_bytes(original + b"\n")
            with self.assertRaises(ValueError):
                load_inventory(path, sources)
            fixture.write_bytes(original)
            changed = sources / first["file"]
            changed.write_bytes(changed.read_bytes() + b"\n")
            with self.assertRaises(ValueError):
                load_inventory(path, sources)


if __name__ == "__main__":
    unittest.main()
