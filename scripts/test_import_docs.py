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
            "modules/lists/pages/a.adoc": ("include::example$x.adoc[tag=t]\ninclude::{partialsdir}/p/q.adoc[]\n"
                                            "include::html:example$doc.adoc[]\ninclude::{missing}/r.adoc[]\n"),
            "modules/lists/examples/x.adoc": "include::partial$p.adoc[]\n",
            "modules/lists/partials/p.adoc": "Partial.\n",
            "modules/lists/partials/p/q.adoc": "include::{snippetsdir}/s.adoc[]\ninclude::../../../out.adoc[]\n",
            "modules/lists/partials/snippets/s.adoc": "Snippet.\n",
            "modules/html/examples/doc.adoc": "Other module.\n",
        }.items():
            target = self.docs / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text, encoding="utf-8")
        self.attributes = {"partialsdir": "partials", "snippetsdir": "../snippets@", "unset!": ""}

    def test_targets_expand_host_attributes(self):
        self.assertEqual(import_docs.expand("{partialsdir}/a.adoc", self.attributes), "partials/a.adoc")
        self.assertEqual(import_docs.expand("{SnippetsDir}/b", self.attributes), "../snippets/b")
        self.assertIsNone(import_docs.expand("{unset}/a.adoc", self.attributes))
        self.assertEqual(import_docs.expand("example$x.adoc", self.attributes), "example$x.adoc")

    def test_ids_and_module_paths_resolve_and_uris_do_not(self):
        lists = self.docs / "modules" / "lists"
        self.assertEqual(import_docs.resolve(self.docs, "lists", "example$x.adoc"), lists / "examples/x.adoc")
        self.assertEqual(import_docs.resolve(self.docs, "lists", "partials/p.adoc"), lists / "partials/p.adoc")
        self.assertEqual(import_docs.resolve(self.docs, "lists", "dir/partial$p.adoc"), lists / "partials/p.adoc")
        # A module-qualified id (or any scheme) is a URI to Asciidoctor.
        for key in ["html:example$doc.adoc", "ROOT:partial$p.adoc", "https://example.org/a.adoc",
                    "example$missing.adoc", "../out.adoc", "/abs.adoc", "image$x.png"]:
            self.assertIsNone(import_docs.resolve(self.docs, "lists", key), key)

    def test_includes_are_collected_transitively_under_their_fixture_names(self):
        page = (self.docs / "modules/lists/pages/a.adoc").read_text()
        found = import_docs.resources(self.docs, "lists", page, self.attributes)
        lists = self.docs / "modules/lists"
        # A nested include is named from the including file's directory, as
        # both processors open it in the fixture tree.
        self.assertEqual(found, {
            "example$x.adoc": lists / "examples/x.adoc",
            "partial$p.adoc": lists / "partials/p.adoc",
            "partials/p/q.adoc": lists / "partials/p/q.adoc",
            "partials/snippets/s.adoc": lists / "partials/snippets/s.adoc",
        })

    def test_collected_directories_supply_their_family(self):
        # JUnit's collector copies src/test into modules/ROOT/examples.
        source = self.docs / "src/test/java/example/Demo.java"
        source.parent.mkdir(parents=True)
        source.write_text("class Demo {}\n", encoding="utf-8")
        collect = [("src/test", "modules/ROOT/examples")]
        self.assertEqual(import_docs.resolve(self.docs, "ROOT", "example$java/example/Demo.java", collect), source)
        self.assertIsNone(import_docs.resolve(self.docs, "ROOT", "example$java/example/Demo.java"))
        self.assertIsNone(import_docs.resolve(self.docs, "lists", "example$java/example/Demo.java", collect))

    def test_document_attribute_entries_expand_later_includes(self):
        snippet = self.docs / "modules/lists/partials/snippets/frag-mysql.adoc"
        snippet.write_text("Snippet.\n", encoding="utf-8")
        page = ":context: mysql\n:partialsdir: ignored\ninclude::{partialsdir}/snippets/frag-{context}.adoc[]\n"
        found = import_docs.resources(self.docs, "lists", page, self.attributes)
        # A hard host attribute stays locked; the page's own entry applies.
        self.assertEqual(found, {"partials/snippets/frag-mysql.adoc": snippet})
        unset = ":context!:\ninclude::partials/snippets/frag-{context}.adoc[]\n"
        self.assertEqual(import_docs.resources(self.docs, "lists", unset, {"context": "mysql@"}), {})

    def test_case_ids_name_module_and_page(self):
        self.assertEqual(import_docs.case_id("asciidoc-lang", "lists", Path("ordered.adoc")),
                         "asciidoc-lang-lists-ordered")
        self.assertEqual(import_docs.case_id("asciidoc-lang", "ROOT", Path("syntax-quick-reference.adoc")),
                         "asciidoc-lang-syntax-quick-reference")
        self.assertEqual(import_docs.case_id("docs", "api", Path("sub/Page_One.adoc")), "docs-api-sub-page-one")


if __name__ == "__main__":
    unittest.main()
