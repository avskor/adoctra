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
            "docx backend does not convert tables yet",
            "docx backend does not convert block images yet",
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
