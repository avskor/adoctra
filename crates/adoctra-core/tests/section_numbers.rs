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

/// The section tree as `(kind level, heading source)` lines.
fn outline(blocks: &[Block], source: &str, depth: usize, out: &mut Vec<String>) {
    for block in blocks {
        let pad = "  ".repeat(depth);
        match &block.kind {
            BlockKind::Section { level, blocks } => {
                out.push(format!("{pad}section {level}"));
                outline(blocks, source, depth + 1, out);
            }
            BlockKind::Heading { level, .. } => out.push(format!(
                "{pad}h{level} {}",
                &source[block.span.start..block.span.end]
            )),
            _ => {}
        }
    }
}

#[test]
fn special_sections_take_level_one_and_nest_by_their_title_line() {
    let source = "= Doc\n\n[appendix]\n== Extra\n\n=== Inside\n\n[appendix]\n= Zero\n\n== After\n\n[glossary]\n== Gloss";
    let result = parse(source);
    let mut out = Vec::new();
    outline(&result.document.blocks, source, 0, &mut out);
    assert_eq!(
        out,
        [
            "h0 = Doc",
            "section 1",
            "  h1 == Extra",
            "  section 2",
            "    h2 === Inside",
            // A level-0 appendix is a level-1 section; the next level-1 title
            // is its sibling.
            "section 1",
            "  h1 = Zero",
            "section 1",
            "  h1 == After",
            "section 1",
            "  h1 == Gloss",
        ]
    );
    // The title line keeps its level for diagnostics.
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| (d.message.as_str(), &source[d.span.start..d.span.end]))
        .collect();
    assert_eq!(
        errors,
        [(
            "level 0 sections can only be used when doctype is book",
            "= Zero"
        )]
    );
    assert_eq!(
        numbered(source),
        [
            "Doc",
            "Appendix A: Extra",
            "Inside",
            "Appendix B: Zero",
            "After",
            "Gloss"
        ]
    );
}

#[test]
fn appendix_captions_and_signifiers_are_localizable() {
    let source = "= Книга\n:doctype: book\n:sectnums:\n:appendix-caption: Приложение\n\
        :appendix-refsig: Прил.\n:chapter-refsig: Глава\n:section-refsig: Раздел\n:xrefstyle: full\n\n\
        = Часть\n\n== Глава\n\n[appendix]\n== Первое\n\n=== Внутри\n\n[appendix]\n= Второе\n\n\
        :appendix-caption!:\n\n[appendix]\n== Третье\n\n[bibliography]\n== Литература\n\n\
        См. <<_первое>> xref:_второе[xrefstyle=short] <<_внутри>> <<_глава>> <<_третье>>";
    assert_eq!(
        numbered(source),
        [
            "Книга",
            "Часть",
            "1. Глава",
            "Приложение A: Первое",
            "A.1. Внутри",
            // The caption is fixed when reading closes the appendix, after
            // the unset that follows its title.
            "B. Второе",
            "C. Третье",
            "Литература",
        ]
    );
    assert_eq!(
        labels(source),
        [
            "Прил. A, _Первое_",
            "Прил. B",
            "Раздел A.1, \u{201c}Внутри\u{201d}",
            "Глава 1, _Глава_",
            "Прил. C, _Третье_",
        ]
    );
}

#[test]
fn special_section_nesting_and_numbering_follow_ruby() {
    let source =
        "= B\n:doctype: book\n:sectnums:\n\n[preface]\n= Pref\n\n[abstract]\n=== Abs\n\n== Ch\n\n\
        [glossary]\n== G\n\n=== GS";
    let result = parse(source);
    // An abstract inside a preface is special by inheritance: unnumbered and
    // without a chapter number.
    assert_eq!(numbered(source), ["B", "Pref", "Abs", "1. Ch", "G", "GS"]);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| (d.message.as_str(), &source[d.span.start..d.span.end]))
        .collect();
    assert_eq!(
        errors,
        [("glossary sections do not support nested sections", "=== GS")]
    );
    let doc = result.document;
    let levels: Vec<_> = doc
        .flat_blocks()
        .iter()
        .filter_map(|b| match b.kind {
            BlockKind::Heading { level, .. } => Some(level),
            _ => None,
        })
        .collect();
    assert_eq!(levels, [0, 1, 1, 1, 1, 2]);
}

#[test]
fn appendix_counter_and_reading_labels_see_sections_closed_so_far() {
    let source = ":appendix-number: C\n:sectnums:\n\n[appendix]\n== First {appendix-number}\n\n\
        === See xref:_first_c[xrefstyle=short]\n\n[appendix]\n== Second {appendix-number}\n\n\
        [appendix]\n[caption=\"Custom: \"]\n== Third\n\n== One\n\n=== In xref:_one[xrefstyle=full]\n\n\
        Body {appendix-number} <<_see_appendix>> <<_in_section_one>>";
    assert_eq!(
        numbered(source),
        [
            "Appendix D: First C",
            // Reading has not closed the appendix yet: no letter.
            "D.1. See Appendix ",
            "Appendix E: Second D",
            // A block caption attribute does not change a section caption.
            "Appendix F: Third",
            "1. One",
            "1.1. In Section , \u{201c}One\u{201d}",
        ]
    );
    let doc = parse(source).document;
    // Conversion starts from the header value.
    assert!(text(last_paragraph(&doc.blocks).unwrap()).starts_with("Body C "));
    assert_eq!(
        labels(source),
        ["See Appendix ", "In Section , \u{201c}One\u{201d}"]
    );
}
