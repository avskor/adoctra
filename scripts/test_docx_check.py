import os
import tempfile
import unittest
import zipfile
from unittest.mock import patch

import docx_check

CONTENT_TYPES = """<?xml version="1.0"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Default Extension="svg" ContentType="image/svg+xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"""
ROOT_RELS = """<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"""
DOCUMENT_RELS = """<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.svg"/>
</Relationships>"""
W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
DOCUMENT = f"""<w:document {W}><w:body>
<w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr></w:p><w:sectPr/>
</w:body></w:document>"""
STYLES = f'<w:styles {W}><w:style w:styleId="Title"/></w:styles>'


def package(directory, **overrides):
    parts = {
        "[Content_Types].xml": CONTENT_TYPES,
        "_rels/.rels": ROOT_RELS,
        "word/_rels/document.xml.rels": DOCUMENT_RELS,
        "word/document.xml": DOCUMENT,
        "word/styles.xml": STYLES,
        "word/media/image1.svg": '<svg xmlns="http://www.w3.org/2000/svg"/>',
    }
    parts.update(overrides)
    path = os.path.join(directory, "test.docx")
    with zipfile.ZipFile(path, "w") as zf:
        for name, data in parts.items():
            if data is not None:
                zf.writestr(name, data)
    return path


class DocxCheckTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def test_valid_package_passes(self):
        self.assertEqual(docx_check.check(package(self.dir.name)), [])

    def test_malformed_svg_media_is_the_documents_own_file(self):
        # The backend copies an image byte for byte; Word opens the package
        # (golden svg/incomplete), so only OOXML parts must be XML.
        path = package(self.dir.name, **{"word/media/image1.svg": "<svg"})
        self.assertEqual(docx_check.check(path), [])

    def test_malformed_part_undeclared_type_and_missing_target_are_problems(self):
        path = package(self.dir.name, **{"word/styles.xml": "<w:styles", "word/x.bin2": "x",
                                         "word/media/image1.svg": None})
        problems = docx_check.check(path)
        self.assertTrue(any("word/styles.xml: malformed XML" in p for p in problems), problems)
        self.assertIn("word/x.bin2: no content type", problems)
        self.assertTrue(any("targets missing part word/media/image1.svg" in p for p in problems))

    def test_undefined_style_and_missing_final_sect_pr(self):
        document = f'<w:document {W}><w:body><w:p><w:pPr><w:pStyle w:val="Nope"/></w:pPr></w:p></w:body></w:document>'
        problems = docx_check.check(package(self.dir.name, **{"word/document.xml": document}))
        self.assertIn("word/document.xml: undefined pStyle Nope", problems)
        self.assertIn("word/document.xml: w:body does not end with w:sectPr", problems)

    def test_corpus_converts_each_input_as_article_and_book(self):
        os.makedirs(os.path.join(self.dir.name, "in", "sub"))
        for name in ("a.adoc", "sub/b.adoc", "skip.txt"):
            open(os.path.join(self.dir.name, "in", name), "w").close()
        valid = package(self.dir.name)
        calls = []

        def fake_convert(cli, source, doctype, out):
            calls.append((os.path.basename(source), doctype))
            if source.endswith("b.adoc") and doctype == "book":
                return ["conversion failed (1): boom"]
            with open(valid, "rb") as src, open(out, "wb") as dst:
                dst.write(src.read())
            return []

        with patch.object(docx_check, "convert", fake_convert):
            count, problems = docx_check.check_corpus("cli", [os.path.join(self.dir.name, "in")])
        self.assertEqual(count, 4)
        self.assertEqual(sorted(calls), [("a.adoc", "article"), ("a.adoc", "book"),
                                         ("b.adoc", "article"), ("b.adoc", "book")])
        self.assertEqual(len(problems), 1)
        self.assertTrue(problems[0].endswith("b.adoc (book): conversion failed (1): boom"))

    def test_failed_conversion_is_reported(self):
        out = os.path.join(self.dir.name, "x.docx")
        self.assertEqual(docx_check.convert("/bin/false", "in.adoc", "book", out),
                         ["conversion failed (1): "])


if __name__ == "__main__":
    unittest.main()
