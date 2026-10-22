import contextlib
import copy
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import acceptance
from compatibility import pretty
import structure


def block(name, **fields):
    return dict(name=name, id=None, roles=[], **fields)


def project(*blocks):
    return structure.document_projection({"name": "document", "blocks": list(blocks)})


class ProjectionTests(unittest.TestCase):
    def test_sections_take_metadata_from_heading_without_flattening_children(self):
        heading = dict(name="heading", level=1, id="section", roles=["b", "a"])
        section = block("section", level=1, blocks=[heading, block("paragraph")])
        result = project(block("heading", level=0), block("preamble", blocks=[block("paragraph")]), section)
        self.assertTrue(result["has_title"])
        self.assertEqual(result["blocks"][1], {
            "context": "section", "level": 1, "id": "section", "roles": ["b", "a"],
            "blocks": [{"context": "paragraph", "id": None, "roles": []}],
        })
        self.assertFalse(project(block("paragraph"))["has_title"])
        self.assertEqual(project(block("heading", level=2))["blocks"][0]["context"], "floating_title")

    def test_order_nesting_kinds_ids_roles_and_levels_are_significant(self):
        original = [block("paragraph"), block("listing"),
                    block("section", level=1, blocks=[block("heading", level=1)])]
        variants = []
        for field, value in [("id", "new"), ("roles", ["role"]), ("name", "literal")]:
            changed = copy.deepcopy(original)
            changed[0][field] = value
            variants.append(changed)
        variants.append(original[::-1])
        variants.append([block("open", blocks=original)])
        variants.append([*original[:2], block("section", level=2, blocks=[block("heading", level=2)])])
        for variant in variants:
            self.assertNotEqual(project(*original), project(*variant))

    def test_simple_container_is_distinct_from_compound_and_cannot_hide_children(self):
        simple = block("quote", simple=True, blocks=[block("paragraph")])
        compound = dict(simple, simple=False)
        self.assertNotEqual(project(simple), project(compound))
        self.assertEqual(project(simple)["blocks"][0]["blocks"], [])
        for children in [[], [block("listing")], [block("paragraph"), block("paragraph")],
                         [dict(block("paragraph"), id="hidden")]]:
            with self.assertRaises(ValueError):
                project(dict(simple, blocks=children))

    def test_lists_preserve_item_count_checkbox_state_and_attached_blocks(self):
        item = {"inlines": [], "blocks": []}
        source = block("list", ordered=False, items=[item, dict(item, checkbox=False), dict(item, checkbox=True)])
        result = project(source)["blocks"][0]
        self.assertEqual(result["context"], "ulist")
        self.assertEqual([i["checkbox"] for i in result["items"]], [None, False, True])
        for changed in [dict(source, ordered=True), dict(source, items=[item]),
                        dict(source, items=[dict(item, blocks=[block("literal")])])]:
            self.assertNotEqual(project(source), project(changed))
        dlist = block("dlist", items=[{"terms": [[], []], "description": item},
                                     {"terms": [[]], "description": None}])
        self.assertEqual(project(dlist)["blocks"][0]["items"], [
            {"terms": 2, "description": {"checkbox": None, "blocks": []}},
            {"terms": 1, "description": None},
        ])

    def test_tables_preserve_groups_spans_styles_and_nested_documents(self):
        cell = {"style": "asciidoc", "colspan": None, "rowspan": 2,
                "blocks": [block("heading", level=0), block("paragraph")]}
        table = block("table", columns=[{}, {}], head=[], body=[[cell]], foot=[])
        result = project(table)["blocks"][0]
        self.assertEqual(result["columns"], 2)
        self.assertEqual(result["body"][0][0]["colspan"], 1)
        self.assertTrue(result["body"][0][0]["document"]["has_title"])
        for field, value in [("colspan", 2), ("rowspan", 1), ("style", "literal"), ("blocks", [])]:
            self.assertNotEqual(project(table), project(dict(table, body=[[dict(cell, **{field: value})]])))
        self.assertNotEqual(project(table), project(dict(table, head=table["body"], body=[])))

    def test_unknown_nodes_invalid_section_shape_and_invalid_flags_fail_closed(self):
        for node in [block("unknown"), block("section", level=1, blocks=[]),
                     block("section", level=1, blocks=[block("heading", level=2)]),
                     block("list", ordered="false", items=[])]:
            with self.subTest(node=node), self.assertRaises(ValueError):
                project(node)
        for doc in [[], None, {"name": "document", "blocks": {}}]:
            with self.assertRaises(ValueError):
                structure.document_projection(doc)

    def test_saved_schema_rejects_unknown_fields_and_wrong_types(self):
        valid = project(block("paragraph"))
        variants = [dict(valid, unknown=True), dict(valid, has_title=1),
                    dict(valid, blocks={}), dict(valid, blocks=[{"context": "future"}])]
        for node in variants:
            with self.assertRaises(ValueError):
                structure.validate_structure(node)

    def test_contents_are_outside_this_profile(self):
        self.assertEqual(project(block("paragraph", inlines=[{"value": "one"}])),
                         project(block("paragraph", inlines=[{"value": "two"}])))

    def test_initial_discrete_level_zero_heading_is_not_a_document_title(self):
        for style in ["discrete", "float"]:
            result = project(block("heading", level=0, style=style), block("paragraph"))
            self.assertFalse(result["has_title"])
            self.assertEqual(len(result["blocks"]), 2)
            self.assertEqual(result["blocks"][0]["context"], "floating_title")


class StructureExecutionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "reference").mkdir()
        self.case = {"id": "structure-test", "category": "structure", "source": "Text.", "expect": "pass"}
        self.reference = {"structure": project(block("paragraph"))}
        self.path = self.root / "reference/structure-test.json"
        self.path.write_text(pretty(self.reference))
        self.actual = {"name": "document", "blocks": [block("paragraph")]}

    def run_case(self, actual=None, **options):
        output = subprocess.CompletedProcess([], 0, json.dumps(actual or self.actual), "")
        with patch("acceptance.invoke", return_value=output) as run, contextlib.redirect_stdout(io.StringIO()):
            result = acceptance.check_case(self.case, Path("adoctra"), self.root,
                                           profile=structure.PROFILE, **options)
        return result, run

    def test_offline_profile_uses_json_backend_without_ruby(self):
        result, run = self.run_case()
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["checks"], {"structure": "PASS"})
        command = run.call_args.args[0]
        self.assertEqual(command[command.index("--to") + 1], "json")
        self.assertEqual(run.call_count, 1)
        self.assertNotEqual(command[0], "ruby")

    def test_reference_update_and_drift_check_use_structure_mode(self):
        result, run = self.run_case(self.reference, update=True)
        self.assertIsNone(result)
        self.assertEqual(run.call_count, 1)
        self.assertIn("--structure", run.call_args.args[0])
        changed = {"structure": project(block("literal"))}
        with self.assertRaisesRegex(ValueError, "reference drift"):
            self.run_case(changed, verify=True)

    def test_include_keeps_same_resource_tree_for_both_processors(self):
        self.case.update(profile="include-body-dom-v1", resources={"child.adoc": "child"})
        outputs = [subprocess.CompletedProcess([], 0, json.dumps(value), "")
                   for value in [self.reference, self.actual]]
        with patch("acceptance.invoke", side_effect=outputs) as run, contextlib.redirect_stdout(io.StringIO()):
            acceptance.check_case(self.case, Path("adoctra"), self.root, profile=structure.PROFILE, verify=True)
        reference, actual = [call.args[0] for call in run.call_args_list]
        self.assertEqual(Path(actual[1]).parent, Path(reference[reference.index("--include-root") + 1]))
        self.assertIn("safe", actual)

    def test_known_structural_difference_rejects_regression_and_unexpected_fix(self):
        self.case.update(expect="xfail", reason="known structure")
        changed = dict(self.actual, blocks=[block("literal")])
        self.assertEqual(self.run_case(changed, record=True)[0]["status"], "XFAIL")
        self.assertEqual(self.run_case(changed)[0]["status"], "XFAIL")
        self.assertEqual(self.run_case(dict(self.actual, blocks=[]))[0]["status"], "FAIL")
        self.assertEqual(self.run_case()[0]["status"], "XPASS")
        (self.root / "baseline/structure-test.json").write_text('{"structure": {}}')
        with self.assertRaises(ValueError):
            self.run_case(changed)

    def test_html_reference_or_non_document_structure_is_rejected(self):
        for report in [{"html": "<p>Text.</p>", "diagnostics": []},
                       {"structure": {"context": "paragraph", "id": None, "roles": []}}]:
            self.path.write_text(pretty(report))
            with self.assertRaises(ValueError):
                self.run_case()

    def test_structural_references_and_manifest_are_valid(self):
        path = acceptance.DEFAULT_MANIFEST.with_name("structure.json")
        manifest, cases = acceptance.load_corpus(path)
        self.assertEqual(manifest["profile"], structure.PROFILE)
        for case in cases:
            report = json.loads((path.parent / "reference" / f"{case['id']}.json").read_text())
            acceptance.project_report(report, structure.PROFILE)

    def test_main_reports_the_selected_profile_and_structural_failures(self):
        path = acceptance.DEFAULT_MANIFEST.with_name("structure.json")
        report = self.root / "report.json"
        result = {"id": "structure-probe-tables", "category": "structure", "status": "FAIL",
                  "checks": {"structure": "DIFF"}, "reason": None}
        with patch("acceptance.check_case", return_value=result) as run, contextlib.redirect_stdout(io.StringIO()):
            status = acceptance.main(["--manifest", str(path), "--case", result["id"],
                                      "--json-report", str(report)])
        self.assertEqual(status, 1)
        self.assertEqual(run.call_args.kwargs["profile"], structure.PROFILE)
        saved = json.loads(report.read_text())
        self.assertEqual(saved["profile"], structure.PROFILE)
        self.assertEqual(saved["categories"], {"structure": {"FAIL": 1}})

    def test_pinned_probes_really_exercise_every_documented_block_kind(self):
        contexts = set()

        def visit(value):
            if isinstance(value, dict):
                if "context" in value:
                    contexts.add(value["context"])
                for child in value.values():
                    visit(child)
            elif isinstance(value, list):
                for child in value:
                    visit(child)

        for path in (acceptance.DEFAULT_MANIFEST.parent / "reference").glob("structure-probe-*.json"):
            visit(json.loads(path.read_text()))
        self.assertEqual(contexts, {
            "document", "section", "floating_title", "preamble", "paragraph", "listing",
            "literal", "stem", "pass", "thematic_break", "page_break", "toc", "image",
            "video", "audio", "verse", "open", "example", "sidebar", "quote", "admonition",
            "ulist", "olist", "colist", "dlist", "table",
        })

    def test_structural_mode_does_not_ignore_unknown_stderr(self):
        for stderr in ["panic: failure", "unknown warning"]:
            output = subprocess.CompletedProcess([], 0, json.dumps(self.actual), stderr)
            with patch("acceptance.invoke", return_value=output), self.assertRaises(ValueError):
                acceptance.check_case(self.case, Path("adoctra"), self.root, profile=structure.PROFILE)


if __name__ == "__main__":
    unittest.main()
