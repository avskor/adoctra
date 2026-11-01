//! Differences from the pinned Asciidoctor found by `scripts/fuzz.py`,
//! minimized; each asserts Ruby's behaviour (checked against 2.0.23).

use adoctra_core::{parse, Block, BlockKind, Inline, Severity, Span};

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

#[test]
fn a_folded_paragraph_loses_its_common_indentation_before_hard_breaks() {
    // Ruby's `adjust_indentation!` strips the common indentation from every
    // line of the paragraph folded into an item, its first included: ` +`
    // alone becomes `+`, which is no hard line break.
    let breaks = |source: &str| {
        let result = parse(source);
        let BlockKind::List { items, .. } = &result.document.flat_blocks()[0].kind else {
            panic!("{source:?}");
        };
        format!("{:?}", items[0].inlines).contains("LineBreak")
    };
    assert!(!breaks("* a\n +"));
    assert!(!breaks("* a\n  +"));
    assert!(breaks("* a\n  b +\n   +"));
    // A less indented line keeps the others' indentation.
    assert!(breaks("* a\nb\n +"));
    assert!(breaks("* a\n +\nb"));
}

#[test]
fn an_unknown_reference_label_keeps_generated_markup() {
    // Ruby writes `[refid]` verbatim: tags the quotes stage generated stay
    // markup, protected source characters stay text.
    let label = |source: &str| {
        let result = parse(source);
        let blocks = result.document.flat_blocks();
        let inlines = match &blocks.last().unwrap().kind {
            BlockKind::Paragraph { inlines } | BlockKind::Heading { inlines, .. } => {
                inlines.clone()
            }
            other => panic!("{other:?}"),
        };
        let Some(Inline::CrossReference { children, span, .. }) = inlines
            .into_iter()
            .find(|inline| matches!(inline, Inline::CrossReference { .. }))
        else {
            panic!("{source:?}");
        };
        assert_eq!(span.start, source.find("<<").unwrap(), "{source:?}");
        children
            .iter()
            .map(|child| match child {
                Inline::Raw { value, .. } => format!("R({value})"),
                Inline::Text { value, .. } => value.clone(),
                other => panic!("{other:?}"),
            })
            .collect::<String>()
    };
    assert_eq!(label("D<<y *e*>>"), "[y R(<strong>)eR(</strong>)]");
    // Ruby's macro ends at the first `>>`, inside the protected `&gt;`.
    assert_eq!(label("D<<y &lt;b&gt;>>"), "[y <b]");
    // A section title converted while reading labels it the same way.
    assert_eq!(label("== S <<y *e*>>"), "[y R(<strong>)eR(</strong>)]");
}

#[test]
fn author_and_revision_values_take_ruby_header_substitutions() {
    // `parse_header_metadata` applies the header subs to every implicit
    // value in `process_authors` order: a counter advances for each value
    // naming it, with the document's attribute-missing policy.
    let source = "= T {counter:n}\nA {counter:n} B <x@{counter:n}>\nv{counter:r}, {counter:r}: x\n";
    let result = parse(source);
    let attrs = &result.document.attributes;
    let get = |key: &str| attrs.get(key).map(str::to_owned);
    assert_eq!(get("firstname").as_deref(), Some("A 2 B <x@3>"));
    assert_eq!(get("author").as_deref(), Some("A 4 B <x@5>"));
    assert_eq!(get("authors").as_deref(), Some("A 6 B <x@7>"));
    assert_eq!(get("n").as_deref(), Some("7"));
    // RevisionInfoLineRx splits inside the expression, as in Ruby.
    assert_eq!(get("revnumber").as_deref(), Some("1"));
    assert_eq!(get("revdate").as_deref(), Some("{counter"));
    assert_eq!(get("revremark").as_deref(), Some("r}: x"));
    // A second author adds the `_1` copies; the author attributes are then
    // derived again from the substituted `author` (values from Ruby).
    let result = parse("= T\nA {counter:m}; B\n");
    let attrs = &result.document.attributes;
    for (key, value) in [
        ("m", "5"),
        ("lastname", "4"),
        ("author", "A 4"),
        ("authorinitials", "A4"),
        ("authors", "A 4, B"),
        ("author_1", "A 4"),
        ("author_2", "B"),
    ] {
        assert_eq!(attrs.get(key), Some(value), "{key}");
    }
    // Missing references warn at the author line under `warn`.
    let source = "= T\n:attribute-missing: warn\nA {miss}\n";
    let result = parse(source);
    let line = source.find("A {miss}").unwrap();
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| (d.message.as_str(), d.span))
        .collect();
    let at = Span::new(line, line + "A {miss}".len());
    assert_eq!(
        warnings,
        [
            ("skipping reference to missing attribute: miss", at),
            ("skipping reference to missing attribute: miss", at),
            ("skipping reference to missing attribute: miss", at),
        ]
    );
}

