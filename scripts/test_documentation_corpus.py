"""The independent documentation snapshots must stay pinned and reproducible."""

import json
import posixpath
import re
import unittest

import acceptance
import import_docs

# Pages per origin: the full Asciidoctor, AsciiDoc language, Asciidoctor PDF,
# Antora, JUnit, Debezium and Spring Framework documentation (every
# `<component>/modules/*/pages/**/*.adoc` of the pinned revision, imported by
# import_docs.py).
PAGES = {"asciidoctor-docs-2.0.23": 78, "asciidoc-lang-0.3.0": 202, "asciidoctor-pdf-2.3.27": 96,
         "antora-3.2.2": 205, "junit-6.1.3": 64, "debezium-3.7.0": 83, "spring-framework-6.2.0": 455}
# Directories an origin's Antora collector copies into its component.
COLLECT = {"junit-6.1.3": [("src/test", "modules/ROOT/examples")]}
# Pages outside the body profile: Ruby's HTML has crossing tags (a `*...*`
# span over two code spans), which the DOM projection cannot hold; adoctra
# writes the same markup (golden strong_across_monospace).
BODY_EXCLUSIONS = {"debezium-operations-debezium-ui", "debezium-transformations-neo4j-cud-converter"}
# Pages outside the semantic profile: diagnostics (warnings stop the
# profile), raw markup, those crossing tags and a video block.
SEMANTIC_EXCLUSIONS = BODY_EXCLUSIONS | {
    "asciidoc-lang-syntax-quick-reference", "asciidoc-lang-pass-pass-macro",
    "asciidoc-lang-subs-apply-subs-to-text",
    "debezium-install-kafka-connect", "debezium-kc-tutorial",
    "debezium-operations-kubernetes", "debezium-operations-openshift",
    "debezium-operations-debezium-platform",
}
# Pages outside the visual profile: the largest Debezium connector
# references (264-471 KB with their includes, ~90000-185000 px tall) exceed
# what headless Chrome screenshots in time and memory in this environment;
# the other profiles compare them.
VISUAL_EXCLUSIONS = {"debezium-mysql", *(f"debezium-connectors-{name}" for name in
                                          ["oracle", "postgresql", "mariadb", "sqlserver", "db2", "informix"])}
# Pages selected by hand before the full imports keep their earlier
# attribute conventions.
HAND_SELECTED = {"asciidoctor-docs-2.0.23", "spring-framework-6.2.0"}


