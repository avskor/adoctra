import contextlib
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import acceptance
from compatibility import pretty


class CorpusTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.source = b"= Title\r\n\r\nText.\r\n"
        (self.root / "source.adoc").write_bytes(self.source)
        (self.root / "COPYRIGHT").write_text("Copyright, permission notice\n")
        self.manifest = {
            "asciidoctor": "2.0.23", "profile": acceptance.PROFILE,
            "origins": {"upstream": {
                "project": "Example", "revision": "1.0", "source": "pinned release",
                "notice": {"path": "COPYRIGHT", "sha256": self.digest(b"Copyright, permission notice\n")},
            }},
            "files": {"source.adoc": {"origin": "upstream", "sha256": self.digest(self.source)}},
            "cases": [{"id": "document", "category": "document", "source": "source.adoc", "expect": "pass"}],
        }
        self.path = self.root / "cases.json"
        self.save()

    @staticmethod
    def digest(raw):
        return hashlib.sha256(raw).hexdigest()

    def save(self):
        self.path.write_text(pretty(self.manifest), encoding="utf-8")

    def test_pinned_sources_preserve_original_line_endings(self):
        _, cases = acceptance.load_corpus(self.path)
        self.assertEqual(cases[0]["source"], self.source.decode())
        self.assertEqual(self.manifest["cases"][0]["source"], "source.adoc")

    def test_changed_source_and_notice_fail_before_running_renderers(self):
        for name in ["source.adoc", "COPYRIGHT"]:
            with self.subTest(name=name):
                path = self.root / name
                before = path.read_bytes()
                path.write_bytes(before + b"changed")
                with patch("acceptance.invoke") as run:
                    with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                        acceptance.main(["--manifest", str(self.path)])
                    run.assert_not_called()
                path.write_bytes(before)

    def test_paths_cannot_escape_or_alias_parent_directories(self):
        for name in ["../outside", "/absolute", "a/../source.adoc", "a//b", "./source.adoc", "C:x", "a\\b"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                acceptance.fixture_path(self.root, name)
        with tempfile.TemporaryDirectory() as other:
            external = Path(other) / "external"
            external.write_bytes(b"x")
            (self.root / "link").symlink_to(external)
            with self.assertRaises(ValueError):
                acceptance.fixture_path(self.root, "link")

    def test_reference_version_profile_provenance_and_expectations_are_validated(self):
        variants = [
            ("version", lambda m: m.update(asciidoctor="2.0.24")),
            ("profile", lambda m: m.update(profile="unknown")),
            ("origin", lambda m: m["origins"]["upstream"].pop("revision")),
            ("empty", lambda m: m.update(cases=[])),
            ("duplicate", lambda m: m["cases"].append(m["cases"][0].copy())),
            ("id", lambda m: m["cases"][0].update(id="../escape")),
            ("xfail", lambda m: m["cases"][0].update(expect="xfail")),
            ("category", lambda m: m["cases"][0].update(category="unknown")),
            ("files-type", lambda m: m.update(files=[])),
            ("case-type", lambda m: m.update(cases=["bad"])),
            ("resources-type", lambda m: m["cases"][0].update(resources=[])),
            ("origin-type", lambda m: m["origins"].update(upstream="bad")),
            ("notice-type", lambda m: m["origins"]["upstream"].update(notice="bad")),
        ]
        original = json.loads(self.path.read_text())
        for name, change in variants:
            with self.subTest(name=name):
                self.manifest = json.loads(json.dumps(original))
                change(self.manifest)
                self.save()
                with self.assertRaises(ValueError):
                    acceptance.load_corpus(self.path)

    def test_includes_use_only_declared_pinned_resources(self):
        case = self.manifest["cases"][0]
        case.update(profile="include-body-dom-v1", resources={"sub/part.adoc": "source.adoc"})
        self.save()
        _, cases = acceptance.load_corpus(self.path)
        self.assertEqual(cases[0]["resources"], {"sub/part.adoc": self.source.decode()})
        case["resources"] = {"../escape": "source.adoc"}
        self.save()
        with self.assertRaises(ValueError):
            acceptance.load_corpus(self.path)

    def test_unknown_case_filter_is_an_error(self):
        with self.assertRaisesRegex(ValueError, "unknown acceptance cases"):
            acceptance.main(["--manifest", str(self.path), "--case", "typo"])

    def test_ci_exit_status_and_json_report_include_diagnostic_failures(self):
        report = self.root / "report.json"
        for status, code in [("PASS", 0), ("XFAIL", 0), ("FAIL", 1), ("XPASS", 1)]:
            result = {"id": "document", "category": "document", "status": status,
                      "checks": {"body": "PASS", "diagnostics": "DIFF"}, "reason": None}
            with self.subTest(status=status), patch("acceptance.check_case", return_value=result):
                with contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(acceptance.main([
                        "--manifest", str(self.path), "--json-report", str(report),
                    ]), code)
                saved = json.loads(report.read_text())
                self.assertEqual(saved["counts"], {status: 1})
                self.assertEqual(saved["cases"][0]["checks"]["diagnostics"], "DIFF")

    def test_rendering_error_identifies_the_case_without_accepting_xfail(self):
        self.manifest["cases"][0].update(expect="xfail", reason="known warning")
        self.save()
        with patch("acceptance.check_case", side_effect=RuntimeError("renderer crashed")):
            with self.assertRaisesRegex(RuntimeError, "document: renderer crashed"):
                acceptance.main(["--manifest", str(self.path)])


class DiagnosticTests(unittest.TestCase):
    def test_cli_parser_and_backend_messages_keep_order_severity_and_text(self):
        stderr = ("adoctra: /tmp/fixture/sub/я.adoc:4:2: warning at 8..12: duplicate: x\n"
                  "adoctra: -:6:1: error at 30..31: invalid row\n"
                  "adoctra: warning: cannot read /tmp/fixture/image.svg\n")
        self.assertEqual(acceptance.cli_diagnostics(stderr, Path("/tmp/fixture")), [
            {"severity": "warning", "message": "duplicate: x"},
            {"severity": "error", "message": "invalid row"},
            {"severity": "warning", "message": "cannot read <fixture>/image.svg"},
        ])
        self.assertEqual(acceptance.cli_diagnostics("adoctra: -:1:1: info at 0..1: trace\n"), [])

    def test_unexpected_stderr_is_never_silently_discarded(self):
        for stderr in ["panic: x\n", "adoctra: unknown message\n", "adoctra: warning: first\ncontinuation\n"]:
            with self.subTest(stderr=stderr), self.assertRaises(ValueError):
                acceptance.cli_diagnostics(stderr)

    def test_backend_warning_keeps_nested_diagnostic_text(self):
        message = "command failed: /path:3:4: error at 10..12: nested text"
        self.assertEqual(acceptance.cli_diagnostics(f"adoctra: warning: {message}\n"),
                         [{"severity": "warning", "message": message}])

    def test_bad_reference_report_is_not_a_compatibility_failure(self):
        for report in [
            {"html": "<p>x</p>", "diagnostics": {}},
            {"html": "<p>x</p>", "diagnostics": [{"severity": "warn", "message": "x"}]},
            {"html": "<p>x", "diagnostics": []},
            {"html": "<p>x</p>", "diagnostics": [], "unexpected": 1},
        ]:
            result = subprocess.CompletedProcess([], 0, json.dumps(report), "")
            with self.subTest(report=report), self.assertRaises(ValueError):
                acceptance.reference_report(result)
        with self.assertRaisesRegex(ValueError, "reference stderr"):
            acceptance.reference_report(subprocess.CompletedProcess([], 0, "{}", "warning"))

    def test_profile_preserves_diagnostic_order_duplicates_and_severity(self):
        a = {"severity": "warning", "message": "a"}
        b = {"severity": "error", "message": "b"}
        original = {"html": "<p>x</p>", "diagnostics": [a, a, b]}
        projected = acceptance.project_report(original)
        for diagnostics in [[a, b], [b, a, a], [a, a, dict(b, severity="warning")], []]:
            self.assertNotEqual(projected, acceptance.project_report(dict(original, diagnostics=diagnostics)))


class ExecutionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "reference").mkdir()
        self.reference = {"html": "<p>same</p>\n", "diagnostics": []}
        self.reference_path = self.root / "reference" / "case.json"
        self.reference_path.write_text(pretty(self.reference))
        self.case = {"id": "case", "source": "same", "category": "document", "expect": "pass"}

    def run_case(self, html="<p>same</p>\n", stderr="", **options):
        with patch("acceptance.invoke", return_value=subprocess.CompletedProcess([], 0, html, stderr)) as run:
            with contextlib.redirect_stdout(io.StringIO()):
                result = acceptance.check_case(self.case, Path("adoctra"), self.root, **options)
            return result, run

    def test_offline_run_never_invokes_ruby(self):
        result, run = self.run_case()
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["checks"], {"body": "PASS", "diagnostics": "PASS"})
        self.assertEqual(run.call_count, 1)
        self.assertNotEqual(run.call_args.args[0][0], "ruby")

    def test_same_html_with_new_warning_fails(self):
        result, _ = self.run_case(stderr="adoctra: warning: new warning\n")
        self.assertEqual(result["status"], "FAIL")
        self.assertEqual(result["checks"], {"body": "PASS", "diagnostics": "DIFF"})

    def test_xfail_baseline_detects_new_regressions_and_unexpected_fixes(self):
        self.case.update(expect="xfail", reason="known warning")
        stderr = "adoctra: warning: known\n"
        result, _ = self.run_case(stderr=stderr, record=True)
        self.assertEqual(result["status"], "XFAIL")
        self.assertEqual(self.run_case(stderr=stderr)[0]["status"], "XFAIL")
        self.assertEqual(self.run_case(stderr="adoctra: warning: changed\n")[0]["status"], "FAIL")
        self.assertEqual(self.run_case(html="<p>changed</p>", stderr=stderr)[0]["status"], "FAIL")
        self.assertEqual(self.run_case()[0]["status"], "XPASS")

    def test_update_runs_only_reference_and_verification_detects_drift(self):
        output = json.dumps(self.reference)
        result, run = self.run_case(html=output, update=True)
        self.assertIsNone(result)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[0][0], "ruby")
        self.assertIn("--report", run.call_args.args[0])
        self.assertEqual(json.loads(self.reference_path.read_text()), self.reference)
        changed = json.dumps(dict(self.reference, diagnostics=[{"severity": "warning", "message": "new"}]))
        with self.assertRaisesRegex(ValueError, "reference drift"):
            self.run_case(html=changed, verify=True)

    def test_process_failure_is_an_error_even_for_xfail(self):
        failed = subprocess.CompletedProcess(["adoctra"], 1, b"", b"panic")
        with patch("acceptance.subprocess.run", return_value=failed):
            with self.assertRaisesRegex(RuntimeError, "panic"):
                acceptance.invoke(["adoctra"], "")
        # Output is read as bytes: a lone CR stays content.
        done = subprocess.CompletedProcess(["adoctra"], 0, "a\rb\r\n".encode(), b"")
        with patch("acceptance.subprocess.run", return_value=done) as run:
            self.assertEqual(acceptance.invoke(["adoctra"], "Я").stdout, "a\rb\r\n")
        self.assertEqual(run.call_args.kwargs["input"], "Я".encode())
        with patch("acceptance.subprocess.run", side_effect=subprocess.TimeoutExpired("adoctra", 30)):
            with self.assertRaises(subprocess.TimeoutExpired):
                acceptance.invoke(["adoctra"], "")

    def test_corrupted_baseline_is_an_error(self):
        self.case.update(expect="xfail", reason="known warning")
        (self.root / "baseline").mkdir()
        (self.root / "baseline" / "case.json").write_text("{}")
        with self.assertRaisesRegex(ValueError, "invalid baseline"):
            self.run_case(stderr="adoctra: warning: known\n")


if __name__ == "__main__":
    unittest.main()