#[test]
fn an_undisplayed_document_title_converts_at_its_first_use() {
    // Embedded output without the title (`notitle`) does not convert it;
    // a reference to the document would, and the tree still gets its nodes.
    let embedded = adoctra_core::ParseOptions {
        embedded: true,
        ..Default::default()
    };
    let counter = |source: &str| {
        let result = adoctra_core::parse_with_options(source, &embedded);
        let title = match &result.document.flat_blocks()[0].kind {
            BlockKind::Heading { inlines, .. } => format!("{inlines:?}"),
            other => panic!("{other:?}"),
        };
        let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[1].kind else {
            panic!();
        };
        (format!("{inlines:?}"), title)
    };
    let (body, title) = counter("= T {counter:n}\n\n{n}");
    assert!(body.contains("value: Some(\"1\")"), "{body}");
    assert!(title.contains("\"2\""), "{title}");
    let (body, _) = counter("[[top]]\n= T {counter:n}\n\n{n} <<top>> {n}");
    assert!(
        body.contains("value: Some(\"1\")") && body.contains("value: Some(\"2\")"),
        "{body}"
    );
}

#[test]
fn a_folded_paragraph_entries_are_not_played_back() {
    // Ruby applies an entry above a description's adjacent paragraph while
    // reading; the paragraph folds away, so conversion never replays it.
    // A later block's own entry still is.
    let source = "m;; f\n:x: v\ny\n+\n:z: w\nz\n\n{x}{z}";
    let result = parse(source);
    let blocks = result.document.flat_blocks();
    let BlockKind::Paragraph { inlines } = &blocks.last().unwrap().kind else {
        panic!("{blocks:?}");
    };
    let values: Vec<_> = inlines
        .iter()
        .map(|inline| match inline {
            Inline::AttributeRef {
                name, value, span, ..
            } => (name.as_str(), value.clone(), *span),
            other => panic!("{other:?}"),
        })
        .collect();
    let at = source.rfind("{x}").unwrap();
    assert_eq!(
        values,
        [
            ("x", None, Span::new(at, at + 3)),
            ("z", Some("w".to_owned()), Span::new(at + 3, at + 6)),
        ]
    );
}

#[test]
fn a_body_cell_substitutes_its_text_whole_then_splits_paragraphs() {
    // Ruby's `Cell#content`: substitutions run on the whole cell text, then
    // the result splits at `\n{2,}`; a head cell keeps `Cell#text`.
    let paragraphs = |source: &str| -> Vec<String> {
        let result = parse(source);
        let BlockKind::Table(table) = &result.document.flat_blocks()[0].kind else {
            panic!("{source:?}");
        };
        let cell = table.cells().next().unwrap();
        let pieces: Vec<String> = match &cell.content {
            adoctra_core::ast::CellContent::Paragraphs(paragraphs) => paragraphs
                .iter()
                .map(|nodes| format!("{nodes:?}"))
                .collect(),
            other => panic!("{other:?}"),
        };
        pieces
    };
    // The em dash replacement takes the blank line's second LF: one paragraph.
    assert_eq!(paragraphs("|===\n|\"\n\n-- \n|===").len(), 1);
    // A passthrough spanning the blank line converts to nothing.
    assert!(paragraphs("|===\n|pass:a[\n\n]\n|===").is_empty());
    // Plain text splits, keeping each piece's source range.
    let source = "|===\n|ab\n\ncd\n|===";
    let pieces = paragraphs(source);
    let start = source.find("ab").unwrap();
    assert_eq!(pieces.len(), 2);
    assert!(
        pieces[0].contains(&format!("start: {start}, end: {}", start + 2)),
        "{pieces:?}"
    );
    assert!(
        pieces[1].contains(&format!("start: {}, end: {}", start + 4, start + 6)),
        "{pieces:?}"
    );
}

/// The debug form of a document's blocks, for compact shape checks.
fn tree(source: &str) -> String {
    format!("{:?}", parse(source).document.flat_blocks())
}

#[test]
fn an_unknown_paragraph_style_still_ends_a_list_item_paragraph_at_a_list() {
    // Ruby resets a style it does not know before `read_paragraph_lines`.
    let result = parse("* m\n[e]\n#\n. n");
    let BlockKind::List { items, .. } = &result.document.flat_blocks()[0].kind else {
        panic!();
    };
    assert!(
        matches!(
            items[0].blocks.as_slice(),
            [Block {
                kind: BlockKind::List { ordered: true, .. },
                ..
            }]
        ),
        "{:?}",
        items[0]
    );
}

