//! Section numbering (`sectnums`), like Ruby's `initialize_section`,
//! `assign_numeral` and `convert_section`, and numbered xrefstyle labels.
use adoctra_core::{
    parse, parse_with_options, tokens, Block, BlockKind, Inline, ParseOptions, TokenKind,
};

fn text(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Inline::Text { value, .. }
            | Inline::Raw { value, .. }
            | Inline::Substituted { value, .. } => value.clone(),
            Inline::AttributeRef { name, value, .. } => {
                value.clone().unwrap_or_else(|| format!("{{{name}}}"))
            }
            Inline::Emphasis { children, .. } => format!("_{}_", text(children)),
            Inline::Strong { children, .. } | Inline::CrossReference { children, .. } => {
                text(children)
            }
            _ => panic!("{node:?}"),
        })
        .collect()
}

/// Every heading as `caption + title` with the source slice of the heading.
fn headings(blocks: &[Block], source: &str, out: &mut Vec<(String, String)>) {
    for block in blocks {
        match &block.kind {
            BlockKind::Heading { inlines, .. } => out.push((
                format!(
                    "{}{}",
                    block.caption.as_deref().unwrap_or(""),
                    text(inlines)
                ),
                source[block.span.start..block.span.end].to_owned(),
            )),
            BlockKind::Section { blocks, .. } | BlockKind::Preamble { blocks } => {
                headings(blocks, source, out)
            }
            _ => {}
        }
    }
}

fn numbered(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    headings(&parse(source).document.blocks, source, &mut out);
    out.into_iter().map(|(title, _)| title).collect()
}

fn last_paragraph(blocks: &[Block]) -> Option<&[Inline]> {
    blocks.iter().rev().find_map(|block| match &block.kind {
        BlockKind::Paragraph { inlines } => Some(inlines.as_slice()),
        BlockKind::Section { blocks, .. } | BlockKind::Preamble { blocks } => {
            last_paragraph(blocks)
        }
        _ => None,
    })
}

fn labels(source: &str) -> Vec<String> {
    let doc = parse(source).document;
    last_paragraph(&doc.blocks)
        .unwrap()
        .iter()
        .filter_map(|n| match n {
            Inline::CrossReference { children, .. } => Some(text(children)),
            _ => None,
        })
        .collect()
}

#[test]
fn article_sections_number_by_level_with_toggles_and_sectnumlevels() {
    let source = "= Doc\n:sectnums:\n\n== One\n\n=== One A\n\n==== Deep\n\n===== Deeper\n\n\
        [discrete]\n== Disc\n\n:sectnums!:\n\n== Off\n\n=== Child\n\n:sectnums:\n\n=== Child Two\n\n\
        :sectnumlevels: 1\n\n== Two\n\n=== Hidden\n\n:sectnumlevels: 4\n\n==== Shown";
    assert_eq!(
        numbered(source),
        [
            "Doc",
            "1. One",
            "1.1. One A",
            "1.1.1. Deep",
            // Beyond the default sectnumlevels (3).
            "Deeper",
            "Disc",
            "Off",
            "Child",
            // The unnumbered parent contributes an empty numeral, like Ruby.
            ".1. Child Two",
            "2. Two",
            // Numbered (it advances its parent's ordinal) but not displayed.
            "Hidden",
            "2.1.1. Shown",
        ]
    );
    // Headings keep their spans; the caption is not part of the inlines.
    let mut out = Vec::new();
    headings(&parse(source).document.blocks, source, &mut out);
    assert_eq!(out[1].1, "== One");
    let doc = parse(source).document;
    let ids: Vec<_> = doc
        .flat_blocks()
        .iter()
        .filter_map(|b| b.id.clone())
        .collect();
    assert_eq!(ids[0], "_one");
}

