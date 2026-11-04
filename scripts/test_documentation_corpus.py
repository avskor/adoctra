"""The independent documentation snapshots must stay pinned and reproducible."""

import json
import re
import unittest

import acceptance
import import_docs

# Pages per origin: the full Asciidoctor, AsciiDoc language, Asciidoctor PDF
# and Antora documentation (every `docs/modules/*/pages/**/*.adoc` of the
# pinned revision, imported by import_docs.py) and a selection of Spring
# Framework pages.
PAGES = {"asciidoctor-docs-2.0.23": 78, "asciidoc-lang-0.3.0": 202, "asciidoctor-pdf-2.3.27": 96,
         "antora-3.2.2": 205, "spring-framework-6.2.0": 6}
# Full documentation pages outside the semantic profile by its design (they
# stay in the body, structure and visual profiles): diagnostics (warnings
# stop the profile) and raw markup.
SEMANTIC_EXCLUSIONS = {
    "asciidoc-lang-syntax-quick-reference", "asciidoc-lang-pass-pass-macro",
    "asciidoc-lang-subs-apply-subs-to-text",
}


class DocumentationCorpusTests(unittest.TestCase):
    def setUp(self):
        self.root = acceptance.DEFAULT_MANIFEST.parent
        self.body, _ = acceptance.load_corpus(self.root / "cases.json")
        self.structure, _ = acceptance.load_corpus(self.root / "structure.json")
        self.semantics, _ = acceptance.load_corpus(self.root / "semantics.json")
        self.origins = set(PAGES)
        self.documents = [case for case in self.body["cases"]
                          if self.body["files"][case["source"]]["origin"] in self.origins
                          and "/pages/" in case["source"]]

    def test_pages_are_full_upstream_snapshots_with_commit_license_and_configuration(self):
        self.assertEqual(len(self.documents), sum(PAGES.values()))
        for origin, count in PAGES.items():
            record = self.body["origins"][origin]
            self.assertRegex(record["revision"], r"^[0-9a-f]{40}$")
            # The tree of the pinned commit (or its docs directory).
            self.assertRegex(record["source"], "/tree/" + record["revision"] + "(/docs)?$")
            self.assertIn(record["notice"]["path"], self.body["files"])
            self.assertIn(record["configuration"], self.body["files"])
            pages = [c for c in self.documents if self.body["files"][c["source"]]["origin"] == origin]
            self.assertEqual(len(pages), count, origin)
            self.assertEqual(len({c["source"] for c in pages}), count, origin)
            for case in pages:
                file = self.body["files"][case["source"]]
                self.assertIn("/pages/", file["upstream_path"])
                self.assertEqual(case["source"], f"sources/{origin}/{file['upstream_path']}")
                # One origin's pages share its resolved antora.yml attributes.
                self.assertEqual(case["attributes"], pages[0]["attributes"], case["id"])

    def test_every_profile_uses_identical_pages_resources_attributes_and_provenance(self):
        for kind, manifest in (("structure", self.structure), ("semantics", self.semantics)):
            cases = {case["source"]: case for case in manifest["cases"]}
            for case in self.documents:
                other = cases.get(case["source"])
                if kind == "semantics" and case["id"] in SEMANTIC_EXCLUSIONS:
                    self.assertIsNone(other, case["id"])
                    continue
                self.assertIsNotNone(other, (kind, case["id"]))
                for key in ["resources", "attributes", "profile"]:
                    self.assertEqual(case.get(key), other.get(key), (kind, case["id"], key))
                for name in [case["source"], *case.get("resources", {}).values()]:
                    self.assertEqual(self.body["files"][name], manifest["files"][name])
            for origin in self.origins:
                self.assertEqual(self.body["origins"][origin], manifest["origins"][origin])

    def test_visual_profile_renders_every_page(self):
        visual = json.loads((self.root.parent / "visual" / "cases.json").read_text(encoding="utf-8"))
        sources = {case["source"]: case for case in visual["cases"]}
        for case in self.documents:
            entry = sources.get(f"compatibility/acceptance/{case['source']}")
            self.assertIsNotNone(entry, case["id"])
            self.assertEqual(entry.get("attributes", {}), case["attributes"], case["id"])
            self.assertEqual(entry.get("resources", {}),
                             {name: f"compatibility/acceptance/{path}"
                              for name, path in case.get("resources", {}).items()}, case["id"])

    def test_antora_resources_are_real_registered_files_and_host_urls_are_resolved(self):
        for case in self.documents:
            resources = case.get("resources", {})
            texts = [(self.root / case["source"]).read_text()]
            texts += [(self.root / path).read_text() for path in resources.values()]
            targets = {t for text in texts for t in re.findall(r"^include::([^\[]+)\[", text, re.M)}
            # Every resource is an include target; a target without a resource
            # is one Antora cannot resolve either (attribute reference, missing).
            self.assertLessEqual(set(resources), targets, case["id"])
            page = self.body["files"][case["source"]]["upstream_path"]
            module = page.split("/modules/")[1].split("/")[0]
            for target, name in resources.items():
                match = import_docs.RESOURCE_ID.fullmatch(target)
                self.assertIsNotNone(match, (case["id"], target))
                family = import_docs.FAMILIES[match[2]]
                self.assertEqual(self.body["files"][name]["upstream_path"],
                                 f"docs/modules/{match[1] or module}/{family}/{match[3]}", case["id"])
            for target in targets - set(resources):
                self.assertTrue("{" in target or not import_docs.RESOURCE_ID.fullmatch(target)
                                or not (self.root / case["source"]).parent.joinpath(target).exists(),
                                (case["id"], target))
            for value in case["attributes"].values():
                self.assertNotRegex(value, r"\{[^}]+\}")
                # A soft default keeps antora.yml's `@`, which the CLI
                # accepts; the selected Asciidoctor and Spring pages pass
                # hard values (from before soft overrides were supported).
                if origin_of := self.body["files"][case["source"]]["origin"]:
                    if origin_of in {"asciidoctor-docs-2.0.23", "spring-framework-6.2.0"}:
                        self.assertFalse(value.endswith("@"))


if __name__ == "__main__":
    unittest.main()