#[test]
fn a_comment_style_peek_leaves_the_next_line_to_preprocessing() {
    // Ruby's direct peek does not consume the line: the list reads it
    // preprocessed, so the malformed directive errs.
    let source = "[comment]\n. m\nifdef::[]";
    let result = parse(source);
    let at = source.find("ifdef::[]").unwrap();
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|d| (d.message.as_str(), d.span))
            .collect::<Vec<_>>(),
        [(
            "malformed ifdef directive: missing target",
            Span::new(at, at + 9)
        )]
    );
    assert!(!tree(source).contains("ifdef"), "{}", tree(source));
}

#[test]
fn a_verse_drops_blank_edge_lines_after_substitutions() {
    let verse = |source: &str| match &parse(source).document.flat_blocks()[0].kind {
        BlockKind::Verse { inlines, .. } => format!("{inlines:?}"),
        other => panic!("{other:?}"),
    };
    let source = "[verse]\n____\n\n]\n____";
    let at = source.rfind("]\n____").unwrap();
    assert!(
        verse(source).contains(&format!(
            "value: \"]\", span: Span {{ source_id: SourceId(0), start: {at}, end: {},",
            at + 1
        )),
        "{}",
        verse(source)
    );
    // Lines an attribute leaves blank go too.
    assert!(!verse("[verse]\n____\n{empty}\na\n____").contains("\\n"));
}

#[test]
fn mailto_fields_encode_their_lexical_spelling() {
    // Ruby percent-encodes the macros stage spelling: a replacement is its
    // character reference there.
    assert!(tree("mailto:a@b[,b't]").contains("target: \"mailto:a@b?subject=b%26%238217%3Bt\""));
    assert!(tree("mailto:a@b[,a&b]").contains("target: \"mailto:a@b?subject=a%26amp%3Bb\""));
}

#[test]
fn a_hash_inside_a_replacement_reference_is_no_document_separator() {
    // Ruby tests the first `#` of `i&#8217;c#d`, which belongs to the
    // character reference: the whole refid names an id here.
    let result = parse("A<<i'c#d>>");
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!();
    };
    let Inline::CrossReference { xref, .. } = &inlines[1] else {
        panic!("{inlines:?}");
    };
    assert_eq!(xref.path, None);
    assert_eq!(xref.target, "#i\u{2019}c#d");
}

#[test]
fn a_reference_a_body_attribute_converts_takes_the_output_suffix() {
    // Ruby converts `pass:m` while defining the entry, with the current
    // `outfilesuffix` (set after the header, unset within it).
    assert!(tree("== e\n:o: pass:c,m[<<c#>>]\nA{o}").contains("target: \"c.html\""));
    assert!(tree(":o: pass:c,m[<<c#>>]\n\nA{o}").contains("target: \"c\""));
}

#[test]
fn reading_conversions_use_the_header_compat_mode() {
    // Ruby's `compat_mode` changes during conversion only (playback), so a
    // section title converted while reading keeps the header's mode.
    let source = "== e\n:compat-mode:\n\n== +*+ `y`";
    let shown = tree(source);
    assert!(
        !shown.contains("Monospace { children: [Text { value: \"*\""),
        "{shown}"
    );
    assert!(tree("== e\n:compat-mode:\n= xref:o[e=]").contains("\"[o]\""));
}

#[test]
fn a_manual_page_name_section_has_no_numeral() {
    // Ruby's header NAME section is numbered (`sectnums`) without a numeral.
    let source = "= t(1)\n:doctype: manpage\n:sectnums:\n:xrefstyle: full\n\n== NAME\n\nt - x\n\n== D\n\n<<_name>>";
    let result = parse(source);
    let blocks = result.document.flat_blocks();
    let BlockKind::Paragraph { inlines } = &blocks.last().unwrap().kind else {
        panic!();
    };
    let Inline::CrossReference { children, .. } = &inlines[0] else {
        panic!("{inlines:?}");
    };
    let label: String = children
        .iter()
        .map(|child| match child {
            Inline::Text { value, .. } => value.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(label, "Section , \u{201c}NAME\u{201d}");
}

#[test]
fn illegal_part_content_is_reported_where_reading_completes_the_block() {
    let result = parse("= B\n:doctype: book\n\n= P\n--\n--\n.");
    let messages: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "illegal block content outside of partintro block",
            "invalid part, must have at least one section (e.g., chapter, appendix, etc.)",
        ]
    );
}

#[test]
fn an_escaped_quote_starting_a_single_quoted_value_skips_its_substitutions() {
    // Ruby's AttributeList tests the unescaped value: `\'` makes it start
    // with an apostrophe, so it is not substituted again.
    let source = ":attribute-missing: warn\n\nlink:s['\\'{g}'=]";
    let warnings = parse(source)
        .diagnostics
        .iter()
        .filter(|d| d.message.contains("missing attribute"))
        .count();
    assert_eq!(warnings, 1);
}
