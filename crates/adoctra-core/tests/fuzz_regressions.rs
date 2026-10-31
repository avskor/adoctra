//! Differences from the pinned Asciidoctor found by `scripts/fuzz.py`,
//! minimized; each asserts Ruby's behaviour (checked against 2.0.23).

use adoctra_core::{parse, BlockKind, Inline, Severity, Span};

fn literal_text(source: &str) -> (String, Span) {
    let result = parse(source);
    let blocks = result.document.flat_blocks();
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    match &blocks[0].kind {
        BlockKind::Literal { text, .. } => (text.clone(), blocks[0].span),
        other => panic!("expected a literal paragraph, got {other:?}"),
    }
}

#[test]
fn indented_heading_and_anchor_lines_are_literal_paragraphs() {
    // Ruby's AtxSectionTitleRx and BlockAnchorRx are anchored at the line
    // start: an indented line starts a literal paragraph instead.
    assert_eq!(literal_text(" = t"), ("= t".into(), Span::new(0, 4)));
    assert_eq!(literal_text("\t== S"), ("== S".into(), Span::new(0, 5)));
    assert_eq!(literal_text(" [[n,]]"), ("[[n,]]".into(), Span::new(0, 7)));
    let result = parse("= T\n\n == Sub");
    assert!(result.document.header.is_some());
    // The document title stays the first block.
    assert!(matches!(
        result.document.flat_blocks().last().unwrap().kind,
        BlockKind::Literal { .. }
    ));
    // A trailing blank still ends an anchor line.
    let result = parse("[[a]] \npara");
    assert_eq!(result.document.flat_blocks()[0].id.as_deref(), Some("a"));
}

#[test]
fn unicode_spaces_are_not_blank_like_ruby_strip() {
    // Ruby's `rstrip` removes ASCII whitespace and NUL only: a line of
    // no-break spaces is a paragraph, and joins its neighbours.
    let result = parse("\u{a0}");
    let blocks = result.document.flat_blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].span, Span::new(0, 2));
    match &blocks[0].kind {
        BlockKind::Paragraph { inlines } => {
            assert!(matches!(&inlines[0], Inline::Text { value, .. } if value == "\u{a0}"))
        }
        other => panic!("expected a paragraph, got {other:?}"),
    }
    let result = parse("a\n\u{3000}\nb");
    let blocks = result.document.flat_blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].span, Span::new(0, 7));
    // A role keeps an inner no-break space (Ruby's `split` is ASCII).
    let result = parse("[.r\u{a0}s]\npara");
    assert_eq!(result.document.flat_blocks()[0].roles, ["r\u{a0}s"]);
}

#[test]
fn unterminated_blocks_name_ruby_contexts() {
    // Ruby's message names the context after the style masquerade.
    for (source, context) in [
        ("++++\nx", "pass"),
        ("[stem]\n++++\nx", "stem"),
        ("[latexmath]\n++++\nx", "stem"),
        ("[NOTE]\n====\nx", "admonition"),
        ("[source]\n--\nx", "listing"),
        ("[literal]\n--\nx", "literal"),
        ("[quote]\n--\nx", "quote"),
        ("[verse]\n--\nx", "verse"),
        ("[sidebar]\n--\nx", "sidebar"),
        ("[example]\n--\nx", "example"),
        ("[pass]\n--\nx", "pass"),
        ("[CAUTION]\n--\nx", "admonition"),
        ("[abstract]\n--\nx", "open"),
        ("====\nx", "example"),
    ] {
        let result = parse(source);
        let messages: Vec<_> = result
            .diagnostics
            .iter()
            .filter_map(|d| d.report.message.clone())
            .collect();
        assert_eq!(
            messages,
            [format!("unterminated {context} block")],
            "{source:?}"
        );
    }
}

#[test]
fn excluded_partintro_registers_no_footnotes() {
    // Ruby never converts an excluded block, so its footnotes do not count.
    let result = parse("[partintro]\n--\nfootnote:[e]\n--\n\nx footnote:[g]");
    assert_eq!(result.document.footnotes.len(), 1);
    assert_eq!(result.document.footnotes[0].index, "1");
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message.starts_with("partintro block can only be used")));
}

#[test]
fn section_level_errors_come_in_ruby_reporting_order() {
    // Ruby reports a section's level error when it reads the title, before
    // the unterminated listing that follows it.
    let result = parse("r\n\nitle\n===\n----");
    let order: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.report.severity.unwrap_or(d.severity),
                d.report.message.as_deref().unwrap_or(&d.message).to_owned(),
            )
        })
        .collect();
    assert_eq!(
        order,
        [
            (
                Severity::Error,
                "level 0 sections can only be used when doctype is book".to_owned()
            ),
            (Severity::Warning, "unterminated listing block".to_owned()),
        ]
    );
    // An invalid part is reported when reading closes it: before the next
    // part's own diagnostics.
    let result = parse("= D\n:doctype: book\n\n= P1\n\nx\n\n= P2\n\n----");
    let order: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| d.report.message.as_deref().unwrap_or(&d.message).to_owned())
        .collect();
    assert_eq!(
        order,
        [
            "invalid part, must have at least one section (e.g., chapter, appendix, etc.)",
            "unterminated listing block",
            "invalid part, must have at least one section (e.g., chapter, appendix, etc.)",
        ]
    );
}

#[test]
fn fence_like_line_with_a_multibyte_character_does_not_panic() {
    // `lists::delimiter` sliced four characters at a byte index.
    for source in ["``\u{43e}}", "``\u{43e}", "--\u{1f642}-", "==\u{e9}="] {
        let result = parse(source);
        assert!(result.diagnostics.is_empty(), "{source:?}");
        let blocks = result.document.flat_blocks();
        assert!(
            matches!(blocks[0].kind, BlockKind::Paragraph { .. }),
            "{source:?}: {blocks:?}"
        );
        assert_eq!(blocks[0].span, Span::new(0, source.len()));
    }
}

#[test]
fn manpage_doctype_at_end_of_input_does_not_panic() {
    // `manpage::read` looked for block metadata past the last line.
    for source in [
        ":doctype: manpage",
        ":doctype: manpage\n",
        "= t(1)\n:doctype: manpage",
    ] {
        let result = parse(source);
        let messages: Vec<_> = result
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect();
        assert!(
            messages.contains(&"name section expected"),
            "{source:?}: {messages:?}"
        );
    }
}
