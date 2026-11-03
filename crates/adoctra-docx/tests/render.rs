//! End-to-end conversion into the built-in and custom templates.

use std::io::{Cursor, Read, Write};

use adoctra_core::parse;
use adoctra_docx::{check_template, Builtin, DocxBackend};

/// The parts of a `.docx`, in archive order.
fn parts(bytes: &[u8]) -> Vec<(String, String)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("a zip file");
    (0..zip.len())
        .map(|i| {
            let mut file = zip.by_index(i).unwrap();
            let mut text = String::new();
            file.read_to_string(&mut text).unwrap();
            (file.name().to_string(), text)
        })
        .collect()
}

fn part(bytes: &[u8], name: &str) -> String {
    parts(bytes)
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no part {name}"))
        .1
}

fn render(src: &str) -> adoctra_docx::Output {
    DocxBackend::new().render(&parse(src).document).unwrap()
}

#[test]
fn an_empty_document_is_a_complete_package() {
    let out = render("");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let names: Vec<_> = parts(&out.bytes).into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        [
            "[Content_Types].xml",
            "_rels/.rels",
            "word/_rels/document.xml.rels",
            "word/document.xml",
            "word/fontTable.xml",
            "word/footer1.xml",
            "word/numbering.xml",
            "word/settings.xml",
            "word/styles.xml",
        ]
    );
    let document = part(&out.bytes, "word/document.xml");
    // No header: the cover markers and the body sample are gone.
    assert!(!document.contains("<w:sdt>"), "{document}");
    assert!(!document.contains("goes here"));
    assert!(document.ends_with("</w:sectPr></w:body></w:document>\n"));
}

#[test]
fn output_is_deterministic() {
    let src = "= Title\nAnn Lee\n\ntext\n";
    assert_eq!(render(src).bytes, render(src).bytes);
}

#[test]
fn fills_the_header_markers_of_the_article_template() {
    let out = render("= Main Title\nJane Doe <j@x.org>; Bob Roe\nv1.2, 2026-01-02: Draft & more\n");
    let document = part(&out.bytes, "word/document.xml");
    assert!(document.contains(
        r#"<w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t xml:space="preserve">Main Title</w:t></w:r></w:p>"#
    ));
    assert!(document.contains(r#"<w:t xml:space="preserve">Jane Doe, Bob Roe</w:t>"#));
    assert!(document
        .contains(r#"<w:t xml:space="preserve">version 1.2, 2026-01-02: Draft &amp; more</w:t>"#));
    // The template's subtitle marker has no value yet.
    assert!(!document.contains(r#"w:val="Subtitle""#));
    assert!(!document.contains("<w:sdt>"));
}

#[test]
fn the_version_label_attribute_localizes_the_revision_line() {
    let out = render("= T\nA B\nv2, 2026-03-04\n:version-label: Версия\n");
    let document = part(&out.bytes, "word/document.xml");
    assert!(document.contains(r#"<w:t xml:space="preserve">версия 2, 2026-03-04</w:t>"#));
}

#[test]
fn book_documents_get_the_book_template() {
    let book = render("= A Book\n:doctype: book\n");
    let article = render("= An Article\n");
    let book_doc = part(&book.bytes, "word/document.xml");
    // The cover is its own section.
    assert_eq!(book_doc.matches("<w:sectPr>").count(), 2);
    assert_eq!(
        part(&article.bytes, "word/document.xml")
            .matches("<w:sectPr>")
            .count(),
        1
    );
    assert!(part(&book.bytes, "word/styles.xml").contains("<w:pageBreakBefore/>"));
    assert!(!part(&article.bytes, "word/styles.xml").contains("<w:pageBreakBefore/>"));
}

#[test]
fn content_is_reported_until_blocks_are_converted() {
    let out = render("= T\n\nparagraph\n");
    assert_eq!(
        out.warnings,
        ["docx backend does not convert block content yet"]
    );
    assert!(render("= T\n").warnings.is_empty());
}

/// A minimal template as Word saves it: no `adoctra:body` marker, a specimen
/// body, styles with Russian-UI ids and no built-in headings.
fn word_template() -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    let files = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        ),
        (
            "word/_rels/document.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
        ),
        (
            "word/document.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Specimen</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:body></w:document>"#,
        ),
        (
            "word/styles.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:default="1" w:styleId="a"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="a7"><w:name w:val="Listing"/><w:aliases w:val="Code"/></w:style></w:styles>"#,
        ),
        (
            "word/theme/theme1.xml",
            "<a:theme xmlns:a=\"x\">kept</a:theme>",
        ),
    ];
    for (name, text) in files {
        zip.start_file(name, options).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn renders_into_a_custom_template_keeping_its_parts() {
    let doc = parse("= T\n").document;
    let out = DocxBackend::new()
        .with_template(word_template())
        .render(&doc)
        .unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    // Unknown parts are copied unchanged.
    assert_eq!(
        part(&out.bytes, "word/theme/theme1.xml"),
        "<a:theme xmlns:a=\"x\">kept</a:theme>"
    );
    // No marker: the specimen body is replaced, the section stays.
    assert_eq!(
        part(&out.bytes, "word/document.xml"),
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:body></w:document>"#
    );
    // A .dotx main part is declared a document.
    let types = part(&out.bytes, "[Content_Types].xml");
    assert!(types.contains("wordprocessingml.document.main+xml"));
    assert!(!types.contains("template.main+xml"));
}

#[test]
fn an_invalid_template_is_an_error() {
    let doc = parse("").document;
    let err = DocxBackend::new()
        .with_template(b"not a docx".to_vec())
        .render(&doc)
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("invalid docx template: not a zip file"),
        "{err}"
    );
}

#[test]
fn checks_a_template_against_the_style_contract() {
    for builtin in [Builtin::Book, Builtin::Article] {
        assert_eq!(
            check_template(&builtin.to_docx()).unwrap(),
            Vec::<String>::new()
        );
    }
    let warnings = check_template(&word_template()).unwrap();
    // Word built-ins (headings, TOC, caption…) are added, not reported; the
    // aliased `Code` style is found.
    assert!(warnings.contains(&"docx template is missing paragraph style: Literal".to_string()));
    assert!(warnings.contains(&"docx template is missing table style: Code Table".to_string()));
    assert!(warnings
        .iter()
        .all(|w| !w.ends_with(": Heading 1") && !w.ends_with(": Code")));
    assert!(!warnings.iter().any(|w| w.ends_with("style: Hyperlink")));
}