class DocumentationCorpusTests(unittest.TestCase):
    def setUp(self):
        self.root = acceptance.DEFAULT_MANIFEST.parent
        self.body, _ = acceptance.load_corpus(self.root / "cases.json")
        self.structure, _ = acceptance.load_corpus(self.root / "structure.json")
        self.semantics, _ = acceptance.load_corpus(self.root / "semantics.json")
        self.origins = set(PAGES)
        # The structure profile holds every page.
        self.documents = [dict(case, id=case["id"].removeprefix("structure-"))
                          for case in self.structure["cases"]
                          if self.structure["files"][case["source"]]["origin"] in self.origins
                          and "/pages/" in case["source"]]

    def origin(self, case):
        return self.structure["files"][case["source"]]["origin"]

    def test_pages_are_full_upstream_snapshots_with_commit_license_and_configuration(self):
        self.assertEqual(len(self.documents), sum(PAGES.values()))
        for origin, count in PAGES.items():
            record = self.structure["origins"][origin]
            self.assertRegex(record["revision"], r"^[0-9a-f]{40}$")
            # The tree of the pinned commit (or its docs directory).
            self.assertRegex(record["source"], "/tree/" + record["revision"] + "(/docs)?$")
            self.assertIn(record["notice"]["path"], self.structure["files"])
            self.assertIn(record["configuration"], self.structure["files"])
            pages = [c for c in self.documents if self.origin(c) == origin]
            self.assertEqual(len(pages), count, origin)
            self.assertEqual(len({c["source"] for c in pages}), count, origin)
            for case in pages:
                file = self.structure["files"][case["source"]]
                self.assertIn("/pages/", file["upstream_path"])
                self.assertEqual(case["source"], f"sources/{origin}/{file['upstream_path']}")
                # One origin's pages share its resolved antora.yml attributes.
                self.assertEqual(case["attributes"], pages[0]["attributes"], case["id"])

    def test_every_profile_uses_identical_pages_resources_attributes_and_provenance(self):
        for kind, manifest, excluded in (("body", self.body, BODY_EXCLUSIONS),
                                         ("semantics", self.semantics, SEMANTIC_EXCLUSIONS)):
            cases = {case["source"]: case for case in manifest["cases"]}
            for case in self.documents:
                other = cases.get(case["source"])
                if case["id"] in excluded:
                    self.assertIsNone(other, (kind, case["id"]))
                    continue
                self.assertIsNotNone(other, (kind, case["id"]))
                for key in ["resources", "attributes", "profile"]:
                    self.assertEqual(case.get(key), other.get(key), (kind, case["id"], key))
                for name in [case["source"], *case.get("resources", {}).values()]:
                    self.assertEqual(self.structure["files"][name], manifest["files"][name])
            for origin in self.origins:
                self.assertEqual(self.structure["origins"][origin], manifest["origins"][origin])

    def test_visual_profile_renders_every_page(self):
        visual = json.loads((self.root.parent / "visual" / "cases.json").read_text(encoding="utf-8"))
        sources = {case["source"]: case for case in visual["cases"]}
        for case in self.documents:
            entry = sources.get(f"compatibility/acceptance/{case['source']}")
            if case["id"] in VISUAL_EXCLUSIONS:
                self.assertIsNone(entry, case["id"])
                continue
            self.assertIsNotNone(entry, case["id"])
            self.assertEqual(entry.get("attributes", {}), case["attributes"], case["id"])
            self.assertEqual(entry.get("resources", {}),
                             {name: f"compatibility/acceptance/{path}"
                              for name, path in case.get("resources", {}).items()}, case["id"])

    def test_resources_are_the_files_both_processors_read(self):
        for case in self.documents:
            origin = self.origin(case)
            resources = case.get("resources", {})
            page = self.structure["files"][case["source"]]["upstream_path"]
            component, rest = page.split("/modules/", 1)
            module = rest.split("/")[0]
            # Every fixture name is an include target of the page or of a
            # resource (joined to the including file's directory), read in
            # order with the attribute entries before it, as the import does.
            reachable = set()
            attributes = import_docs.Attributes(case["attributes"])

            def walk(path, base, depth=0):
                for line in (self.root / path).read_text().splitlines():
                    attributes.read(line)
                    include = import_docs.INCLUDE.match(line)
                    expanded = include and import_docs.expand(include[1], attributes)
                    if not expanded:
                        continue
                    key = posixpath.normpath(posixpath.join(base, expanded)) if base else expanded
                    reachable.add(key)
                    if key in resources and depth < import_docs.MAX_INCLUDE_DEPTH:
                        walk(resources[key], posixpath.dirname(key), depth + 1)

            walk(case["source"], "")
            self.assertLessEqual(set(resources), reachable, case["id"])
            for key, name in resources.items():
                # Never a URI to Asciidoctor (a module-qualified Antora id).
                self.assertIsNone(re.match(r"[A-Za-z][A-Za-z0-9.+-]+:", key), (case["id"], key))
                upstream = self.structure["files"][name]["upstream_path"]
                nested = re.search(r"(?:^|/)((?:example|partial|page)\$.+)$", key)
                if nested:
                    match = import_docs.RESOURCE_ID.fullmatch(nested[1])
                    virtual = f"modules/{module}/{import_docs.FAMILIES[match[2]]}/{match[3]}"
                else:
                    virtual = f"modules/{module}/{key}"
                expected = {f"{component}/{virtual}"}
                for src, into in COLLECT.get(origin, []):
                    if virtual.startswith(into + "/"):
                        expected.add(f"{component}/{src}/{virtual[len(into) + 1:]}")
                self.assertIn(upstream, expected, (case["id"], key))
            defined = {name.rstrip("@!").lower() for name in case["attributes"]}
            for value in case["attributes"].values():
                # References between host values are expanded beforehand; one
                # to a value the build generates (JUnit's `release-branch`)
                # stays unset, and both processors show the same literal.
                for name in re.findall(r"\{([\w-]+)\}", value):
                    self.assertNotIn(name.lower(), defined, (case["id"], value))
                # A soft default keeps antora.yml's `@`, which the CLI
                # accepts; the hand-selected Asciidoctor and Spring pages pass
                # hard values (from before soft overrides were supported).
                if origin in HAND_SELECTED:
                    self.assertFalse(value.endswith("@"))


if __name__ == "__main__":
    unittest.main()
