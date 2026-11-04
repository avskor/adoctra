from pathlib import Path
import tempfile
import unittest

import import_docs


class ImportDocsTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.docs = Path(self.dir.name) / "docs"
        for path, text in {
            "modules/lists/pages/a.adoc": "include::example$x.adoc[tag=t]\ninclude::{partialsdir}/p.adoc[]\n",
            "modules/lists/examples/x.adoc": "include::partial$p.adoc[]\n",
            "modules/lists/partials/p.adoc": "include::html:example$doc.adoc[]\n",
            "modules/html/examples/doc.adoc": "include::example$x.adoc[]\n",
        }.items():
            target = self.docs / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text, encoding="utf-8")

    def test_resource_ids_resolve_like_antora(self):
        lists = self.docs / "modules" / "lists"
        self.assertEqual(import_docs.resolve(self.docs, "lists", "example$x.adoc"), lists / "examples/x.adoc")
        self.assertEqual(import_docs.resolve(self.docs, "lists", "html:example$doc.adoc"),
                         self.docs / "modules/html/examples/doc.adoc")
        for target in ["example$missing.adoc", "{partialsdir}/p.adoc", "ex-text.adoc",
                       "image$x.png", "example$../pages/a.adoc"]:
            self.assertIsNone(import_docs.resolve(self.docs, "lists", target), target)

    def test_includes_are_collected_transitively_in_the_page_module(self):
        page = (self.docs / "modules/lists/pages/a.adoc").read_text()
        found = import_docs.resources(self.docs, "lists", page)
        # The other module's example includes `example$x.adoc`, which Antora
        # resolves in the page's module (an include is relative to the page).
        self.assertEqual(found, {
            "example$x.adoc": self.docs / "modules/lists/examples/x.adoc",
            "partial$p.adoc": self.docs / "modules/lists/partials/p.adoc",
            "html:example$doc.adoc": self.docs / "modules/html/examples/doc.adoc",
        })

    def test_case_ids_name_module_and_page(self):
        self.assertEqual(import_docs.case_id("asciidoc-lang", "lists", Path("ordered.adoc")),
                         "asciidoc-lang-lists-ordered")
        self.assertEqual(import_docs.case_id("asciidoc-lang", "ROOT", Path("syntax-quick-reference.adoc")),
                         "asciidoc-lang-syntax-quick-reference")
        self.assertEqual(import_docs.case_id("docs", "api", Path("sub/Page_One.adoc")), "docs-api-sub-page-one")


if __name__ == "__main__":
    unittest.main()
