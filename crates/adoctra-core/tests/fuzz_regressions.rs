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

fn item_text(source: &str) -> (String, Span) {
    let result = parse(source);
    match &result.document.flat_blocks()[0].kind {
        BlockKind::List { items, .. } => match items[0].inlines.as_slice() {
            [Inline::Text { value, span }] => (value.clone(), *span),
            other => panic!("{source:?}: {other:?}"),
        },
        other => panic!("{source:?}: expected a list, got {other:?}"),
    }
}

#[test]
fn block_metadata_between_an_item_and_its_folded_paragraph_is_not_text() {
    // Ruby's text-only first block reads `[...]` lines as metadata and
    // `fold_first` joins the item text and the paragraph with LF.
    assert_eq!(item_text("* m\n[e]\n'"), ("m '".into(), Span::new(2, 9)));
    assert_eq!(item_text(". m\n[.r]\nx"), ("m x".into(), Span::new(2, 10)));
    assert_eq!(
        item_text("* m\n[e]\n// c\n[f]\ny"),
        ("m y".into(), Span::new(2, 18))
    );
    // A title or attribute entry line is no metadata there: it stays text.
    assert_eq!(item_text("* m\n.T\nx"), ("m .T x".into(), Span::new(2, 8)));
    // Without metadata the fold is unchanged.
    assert_eq!(item_text("* m\nx"), ("m x".into(), Span::new(2, 5)));
}

#[test]
fn a_folded_paragraph_keeps_its_registered_id() {
    // Ruby registers the paragraph's id (with its reftext) before folding it
    // away: the reference resolves, and a later duplicate names a block.
    let source = "* m\n[[id,Ref]]\nx\n\nsee <<id>>\n\n[[id]]\nagain";
    let result = parse(source);
    let blocks = result.document.flat_blocks();
    let BlockKind::Paragraph { inlines } = &blocks[1].kind else {
        panic!("{blocks:?}");
    };
    let Inline::CrossReference { xref, children, .. } = &inlines[1] else {
        panic!("{inlines:?}");
    };
    assert_eq!(xref.target, "#id");
    let label = source.find("Ref").unwrap();
    assert!(
        matches!(children.as_slice(), [Inline::Text { value, span }]
            if value == "Ref" && *span == Span::new(label, label + 3)),
        "{children:?}"
    );
    let duplicate: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| (d.message.as_str(), d.span))
        .collect();
    let again = source.rfind("[[id]]").unwrap() + "[[id]]\n".len();
    assert_eq!(
        duplicate,
        [(
            "id assigned to block already in use: id",
            Span::new(again, source.len())
        )]
    );
}

#[test]
fn revision_lines_follow_ruby_revision_info_backtracking() {
    // ` *` gives back the space before `:` (a remark-only revision line); a
    // line starting with `:` is one only with a comma before.
    let revision = |line: &str| {
        let source = format!("= T\nA\n{line}\n");
        let result = parse(&source);
        result
            .document
            .header
            .clone()
            .and_then(|header| header.revision)
            .map(|r| {
                assert_eq!(r.span, Span::new(6, 6 + line.len()), "{line:?}");
                (r.number, r.date, r.remark)
            })
    };
    let some = |s: &str| Some(s.to_owned());
    assert_eq!(revision(" :"), Some((None, None, some(""))));
    assert_eq!(revision("  :x"), Some((None, None, some("x"))));
    assert_eq!(revision(":"), None);
    assert_eq!(revision(":a,b"), Some((some(""), some("b"), None)));
    assert_eq!(revision(":a, :b"), Some((some(""), None, some("b"))));
    assert_eq!(revision("a,b,c"), Some((some(""), some("c"), None)));
    assert_eq!(revision("x ,: y"), Some((None, some("x"), some("y"))));
    assert_eq!(
        revision("v1.0,:remark"),
        Some((some("1.0"), None, some("remark")))
    );
    assert_eq!(
        revision("1.0, 2020: r :s"),
        Some((some("1.0"), some("2020"), some("r :s")))
    );
    assert_eq!(
        revision("Ж1, д: р"),
        Some((some("1"), some("д"), some("р")))
    );
}

#[test]
fn a_generated_reference_label_restores_passthroughs() {
    // The macros stage writes `[refid]` before passthroughs are restored;
    // Ruby restores them in that label too.
    for (source, label) in [
        ("A<<e +x+>>", "[e x]"),
        ("A<<e pass:[x]>>", "[e x]"),
        ("A<<e +++<>>", "[e +<]"),
    ] {
        let result = parse(source);
        let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
            panic!("{source:?}");
        };
        let Inline::CrossReference { children, span, .. } = &inlines[1] else {
            panic!("{inlines:?}");
        };
        assert_eq!(*span, Span::new(1, source.len()));
        assert!(
            matches!(children.as_slice(), [Inline::Substituted { value, span: s, .. }]
                if value == label && s == span),
            "{source:?}: {children:?}"
        );
    }
}

#[test]
fn a_reference_to_the_document_inside_its_title_converts_the_title_again() {
    // Ruby memoizes the title after the outer conversion, so the reference's
    // label converts the title once more (whose own reference shows `[sec]`).
    let source = ":r: <<sec>>\n[[sec]]\n= r{r}";
    let result = parse(source);
    let BlockKind::Heading { inlines, .. } = &result.document.flat_blocks()[0].kind else {
        panic!();
    };
    let reference = source.rfind("{r}").unwrap();
    assert!(
        matches!(&inlines[1], Inline::AttributeRef { value: Some(value), span, .. }
            if value == "r[sec]" && *span == Span::new(reference, reference + 3)),
        "{inlines:?}"
    );
}

#[test]
fn a_section_title_converted_while_reading_resolves_before_its_id_registers() {
    // Ruby's `initialize_section` converts an explicit-id title with an
    // attribute reference before registering the id: the reference to the
    // section itself is still unknown there. Without `{` it converts later.
    let label = |source: &str| {
        let result = parse(source);
        let blocks = result.document.flat_blocks();
        let BlockKind::Heading { inlines, .. } = &blocks.last().unwrap().kind else {
            panic!("{blocks:?}")
        };
        let text = |nodes: &[Inline]| -> String {
            nodes
                .iter()
                .map(|node| match node {
                    Inline::Text { value, .. } | Inline::Substituted { value, .. } => value.clone(),
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        inlines
            .iter()
            .find_map(|inline| match inline {
                Inline::AttributeRef { value, .. } => value.clone(),
                Inline::CrossReference { children, .. } => Some(text(children)),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(label(":r: <<sec>>\n[[sec]]\n== r{r}"), "[sec]");
    assert_eq!(label("[[o]]\n== O\n\n:r: <<o>>\n[[sec]]\n== r{r}"), "O");
    assert_eq!(label("[[sec]]\n== r <<sec>>"), "r [sec]");
}

#[test]
fn an_ignored_style_keeps_the_literal_paragraph_tabs() {
    // Ruby's indented paragraph runs `adjust_indentation!` without a tab
    // size; only `[literal]` builds the block with `tabsize`.
    let text = |source: &str| match &parse(source).document.flat_blocks()[0].kind {
        BlockKind::Literal { text, .. } => text.clone(),
        other => panic!("{source:?}: {other:?}"),
    };
    assert_eq!(text("[v,tabsize=2]\n\tx\ty"), "x\ty");
    assert_eq!(text("[v[,tabsize=2]\n\tx\ty"), "x\ty");
    assert_eq!(text("[literal,tabsize=2]\n\tx\ty"), "  x y");
}
