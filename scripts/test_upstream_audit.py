import json
from pathlib import Path
import tempfile
import unittest

import upstream_audit
from upstream_audit import attribute_args, classify, compare, entry_id, load_inventory, raw_body


class ClassifyTests(unittest.TestCase):
    def test_ruby_attribute_forms_become_cli_spellings(self):
        self.assertEqual(attribute_args({"a": "1", "b": None, "c": False, "d": True, "e!": None}),
                         ["a=1", "b!", "c!", "d", "e!"])
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
                ({"base_dir": "/x", "to_file": False}, "option:base_dir,to_file"),
                ({"safe": "foo"}, "safe:foo"),
                ({"attributes": {"docdir": "<UPSTREAM>/test"}}, "path:upstream-checkout")]:
            self.assertEqual(classify("x", options), (None, reason))
        self.assertEqual(classify("include::http://localhost:9876/a[]", {})[1], "network:test-server")
        self.assertEqual(classify("#<IO>", {})[1], "source:non-string")

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

    def test_unprojectable_html_compares_raw_bodies(self):
        broken = "<p><strong><x></strong></p>"
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
            path = sources / "inventory.json"
            first = inventory["entries"][0]["occurrences"][0]
            first["test"] = first["test"] + " (changed)"
            path.write_text(json.dumps(inventory))
            with self.assertRaises(ValueError):
                load_inventory(path, sources)
            first["test"] = first["test"].removesuffix(" (changed)")
            path.write_text(json.dumps(inventory))
            load_inventory(path, sources)
            changed = sources / first["file"]
            changed.write_bytes(changed.read_bytes() + b"\n")
            with self.assertRaises(ValueError):
                load_inventory(path, sources)


if __name__ == "__main__":
    unittest.main()
