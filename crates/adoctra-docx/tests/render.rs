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
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            let text = String::from_utf8_lossy(&bytes).into_owned();
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

fn document(src: &str) -> String {
    part(&render(src).bytes, "word/document.xml")
}

/// The paragraphs of WordprocessingML markup: style id and text (`\t` for
/// tabs, `\n` for breaks).
fn paragraphs(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find("<w:p") {
        let after = &rest[at + 4..];
        if !(after.starts_with('>') || after.starts_with(' ')) {
            rest = after;
            continue;
        }
        let end = after.find("</w:p>").unwrap_or(after.len());
        let p = &after[..end];
        rest = &after[end..];
        let style = p
            .split_once(r#"<w:pStyle w:val=""#)
            .and_then(|(_, s)| s.split_once('"'))
            .map_or(String::new(), |(s, _)| s.to_string());
        let mut text = String::new();
        let mut q = p;
        while let Some(i) = q.find('<') {
            let tag_end = q[i..].find('>').map_or(q.len(), |e| i + e + 1);
            let tag = &q[i..tag_end];
            if tag.starts_with("<w:t>") || tag.starts_with("<w:t ") {
                let close = q[tag_end..].find("</w:t>").unwrap() + tag_end;
                text.push_str(&q[tag_end..close]);
                q = &q[close..];
                continue;
            }
            match tag {
                "<w:tab/>" => text.push('\t'),
                "<w:br/>" => text.push('\n'),
                _ => {}
            }
            q = &q[tag_end..];
        }
        let text = text
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
        // The spacer after a block's table.
        if text.is_empty() && p.contains(r#"w:line="160" w:lineRule="exact""#) {
            continue;
        }
        out.push((style, text));
    }
    out
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
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
            "docProps/core.xml",
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
    let src = "= Title\nAnn Lee\n\ntext <<a>> https://example.org[x]\n\n[[a]]\n== A\n";
    assert_eq!(render(src).bytes, render(src).bytes);
}

#[test]
fn fills_the_header_markers_of_the_article_template() {
    let doc = document(
        "= Main *Title*: The Sub\nJane Doe <j@x.org>; Bob Roe\nv1.2, 2026-01-02: Draft & more\n",
    );
    assert_eq!(
        paragraphs(&doc),
        pairs(&[
            ("Title", "Main Title"),
            ("Subtitle", "The Sub"),
            ("DocumentDetails", "Jane Doe, Bob Roe"),
            ("DocumentDetails", "version 1.2, 2026-01-02: Draft & more"),
        ])
    );
    // The title keeps its formatting.
    assert!(doc.contains(
        r#"<w:r><w:rPr><w:rStyle w:val="Strong"/></w:rPr><w:t xml:space="preserve">Title</w:t></w:r>"#
    ));
    assert!(!doc.contains("<w:sdt>"));
}

#[test]
fn the_title_separator_and_notitle_attributes_apply() {
    let ps = paragraphs(&document("= A | B: C\n:title-separator: |\n"));
    assert_eq!(ps, pairs(&[("Title", "A"), ("Subtitle", "B: C")]));
    assert!(paragraphs(&document("= A\n:notitle:\n\ntext\n"))
        .iter()
        .all(|(style, _)| style != "Title"));
}

#[test]
fn the_version_label_attribute_localizes_the_revision_line() {
    let doc = document("= T\nA B\nv2, 2026-03-04\n:version-label: Версия\n");
    assert!(doc.contains(r#"<w:t xml:space="preserve">версия 2, 2026-03-04</w:t>"#));
}

#[test]
fn writes_core_properties_from_the_header() {
    let out = render(
        "= Main & *Co*: Sub\nJane Doe; Bob Roe\n:description: About it\n:keywords: a, b\n:docdatetime: 2026-11-04 10:20:30 +0300\n",
    );
    let core = part(&out.bytes, "docProps/core.xml");
    assert!(
        core.contains("<dc:title>Main &amp; Co: Sub</dc:title>"),
        "{core}"
    );
    assert!(core.contains("<dc:creator>Jane Doe; Bob Roe</dc:creator>"));
    assert!(core.contains("<dc:description>About it</dc:description>"));
    assert!(core.contains("<cp:keywords>a, b</cp:keywords>"));
    assert!(core.contains(">2026-11-04T10:20:30+03:00</dcterms:created>"));
    let reproducible = render("= T\n:docdatetime: 2026-11-04 10:20:30 +0300\n:reproducible:\n");
    assert!(!part(&reproducible.bytes, "docProps/core.xml").contains("dcterms:created"));
}

#[test]
fn sections_get_heading_styles_numbers_and_bookmarks() {
    let doc = document(
        "= Doc\n:sectnums:\n\n[[intro-1]]\n== Intro\n\n=== Deeper\n\n[appendix]\n== Extra\n\n[discrete]\n=== Loose\n",
    );
    let ps = paragraphs(&doc);
    assert_eq!(
        ps[1..],
        pairs(&[
            ("Heading1", "1. Intro"),
            ("Heading2", "1.1. Deeper"),
            ("AppendixHeading1", "Appendix A: Extra"),
            ("FloatingTitle2", "Loose"),
        ])
    );
    assert!(
        doc.contains(r#"<w:bookmarkStart w:id="0" w:name="intro_1"/><w:bookmarkEnd w:id="0"/>"#)
    );
    assert!(doc.contains(r#"w:name="_deeper""#));
}

#[test]
fn book_parts_and_chapters_start_sections_with_a_first_page_header() {
    let out = render(
        "= A Book\n:doctype: book\n\nPreface text.\n\n= Part One\n\n== Chapter\n\ntext\n\n== Two\n\ntext\n",
    );
    let doc = part(&out.bytes, "word/document.xml");
    let ps = paragraphs(&doc);
    assert!(
        ps.contains(&("PartTitle".into(), "Part One".into())),
        "{ps:?}"
    );
    // The cover, the preface, the part and the first chapter end sections;
    // the last chapter's section is the template's final one.
    assert_eq!(doc.matches("<w:sectPr>").count(), 5);
    // Only the first body section restarts page numbers.
    assert_eq!(doc.matches(r#"w:start="1""#).count(), 1);
    assert_eq!(doc.matches("<w:titlePg/>").count(), 4);
    // The running header shows the title.
    let header = part(&out.bytes, "word/header1.xml");
    assert!(
        header.contains(r#"<w:t xml:space="preserve">A Book</w:t>"#),
        "{header}"
    );
    assert!(!header.contains("<w:sdt>"));
}

#[test]
fn articles_keep_one_section() {
    let doc = document("= An Article\n\n== A\n\ntext\n\n== B\n");
    assert_eq!(doc.matches("<w:sectPr>").count(), 1);
}

#[test]
fn paragraphs_get_lead_roles_alignment_and_titles() {
    let doc = document(
        "= T\n\nFirst.\n\nSecond.\n\n== S\n\n[.lead]\nLed.\n\n[.text-center]\n.Named\nCentered.\n",
    );
    assert_eq!(
        paragraphs(&doc)[1..],
        pairs(&[
            ("Lead", "First."),
            ("Normal", "Second."),
            ("Heading1", "S"),
            ("Lead", "Led."),
            ("BlockTitle", "Named"),
            ("Normal", "Centered."),
        ])
    );
    assert!(doc.contains(r#"<w:pStyle w:val="Normal"/><w:jc w:val="center"/>"#));
}

#[test]
fn inline_formatting_nests_styles_and_direct_properties() {
    let doc = document("*bold _both_* `code` #mark# x^2^ H~2~O line +\nbreak\nnext");
    assert!(
        doc.contains(
            r#"<w:r><w:rPr><w:rStyle w:val="Strong"/><w:i/><w:iCs/></w:rPr><w:t xml:space="preserve">both</w:t></w:r>"#
        ),
        "{doc}"
    );
    assert!(doc
        .contains(r#"<w:rStyle w:val="Monospace"/></w:rPr><w:t xml:space="preserve">code</w:t>"#));
    assert!(doc.contains(r#"<w:rStyle w:val="Mark"/></w:rPr><w:t xml:space="preserve">mark</w:t>"#));
    assert!(doc.contains(
        r#"<w:vertAlign w:val="superscript"/></w:rPr><w:t xml:space="preserve">2</w:t>"#
    ));
    assert!(doc
        .contains(r#"<w:vertAlign w:val="subscript"/></w:rPr><w:t xml:space="preserve">2</w:t>"#));
    assert_eq!(
        paragraphs(&doc),
        pairs(&[("Normal", "bold both code mark x2 H2O line\nbreak next")])
    );
}

#[test]
fn replacements_and_raw_values_become_text() {
    let doc = document("(C) A -- B... pass:[<b>raw</b> &amp;] it's");
    assert_eq!(
        paragraphs(&doc),
        pairs(&[("Normal", "© A\u{2009}—\u{2009}B…\u{200b} raw & it\u{2019}s")])
    );
}

#[test]
fn links_and_cross_references() {
    let out = render(
        "See https://example.org/?a&b[the site], https://example.org/?a&b and <<target,there>> or <<target>>.\n\n[[target]]\n== Target\n",
    );
    let doc = part(&out.bytes, "word/document.xml");
    let rels = part(&out.bytes, "word/_rels/document.xml.rels");
    // One relationship per URL.
    assert_eq!(
        rels.matches("https://example.org/?a&amp;b").count(),
        1,
        "{rels}"
    );
    assert_eq!(doc.matches(r#"<w:hyperlink r:id="rId"#).count(), 2);
    assert!(doc.contains(
        r#"<w:hyperlink w:anchor="target" w:history="1"><w:r><w:rPr><w:rStyle w:val="Xref"/></w:rPr><w:t xml:space="preserve">there</w:t></w:r></w:hyperlink>"#
    ));
    // A textless reference shows the section title core resolved.
    assert!(doc.contains(r#"<w:t xml:space="preserve">Target</w:t></w:r></w:hyperlink>"#));
    assert!(doc.contains(
        r#"<w:rStyle w:val="Hyperlink"/></w:rPr><w:t xml:space="preserve">the site</w:t>"#
    ));
    assert!(doc.contains(r#"w:name="target""#));
    // Interdocument references link to the other file.
    let other = render("See <<other.adoc#sec,elsewhere>>.");
    assert!(part(&other.bytes, "word/_rels/document.xml.rels")
        .contains(r#"Target="other.html#sec" TargetMode="External""#));
}

#[test]
fn inline_anchors_and_monospace_links_keep_both_meanings() {
    let doc = document("[[here]]Text with `https://example.org[code link]`.");
    assert!(
        doc.contains(r#"<w:bookmarkStart w:id="0" w:name="here"/>"#),
        "{doc}"
    );
    // The link inside monospace keeps the code style and is underlined.
    assert!(
        doc.contains(r#"<w:rStyle w:val="Monospace"/><w:u w:val="single"/>"#),
        "{doc}"
    );
}

#[test]
fn ui_macros_use_their_character_styles() {
    let doc = document(":experimental:\n\nkbd:[Ctrl+T] btn:[Save] menu:File[Open]");
    assert!(
        doc.contains(r#"<w:rStyle w:val="Keyboard"/></w:rPr><w:t xml:space="preserve">Ctrl</w:t>"#),
        "{doc}"
    );
    assert!(doc.contains(r#"<w:rStyle w:val="Button"/></w:rPr><w:t xml:space="preserve">[</w:t>"#));
    assert!(doc.contains(r#"<w:rStyle w:val="Menu"/></w:rPr><w:t xml:space="preserve">Open</w:t>"#));
    assert_eq!(
        paragraphs(&doc),
        pairs(&[("Normal", "Ctrl+T [Save] File\u{a0}\u{203a} Open")])
    );
}

#[test]
fn verbatim_verse_and_quote_blocks() {
    let doc = document(concat!(
        "[source]\n----\nfn main() {\n\tx < 1\n}\n----\n\n",
        "[verse, Poet, Poem]\n____\nline one\nline two\n____\n\n",
        "[quote, Author]\n____\nQuoted.\n\nMore.\n____\n",
    ));
    assert_eq!(
        paragraphs(&doc),
        pairs(&[
            ("Code", "fn main() {"),
            ("Code", "\tx < 1"),
            ("Code", "}"),
            ("Verse", "line one\nline two"),
            ("VerseAttribution", "\u{2014} Poet\nPoem"),
            ("Quote", "Quoted."),
            ("Quote", "More."),
            ("QuoteAttribution", "\u{2014} Author"),
        ])
    );
}

#[test]
fn breaks_and_containers() {
    let doc = document(concat!(
        "a\n\n",
        "'''\n\n",
        "<<<\n\n",
        ".Ex\n====\ninside\n====\n\n",
        "NOTE: careful\n",
    ));
    assert!(doc.contains(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#));
    assert_eq!(
        paragraphs(&doc),
        pairs(&[
            ("Normal", "a"),
            ("HorizontalRule", ""),
            ("", ""),
            ("BlockTitle", "Example 1. Ex"),
            ("Normal", "inside"),
            ("AdmonitionLabel", "Note"),
            ("Normal", "careful"),
        ])
    );
}

#[test]
fn unconverted_blocks_are_reported_once_per_kind() {
    let out =
        render("* a\n* b\n\n* c\n\n|===\n|x\n|===\n\nimage::a.png[]\n\n++++\n<p>raw</p>\n++++\n");
    assert_eq!(
        out.warnings,
        [
            "images cannot be read without a resource provider: a.png",
            "docx backend drops passthrough blocks (raw output has no DOCX form)",
        ]
    );
    assert!(render("= T\n\nparagraph\n").warnings.is_empty());
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
    // No marker: the specimen body is replaced, the section stays, and the
    // title opens the body in Word's built-in Title style (added).
    assert_eq!(
        part(&out.bytes, "word/document.xml"),
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t xml:space="preserve">T</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:body></w:document>"#
    );
    assert!(part(&out.bytes, "word/styles.xml").contains(
        r#"<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="a"/>"#
    ));
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

/// The `w:numId` of each numbered paragraph, in order.
fn num_ids(xml: &str) -> Vec<String> {
    xml.split(r#"<w:numId w:val=""#)
        .skip(1)
        .map(|s| s.split('"').next().unwrap().to_string())
        .collect()
}

#[test]
fn unordered_lists_nest_with_their_own_bullets_and_indents() {
    let out = render("* one\n** nested\n* two\n+\ncontinued\n\n[square]\n* sq\n");
    let doc = part(&out.bytes, "word/document.xml");
    let numbering = part(&out.bytes, "word/numbering.xml");
    assert_eq!(
        paragraphs(&doc),
        pairs(&[
            ("ListParagraph", "one"),
            ("ListParagraph", "nested"),
            ("ListParagraph", "two"),
            ("Normal", "continued"),
            ("ListParagraph", "sq"),
        ])
    );
    // Each list has its own definition; the nested one indents further.
    assert_eq!(num_ids(&doc), ["1", "2", "1", "3"]);
    assert!(numbering.contains(r#"<w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/>"#), "{numbering}");
    assert!(numbering
        .contains(r#"<w:lvlText w:val="◦"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="1440""#));
    assert!(numbering.contains(r#"<w:lvlText w:val="▪"/>"#));
    // An attached block aligns with the item text.
    assert!(doc.contains(r#"<w:pStyle w:val="Normal"/><w:ind w:left="720"/>"#));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

#[test]
fn ordered_lists_follow_ruby_styles_and_start() {
    let out = render(
        ". a\n.. b\n\n[upperroman, start=4]\n. c\n\n[lowergreek%reversed]\n. d\n\n[unstyled]\n. e\n",
    );
    let numbering = part(&out.bytes, "word/numbering.xml");
    for (start, format) in [
        (1, "decimal"),
        (1, "lowerLetter"),
        (4, "upperRoman"),
        (1, "lowerLetter"),
    ] {
        assert!(
            numbering.contains(&format!(
                r#"<w:start w:val="{start}"/><w:numFmt w:val="{format}"/><w:lvlText w:val="%1."/>"#
            )),
            "{start} {format}: {numbering}"
        );
    }
    assert!(numbering
        .contains(r#"<w:numFmt w:val="none"/><w:suff w:val="nothing"/><w:lvlText w:val=""/>"#));
    assert_eq!(
        out.warnings,
        [
            "docx backend numbers lowergreek lists with latin letters",
            "docx backend does not support reversed lists",
        ]
    );
}

#[test]
fn checklists_show_ruby_check_marks_without_bullets() {
    let out = render("* [x] done\n* [ ] todo\n* plain\n\n:icons: font\n\n* [x] font\n");
    let doc = part(&out.bytes, "word/document.xml");
    assert_eq!(
        paragraphs(&doc),
        pairs(&[
            ("ListParagraph", "\u{2713} done"),
            ("ListParagraph", "\u{274f} todo"),
            ("ListParagraph", "plain"),
            ("ListParagraph", "\u{2611} font"),
        ])
    );
    let numbering = part(&out.bytes, "word/numbering.xml");
    assert_eq!(numbering.matches(r#"<w:numFmt w:val="none"/>"#).count(), 2);
}

#[test]
fn description_lists_use_term_and_description_styles() {
    let doc =
        document("CPU:: The brain.\n+\nMore.\nRAM::\nDisk:: Storage.\n* in list\nterm::: nested\n");
    assert_eq!(
        paragraphs(&doc),
        pairs(&[
            ("Term", "CPU"),
            ("Description", "The brain."),
            ("Normal", "More."),
            ("Term", "RAM"),
            ("Term", "Disk"),
            ("Description", "Storage."),
            ("ListParagraph", "in list"),
            ("Term", "term"),
            ("Description", "nested"),
        ])
    );
    // Blocks of a description align with its text; the top-level
    // description text is indented by its style.
    assert!(
        doc.contains(r#"<w:pStyle w:val="Normal"/><w:ind w:left="360"/>"#),
        "{doc}"
    );
    assert!(doc.contains(r#"<w:pStyle w:val="Description"/></w:pPr>"#));
    // A nested list's terms and descriptions indent with their container.
    assert!(
        doc.contains(r#"<w:pStyle w:val="Term"/><w:ind w:left="1080"/>"#),
        "{doc}"
    );
    assert!(doc.contains(r#"<w:pStyle w:val="Description"/><w:ind w:left="1440"/>"#));
}

#[test]
fn qanda_lists_number_the_questions() {
    let out = render("[qanda]\nWhat?:: This.\nWhy?:: Because.\n");
    let doc = part(&out.bytes, "word/document.xml");
    assert_eq!(
        paragraphs(&doc),
        pairs(&[
            ("Question", "What?"),
            ("Answer", "This."),
            ("Question", "Why?"),
            ("Answer", "Because."),
        ])
    );
    assert_eq!(num_ids(&doc), ["1", "1"]);
    assert!(doc.contains(r#"<w:pStyle w:val="Answer"/><w:ind w:left="720"/>"#));
    assert!(part(&out.bytes, "word/numbering.xml").contains(r#"<w:numFmt w:val="decimal"/>"#));
}

#[test]
fn horizontal_lists_are_borderless_tables() {
    let doc = document("[horizontal, labelwidth=20]\nCPU:: The brain.\nRAM::\n");
    assert!(
        doc.contains(r#"<w:tblGrid><w:gridCol w:w="1927"/><w:gridCol w:w="7710"/></w:tblGrid>"#),
        "{doc}"
    );
    assert!(doc.contains(r#"<w:tblLayout w:type="fixed"/>"#));
    assert!(doc.contains(r#"<w:insideV w:val="nil"/>"#));
    assert_eq!(doc.matches("<w:tr>").count(), 2);
    // An empty description cell still ends with a paragraph.
    assert!(doc.contains(r#"<w:tcW w:w="7710" w:type="dxa"/></w:tcPr><w:p/></w:tc>"#));
    assert_eq!(
        paragraphs(&doc),
        pairs(&[("Term", "CPU"), ("Normal", "The brain."), ("Term", "RAM")])
    );
    let auto = document("[horizontal]\nA:: b\n");
    assert!(auto.contains(r#"<w:tblLayout w:type="autofit"/>"#));
}

#[test]
fn callout_lists_number_items_in_the_callout_style() {
    let doc = document("----\nx <1>\ny <2>\n----\n<1> First\n<2> Second\n");
    assert!(doc.contains(
        r#"<w:pStyle w:val="ListParagraph"/><w:ind w:left="720" w:hanging="360"/></w:pPr><w:r><w:rPr><w:rStyle w:val="Callout"/></w:rPr><w:t xml:space="preserve">1</w:t></w:r><w:r><w:tab/></w:r>"#
    ), "{doc}");
    let ps = paragraphs(&doc);
    assert_eq!(
        ps[ps.len() - 2..],
        pairs(&[
            ("ListParagraph", "1\tFirst"),
            ("ListParagraph", "2\tSecond")
        ])
    );
}

#[test]
fn lists_inside_a_custom_template_create_its_numbering_part() {
    let doc = parse("* a\n").document;
    let out = DocxBackend::new()
        .with_template(word_template())
        .render(&doc)
        .unwrap();
    let rels = part(&out.bytes, "word/_rels/document.xml.rels");
    assert!(
        rels.contains(r#"relationships/numbering" Target="numbering.xml""#),
        "{rels}"
    );
    assert!(part(&out.bytes, "[Content_Types].xml").contains(r#"PartName="/word/numbering.xml""#));
    assert!(part(&out.bytes, "word/numbering.xml").contains(r#"<w:num w:numId="1">"#));
    // Word's built-in List Paragraph is added to the template.
    assert!(part(&out.bytes, "word/styles.xml").contains(r#"w:styleId="ListParagraph""#));
}

#[test]
fn callout_marks_in_verbatim_blocks_use_the_callout_style() {
    let doc = document("----\nx = 1 # <1>\ny <2> <3>\n<!--4-->\n----\n");
    assert_eq!(
        paragraphs(&doc),
        pairs(&[
            ("Code", "x = 1 # 1"),
            ("Code", "y 2 3"),
            ("Code", "<!--4-->")
        ])
    );
    assert_eq!(doc.matches(r#"<w:rStyle w:val="Callout"/>"#).count(), 4);
    assert!(doc.contains(
        r#"<w:t xml:space="preserve">x = 1 </w:t></w:r><w:r><w:t xml:space="preserve"># </w:t></w:r><w:r><w:rPr><w:rStyle w:val="Callout"/></w:rPr><w:t xml:space="preserve">1</w:t>"#
    ), "{doc}");
}

#[test]
fn delimited_blocks_are_framed_by_their_table_styles() {
    let doc = document(concat!(
        "----\ncode\n----\n\n....\nlit\n....\n\n",
        ".Ex\n====\ninside\n====\n\n",
        ".Side\n****\naside\n****\n\n",
        "[quote, A]\n____\nq\n____\n\n",
        "[verse]\n____\nv\n____\n\n",
        "TIP: hint\n",
    ));
    for style in [
        "CodeTable",
        "LiteralTable",
        "ExampleTable",
        "SidebarTable",
        "QuoteTable",
        "VerseTable",
        "AdmonitionTipTable",
    ] {
        assert!(
            doc.contains(&format!(r#"<w:tblStyle w:val="{style}"/>"#)),
            "{style}: {doc}"
        );
    }
    assert_eq!(
        paragraphs(&doc),
        pairs(&[
            ("Code", "code"),
            ("Literal", "lit"),
            ("BlockTitle", "Example 1. Ex"),
            ("Normal", "inside"),
            ("SidebarTitle", "Side"),
            ("Normal", "aside"),
            ("Quote", "q"),
            ("QuoteAttribution", "\u{2014} A"),
            ("Verse", "v"),
            ("AdmonitionLabel", "Tip"),
            ("Normal", "hint"),
        ])
    );
    // The example's title stands above its box, the sidebar's inside.
    let example = doc.find(r#"w:val="ExampleTable""#).unwrap();
    assert!(doc.find("Example 1. ").unwrap() < example);
    let sidebar = doc.find(r#"w:val="SidebarTable""#).unwrap();
    assert!(doc.find(">Side<").unwrap() > sidebar);
    // An admonition is a label cell and a content cell; its label column
    // takes the style's first-column formatting.
    assert!(doc.contains(r#"w:firstColumn="1""#));
    assert!(
        doc.contains(r#"<w:gridCol w:w="1606"/><w:gridCol w:w="8032"/>"#),
        "{doc}"
    );
    // Adjacent tables are kept apart.
    assert!(!doc.contains("</w:tbl><w:tbl>"));
}

#[test]
fn a_template_without_a_specific_admonition_style_uses_the_generic_one() {
    let doc = parse("WARNING: careful\n").document;
    let out = DocxBackend::new()
        .with_template(word_template())
        .render(&doc)
        .unwrap();
    // The generic style is reported once; the specific one is not looked up.
    assert_eq!(
        out.warnings,
        [
            "docx template is missing table style: Admonition Table",
            "docx template is missing paragraph style: Admonition Label",
        ]
    );
    assert!(!part(&out.bytes, "word/document.xml").contains("<w:tblStyle"));
}

#[test]
fn nested_frames_and_lists_indent_their_tables() {
    let doc = document("* item\n+\n----\ncode\n----\n");
    assert!(
        doc.contains(r#"<w:tblW w:w="8918" w:type="dxa"/><w:tblInd w:w="720" w:type="dxa"/>"#),
        "{doc}"
    );
    // Content inside a frame is not indented again.
    assert!(!doc.contains(r#"<w:pStyle w:val="Code"/><w:ind"#));
}

#[test]
fn tables_map_columns_spans_header_footer_and_alignment() {
    let doc = document(concat!(
        "[cols=\"1,2,1\", options=\"header,footer\", stripes=even]\n",
        "|===\n",
        "|H1 |H2 |H3\n",
        "\n",
        ".2+^.^s|tall 2+>|wide\n",
        "e|it m|mono\n",
        "|f1 |f2 |f3\n",
        "|===\n",
    ));
    assert!(
        doc.contains(r#"<w:tblStyle w:val="Table"/><w:tblW w:w="9638" w:type="dxa"/>"#),
        "{doc}"
    );
    assert!(
        doc.contains(r#"<w:gridCol w:w="2410"/><w:gridCol w:w="4819"/><w:gridCol w:w="2410"/>"#)
    );
    assert!(doc.contains(r#"w:firstRow="1" w:lastRow="1""#));
    assert!(doc.contains(r#"<w:tr><w:trPr><w:tblHeader/></w:trPr>"#));
    // Row and column spans.
    assert!(doc.contains(r#"<w:tcW w:w="2410" w:type="dxa"/><w:vMerge w:val="restart"/><w:vAlign w:val="center"/>"#), "{doc}");
    assert!(doc.contains(r#"<w:tcW w:w="7229" w:type="dxa"/><w:gridSpan w:val="2"/>"#));
    assert!(doc.contains(r#"<w:tcW w:w="2410" w:type="dxa"/><w:vMerge/></w:tcPr><w:p/></w:tc>"#));
    // Cell styles and alignment.
    assert!(doc.contains(r#"<w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:rStyle w:val="Strong"/></w:rPr><w:t xml:space="preserve">tall</w:t>"#));
    assert!(
        doc.contains(r#"<w:jc w:val="right"/></w:pPr><w:r><w:t xml:space="preserve">wide</w:t>"#)
    );
    assert!(
        doc.contains(r#"<w:rStyle w:val="Emphasis"/></w:rPr><w:t xml:space="preserve">it</w:t>"#)
    );
    assert!(doc
        .contains(r#"<w:rStyle w:val="Monospace"/></w:rPr><w:t xml:space="preserve">mono</w:t>"#));
    // `stripes=even` shades the second body row.
    assert_eq!(doc.matches(r#"w:fill="F8F8F7""#).count(), 2, "{doc}");
}

#[test]
fn table_frame_grid_width_caption_and_cell_kinds() {
    let doc = document(concat!(
        ".Data\n",
        "[frame=ends, grid=rows, width=50%, cols=\"a,l\"]\n",
        "|===\n",
        "|* in cell\n| verbatim\n  text\n",
        "|===\n",
    ));
    assert!(
        doc.contains(r#"<w:tblW w:w="4819" w:type="dxa"/>"#),
        "{doc}"
    );
    assert!(doc.contains(r#"<w:left w:val="nil"/>"#));
    assert!(doc.contains(r#"<w:insideH w:val="single""#));
    let ps = paragraphs(&doc);
    assert_eq!(ps[0], ("Caption".into(), "Table 1. Data".into()));
    assert!(ps.contains(&("ListParagraph".into(), "in cell".into())));
    assert!(
        ps.contains(&("Literal".into(), " verbatim\n  text".into())),
        "{ps:?}"
    );
    let auto = document("[%autowidth]\n|===\n|a |b\n|===\n");
    assert!(auto.contains(r#"<w:tblW w:w="0" w:type="auto"/>"#));
    assert!(auto.contains(r#"<w:tblLayout w:type="autofit"/>"#));
}

#[test]
fn cell_backgrounds_come_from_cellbgcolor() {
    let doc = document("|===\n|{set:cellbgcolor:#ff0000}red |plain\n|===\n");
    assert!(doc.contains(r#"w:fill="FF0000""#), "{doc}");
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes
}

fn render_with_images(src: &str, images: &[(&str, Vec<u8>)]) -> adoctra_docx::Output {
    let mut resources = adoctra_core::MemoryResources::new();
    for (name, bytes) in images {
        resources.insert_binary(name, bytes.clone()).unwrap();
    }
    DocxBackend::new()
        .with_resources(std::sync::Arc::new(resources), None)
        .render(&parse(src).document)
        .unwrap()
}

#[test]
fn block_images_are_embedded_sized_and_captioned() {
    let out = render_with_images(
        ".A figure\nimage::img/a.png[Alt text,width=150,align=center]\n\nimage::img/a.png[]\n\nimage::big.png[float=right]\n",
        &[("img/a.png", png(300, 200)), ("big.png", png(2000, 1000))],
    );
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let doc = part(&out.bytes, "word/document.xml");
    // One media part per image file, declared once.
    assert!(!part(&out.bytes, "word/media/image1.png").is_empty());
    assert!(parts(&out.bytes)
        .iter()
        .all(|(n, _)| n != "word/media/image3.png"));
    let types = part(&out.bytes, "[Content_Types].xml");
    assert_eq!(types.matches(r#"Extension="png""#).count(), 1, "{types}");
    // 150 px wide, the ratio kept (EMU at 96 DPI).
    assert!(
        doc.contains(r#"<wp:extent cx="1428750" cy="952500"/>"#),
        "{doc}"
    );
    assert!(doc.contains(r#"descr="Alt text""#));
    assert!(doc.contains(r#"<w:keepNext/><w:jc w:val="center"/>"#));
    // The caption below, Ruby's figure caption.
    let ps = paragraphs(&doc);
    assert_eq!(ps[1], ("Caption".into(), "Figure 1. A figure".into()));
    // Too wide for the text: scaled to the 9638 twips text width.
    assert!(
        doc.contains(r#"<wp:extent cx="6120130" cy="3060065"/>"#),
        "{doc}"
    );
    assert!(doc.contains("<wp:align>right</wp:align>"));
    assert!(doc.starts_with("<?xml") && doc.contains(r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#));
}

#[test]
fn missing_images_and_uris() {
    let out = render_with_images(
        "image::missing.png[Gone]\n\nimage::https://example.org/a.png[Web]\n",
        &[],
    );
    assert_eq!(out.warnings, ["image not found or unreadable: missing.png"]);
    let doc = part(&out.bytes, "word/document.xml");
    assert!(paragraphs(&doc).contains(&("Normal".into(), "[Gone]".into())));
    // A URI is linked, not read.
    assert!(doc.contains(r#"<a:blip r:link="rId"#), "{doc}");
    assert!(part(&out.bytes, "word/_rels/document.xml.rels")
        .contains(r#"Target="https://example.org/a.png" TargetMode="External""#));
}

#[test]
fn inline_images_and_icons() {
    let out = render_with_images(
        "Logo image:logo.png[Logo,32] and icon:heart[] here.",
        &[("logo.png", png(64, 64))],
    );
    let doc = part(&out.bytes, "word/document.xml");
    assert!(doc.contains(r#"<wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="304800" cy="304800"/>"#), "{doc}");
    // A text icon is its alt text in brackets, like Ruby's.
    assert_eq!(
        paragraphs(&doc),
        pairs(&[("Normal", "Logo  and [heart] here.")])
    );
}

#[test]
fn svg_images_get_a_fallback_picture() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"/>"#.to_vec();
    let out = render_with_images("image::d.svg[]\n", &[("d.svg", svg)]);
    let doc = part(&out.bytes, "word/document.xml");
    assert!(doc.contains("asvg:svgBlip"));
    assert!(doc.contains(r#"<wp:extent cx="381000" cy="190500"/>"#));
    assert!(part(&out.bytes, "[Content_Types].xml").contains(r#"Extension="svg""#));
}

#[test]
fn footnotes_become_word_footnotes() {
    let out = render(
        "A.footnote:[First *note*.] B.footnote:disc[Shared.] C.footnote:disc[] D.footnote:nope[]",
    );
    let doc = part(&out.bytes, "word/document.xml");
    let notes = part(&out.bytes, "word/footnotes.xml");
    assert!(doc.contains(r#"<w:footnoteReference w:id="1"/>"#));
    assert!(doc.contains(r#"<w:footnoteReference w:id="2"/>"#));
    // The repeated footnote refers back to its mark.
    assert!(doc.contains("NOTEREF _Ref_footnote_2 \\f \\h"), "{doc}");
    assert!(doc.contains(r#"<w:bookmarkStart w:id="1" w:name="_Ref_footnote_2"/>"#));
    // An unknown footnote shows its id, like Ruby.
    assert!(doc.contains(">[nope]<"));
    assert!(notes.contains(r#"<w:footnote w:type="separator" w:id="-1">"#));
    assert!(notes.contains(r#"<w:footnote w:id="1"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r>"#), "{notes}");
    assert!(
        notes.contains(r#"<w:rStyle w:val="Strong"/></w:rPr><w:t xml:space="preserve">note</w:t>"#)
    );
    assert!(part(&out.bytes, "word/_rels/document.xml.rels").contains("relationships/footnotes"));
    assert!(part(&out.bytes, "[Content_Types].xml").contains("/word/footnotes.xml"));
}

#[test]
fn the_table_of_contents_links_core_outline() {
    let doc = document(
        "= Doc\n:toc:\n:toc-title: Содержание\n:sectnums:\n\n== One [[x]]x\n\n=== Sub\n\n==== Deep\n\n== Two\n",
    );
    assert!(doc.contains(r#"<w:docPartGallery w:val="Table of Contents"/>"#));
    assert!(
        doc.contains(r#"<w:instrText xml:space="preserve"> TOC \o "1-2" \h \z \u </w:instrText>"#)
    );
    assert!(doc.contains(r#"w:dirty="true""#));
    let ps = paragraphs(&doc);
    assert_eq!(
        ps[1..5],
        pairs(&[
            ("TOCHeading", "Содержание"),
            ("TOC1", "1. One x"),
            ("TOC2", "1.1. Sub"),
            ("TOC1", "2. Two"),
        ])
    );
    // Entries link to the headings' bookmarks; the heading keeps its own
    // inline anchor.
    assert!(
        doc.contains(r#"<w:hyperlink w:anchor="_one_x" w:history="1">"#),
        "{doc}"
    );
    assert_eq!(doc.matches(r#"w:name="x""#).count(), 1);
    let toc_end = doc.find("</w:sdt>").unwrap();
    assert!(doc.find(r#"w:name="x""#).unwrap() > toc_end);
}

#[test]
fn toc_macro_and_preamble_placements() {
    let doc = document("= Doc\n:toc: macro\n\n== A\n\n.Contents\ntoc::[]\n\n== B\n");
    let ps = paragraphs(&doc);
    let at = ps.iter().position(|p| p.0 == "TOCHeading").unwrap();
    assert_eq!(ps[at].1, "Contents");
    assert_eq!(ps[at - 1].1, "A");
    let doc = document("= Doc\n:toc: preamble\n\nIntro.\n\n== A\n");
    let ps = paragraphs(&doc);
    let at = ps.iter().position(|p| p.0 == "TOCHeading").unwrap();
    assert_eq!(ps[at - 1].1, "Intro.");
}

#[test]
fn index_terms_become_xe_fields() {
    let doc = document(concat!(
        "The ((Tiger)) and (((Big cats, Lions))) and ((Puma >> Big cats)).\n\n",
        "[index]\n== Index\n",
    ));
    assert!(
        doc.contains(r#"<w:instrText xml:space="preserve"> XE "Tiger" </w:instrText>"#),
        "{doc}"
    );
    assert!(
        doc.contains(r#"<w:instrText xml:space="preserve"> XE "Big cats:Lions" </w:instrText>"#)
    );
    assert!(doc.contains(r#"XE "Puma" \t "See Big cats""#));
    assert!(doc.contains(r#"<w:instrText xml:space="preserve"> INDEX \c "2" </w:instrText>"#));
    assert_eq!(
        paragraphs(&doc)[0],
        ("Normal".into(), "The Tiger and  and Puma.".into())
    );
}

#[test]
fn video_and_audio_are_links() {
    let out = render("video::clip.mp4[]\n");
    assert_eq!(
        out.warnings,
        ["docx backend links video and audio instead of embedding them"]
    );
    let doc = part(&out.bytes, "word/document.xml");
    assert!(part(&out.bytes, "word/_rels/document.xml.rels").contains(r#"Target="clip.mp4""#));
    assert!(doc.contains(">clip.mp4<"));
}

#[test]
fn rouge_colors_source_listings_with_the_theme() {
    let src =
        ":source-highlighter: rouge\n\n[source,ruby]\n----\nputs \"hi\" # <1>\n----\n<1> Greets\n";
    let doc = document(src);
    let ps = paragraphs(&doc);
    assert_eq!(ps[0], ("Code".into(), "puts \"hi\" # 1".into()), "{ps:?}");
    // github's string color, and the mark after the code in the Callout style.
    assert!(
        doc.contains(r#"<w:color w:val="0A3069"/></w:rPr><w:t xml:space="preserve">"hi"</w:t>"#),
        "{doc}"
    );
    assert!(doc.contains(r#"<w:rStyle w:val="Callout"/></w:rPr><w:t xml:space="preserve">1</w:t>"#));
    // Without the highlighter the listing is plain.
    assert!(!document("[source,ruby]\n----\nputs 1\n----\n").contains("<w:color"));
}

#[test]
fn rouge_line_numbers_and_dark_themes() {
    let doc = document(concat!(
        ":source-highlighter: rouge\n:rouge-style: monokai\n\n",
        "[source,ruby,linenums,start=9]\n----\na = 1\nb = 2\n----\n",
    ));
    assert_eq!(
        paragraphs(&doc),
        pairs(&[("Code", " 9  a = 1"), ("Code", "10  b = 2")])
    );
    assert!(
        doc.contains(
            r#"<w:rStyle w:val="LineNumber"/></w:rPr><w:t xml:space="preserve"> 9  </w:t>"#
        ),
        "{doc}"
    );
    // The theme's background and base color.
    assert!(doc.contains(r#"<w:shd w:val="clear" w:color="auto" w:fill="49483E"/></w:tcPr>"#));
    assert!(doc.contains(r#"<w:color w:val="F8F8F2"/>"#));
}

#[test]
fn rouge_keeps_blank_edge_lines_only_when_numbered() {
    let src = |opts: &str| {
        format!(":source-highlighter: rouge\n\n[source,text{opts}]\n----\n\nx\n\n----\n")
    };
    assert_eq!(paragraphs(&document(&src(""))), pairs(&[("Code", "x")]));
    assert_eq!(
        paragraphs(&document(&src(",linenums"))),
        pairs(&[("Code", "1  "), ("Code", "2  x"), ("Code", "3  ")])
    );
}

/// The `w:pgSz` of each section, in order (the final one last).
fn page_sizes(doc: &str) -> Vec<String> {
    doc.split("<w:pgSz ")
        .skip(1)
        .map(|s| s.split("/>").next().unwrap().to_string())
        .collect()
}

#[test]
fn page_layout_breaks_turn_the_page() {
    let doc = document(concat!(
        "Portrait.\n\n",
        "[page-layout=landscape]\n<<<\n\n",
        "Wide.\n\n",
        "[page-layout=landscape]\n<<<\n\n",
        "Still wide.\n\n",
        "[page-layout=portrait]\n<<<\n\n",
        "Tall again.\n",
    ));
    assert_eq!(
        page_sizes(&doc),
        [
            r#"w:w="11906" w:h="16838""#,
            r#"w:w="16838" w:h="11906" w:orient="landscape""#,
            r#"w:w="11906" w:h="16838""#,
        ]
    );
    // A break to the current orientation is a plain page break.
    assert_eq!(doc.matches(r#"<w:br w:type="page"/>"#).count(), 1);
    // Every section keeps the template's footer.
    assert_eq!(
        doc.matches(r#"<w:footerReference w:type="default" r:id="rId5"/>"#)
            .count(),
        3
    );
}

#[test]
fn rotated_tables_get_landscape_pages_of_their_own() {
    let doc = document("Before.\n\n[%rotate]\n|===\n|a |b\n|===\n\nAfter.\n");
    assert_eq!(
        page_sizes(&doc),
        [
            r#"w:w="11906" w:h="16838""#,
            r#"w:w="16838" w:h="11906" w:orient="landscape""#,
            r#"w:w="11906" w:h="16838""#,
        ]
    );
    // The table spans the landscape text width.
    assert!(
        doc.contains(r#"<w:tblW w:w="14570" w:type="dxa"/>"#),
        "{doc}"
    );
    let table = doc.find("<w:tbl>").unwrap();
    let breaks: Vec<usize> = doc.match_indices("<w:sectPr>").map(|(i, _)| i).collect();
    assert!(breaks[0] < table && table < breaks[1]);
    // On landscape pages already, a rotated table stays in place.
    let wide = document("[page-layout=landscape]\n<<<\n\n[%rotate]\n|===\n|a\n|===\n");
    assert_eq!(page_sizes(&wide).len(), 2);
}

#[test]
fn turned_book_sections_have_no_first_page_header() {
    let out = render("= B\n:doctype: book\n\n== One\n\n[%rotate]\n|===\n|x\n|===\n\n== Two\n");
    let doc = part(&out.bytes, "word/document.xml");
    // Cover, chapter one (portrait, first page), its rotated table
    // (landscape, no first page), its rest (portrait), chapter two.
    let sections: Vec<&str> = doc.split("<w:sectPr>").skip(1).collect();
    assert_eq!(sections.len(), 5, "{doc}");
    assert!(sections[1].contains("<w:titlePg/>"));
    assert!(
        sections[2].contains(r#"w:orient="landscape""#) && !sections[2].contains("<w:titlePg/>")
    );
    assert!(!sections[3].contains("<w:titlePg/>"));
    assert!(sections[4].contains("<w:titlePg/>"));
    assert_eq!(doc.matches(r#"w:start="1""#).count(), 1);
}

#[test]
fn a_template_without_sections_ignores_orientation() {
    let mut zip_bytes = word_template();
    // Drop the template body's section properties.
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes.clone())).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).unwrap();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        let name = file.name().to_string();
        if name == "word/document.xml" {
            text = text.replace(
                r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr>"#,
                "",
            );
        }
        out.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(text.as_bytes()).unwrap();
    }
    zip_bytes = out.finish().unwrap().into_inner();
    let doc = parse("[page-layout=landscape]\n<<<\n\ntext\n").document;
    let rendered = DocxBackend::new()
        .with_template(zip_bytes)
        .render(&doc)
        .unwrap();
    assert_eq!(
        rendered.warnings,
        ["docx template has no section properties: page orientation is ignored"]
    );
    assert!(part(&rendered.bytes, "word/document.xml").contains(r#"<w:br w:type="page"/>"#));
}