#[test]
fn book_numbers_parts_and_chapters_with_signifiers() {
    let source = "= Book\n:doctype: book\n:sectnums:\n:partnums:\n:chapter-signifier: Chapter\n\
        :part-signifier: Part\n\n= Part A\n\n== First\n\n=== Sub\n\n== Second\n\n= Part B\n\n== Third\n\n\
        [sect1]\n== Styled\n\n[preface]\n== Pref";
    assert_eq!(
        numbered(source),
        [
            "Book",
            "Part I: Part A",
            "Chapter 1. First",
            "1.1. Sub",
            "Chapter 2. Second",
            "Part II: Part B",
            // Chapters are numbered across parts.
            "Chapter 3. Third",
            // A sect1 style makes a section; its ordinal is within the part.
            "1. Styled",
            // A special section is numbered only with sectnums=all.
            "Pref",
        ]
    );
    // Without partnums parts stay unnumbered; sectnums=all numbers a
    // special section as a chapter.
    let source = "= Book\n:doctype: book\n:sectnums: all\n\n[preface]\n== Pref\n\n= Part\n\n== Chap\n\n[glossary]\n=== Gloss";
    assert_eq!(
        numbered(source),
        ["Book", "1. Pref", "Part", "2. Chap", "2.1. Gloss"]
    );
}

#[test]
fn chapter_number_counter_advances_when_reading_closes_a_chapter() {
    let source = "= Book\n:doctype: book\n:sectnums:\n:chapter-number: 4\n\n== Ch {chapter-number}\n\n\
        In {chapter-number}\n\n== Ch2 {chapter-number}\n\n:sectnums!:\n\n== Ch3\n\n:sectnums:\n\n== Ch4\n\n\
        End {chapter-number}";
    assert_eq!(
        numbered(source),
        ["Book", "5. Ch 4", "6. Ch2 5", "Ch3", "7. Ch4"]
    );
    // Conversion starts again from the header attributes.
    let doc = parse(source).document;
    assert_eq!(text(last_paragraph(&doc.blocks).unwrap()), "End 4");
}

#[test]
fn numbered_alias_and_host_overrides_enable_sectnums() {
    assert_eq!(numbered(":numbered:\n\n== A\n\n=== B"), ["1. A", "1.1. B"]);
    let mut options = ParseOptions::default();
    options.attributes.set("numbered", "");
    let result = parse_with_options(":sectnums!:\n\n== A", &options);
    let mut out = Vec::new();
    for block in result.document.flat_blocks() {
        if let BlockKind::Heading { inlines, .. } = &block.kind {
            out.push(format!(
                "{}{}",
                block.caption.as_deref().unwrap_or(""),
                text(inlines)
            ));
        }
    }
    // The host value is locked under its alias.
    assert_eq!(out, ["1. A"]);
}

#[test]
fn numbered_sections_style_cross_reference_labels() {
    let source =
        ":sectnums:\n:section-refsig: *Sec*\n:xrefstyle: full\n\n== One\n\n=== Sub\n\n== Two\n\n\
        A <<_one>> B xref:_sub[xrefstyle=short] C xref:_two[xrefstyle=basic]";
    assert_eq!(
        labels(source),
        ["*Sec* 1, \u{201c}One\u{201d}", "*Sec* 1.1", "Two"]
    );
    // Without a refsig only the number remains; a book chapter is emphasized.
    let source = "= B\n:doctype: book\n:sectnums:\n:chapter-refsig!:\n\n== Chap\n\n=== Sub\n\n\
        A xref:_chap[xrefstyle=full] B xref:_chap[xrefstyle=short] C xref:_chap[xrefstyle=basic] D xref:_sub[xrefstyle=full]";
    assert_eq!(
        labels(source),
        [
            "1, _Chap_",
            "1",
            "_Chap_",
            "Section 1.1, \u{201c}Sub\u{201d}"
        ]
    );
}

#[test]
fn reading_time_heading_labels_use_section_numbers() {
    let source = ":sectnums:\n\n== One\n\n=== Sub\n\n== See xref:_sub[xrefstyle=short] now\n\nA <<_see_section_1_1_now>>";
    assert_eq!(labels(source), ["See Section 1.1 now"]);
    // Numbering adds no tokens; headings keep their own.
    assert!(tokens(source).iter().any(|t| t.kind == TokenKind::Heading));
}
