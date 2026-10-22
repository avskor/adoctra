"""The independent documentation snapshots must stay pinned and reproducible."""

import re
import unittest

import acceptance


class DocumentationCorpusTests(unittest.TestCase):
    def setUp(self):
        self.root = acceptance.DEFAULT_MANIFEST.parent
        self.body, _ = acceptance.load_corpus(self.root / "cases.json")
        self.structure, _ = acceptance.load_corpus(self.root / "structure.json")
        self.origins = {"asciidoctor-docs-2.0.23", "spring-framework-6.2.0"}
        self.documents = [case for case in self.body["cases"]
                          if self.body["files"][case["source"]]["origin"] in self.origins]

    def test_pages_are_full_upstream_snapshots_with_commit_license_and_configuration(self):
        self.assertEqual(len(self.documents), 12)
        for origin in self.origins:
            record = self.body["origins"][origin]
            self.assertRegex(record["revision"], r"^[0-9a-f]{40}$")
            self.assertTrue(record["source"].endswith("/tree/" + record["revision"]))
            self.assertIn(record["notice"]["path"], self.body["files"])
            self.assertIn(record["configuration"], self.body["files"])
            pages = [c for c in self.documents if self.body["files"][c["source"]]["origin"] == origin]
            self.assertEqual(len(pages), 6)
            for case in pages:
                file = self.body["files"][case["source"]]
                self.assertIn("/pages/", file["upstream_path"])
                self.assertEqual(case["source"], f"sources/{origin}/{file['upstream_path']}")

    def test_both_profiles_use_identical_pages_resources_attributes_and_provenance(self):
        structural_cases = {case["id"]: case for case in self.structure["cases"]}
        for case in self.documents:
            other = structural_cases["structure-" + case["id"]]
            for key in ["source", "resources", "attributes", "profile"]:
                self.assertEqual(case.get(key), other.get(key), (case["id"], key))
            for name in [case["source"], *case.get("resources", {}).values()]:
                self.assertEqual(self.body["files"][name], self.structure["files"][name])
        for origin in self.origins:
            self.assertEqual(self.body["origins"][origin], self.structure["origins"][origin])

    def test_antora_examples_are_real_registered_resources_and_host_urls_are_resolved(self):
        resources = set()
        for case in self.documents:
            source = (self.root / case["source"]).read_text()
            targets = set(re.findall(r"^include::([^\[]+)\[", source, re.M))
            self.assertEqual(targets, set(case.get("resources", {})), case["id"])
            for target, name in case.get("resources", {}).items():
                self.assertTrue(target.startswith("example$"))
                self.assertIn("/examples/", self.body["files"][name]["upstream_path"])
                resources.add(name)
            for value in case["attributes"].values():
                self.assertNotRegex(value, r"\{[^}]+\}")
                self.assertFalse(value.endswith("@"))
        self.assertEqual(len(resources), 6)


if __name__ == "__main__":
    unittest.main()
