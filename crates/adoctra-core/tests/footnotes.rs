//! Footnotes: Ruby's `InlineFootnoteMacroRx` (the last macro), numbering with
//! `footnote-number` when converted, the document's footnotes list and the
//! nested lists of AsciiDoc table cells.
use adoctra_core::ast::{CellContent, FootnoteDefinition};
use adoctra_core::{
    parse, tokens, AttributePart, Block, BlockKind, Footnote, FootnoteKind, Inline, Severity,
    TokenKind,
};

fn paragraph(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines } => inlines,
        other => panic!("not a paragraph: {other:?}"),
    }
}

fn footnotes(inlines: &[Inline]) -> Vec<(&Footnote, &str)> {
    inlines
        .iter()
        .filter_map(|inline| match inline {
            Inline::Footnote { footnote, .. } => Some((footnote.as_ref(), "")),
            _ => None,
        })
        .collect()
}

fn content(definition: &FootnoteDefinition) -> String {
    adoctra_core::attributes::parts_text(&definition.content)
}

#[test]
fn definitions_references_and_unresolved_ids() {
    let source =
        "A footnote:[note *b*] and footnote:x[named] then footnote:x[] and footnote:y[].\n";
    let result = parse(source);
    let messages: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.severity,
                d.message.as_str(),
                &source[d.span.start..d.span.end],
            )
        })
        .collect();
    assert_eq!(
        messages,
        [(
            Severity::Warning,
            "invalid footnote reference: y",
            "footnote:y[]"
        )]
    );
    let blocks = result.document.into_flat_blocks();
    let inlines = paragraph(&blocks[0]);
    let found: Vec<_> = inlines
        .iter()
        .filter_map(|inline| match inline {
            Inline::Footnote { footnote, span } => Some((
                footnote.kind,
                footnote.index.clone(),
                footnote.id.clone(),
                &source[span.start..span.end],
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                FootnoteKind::Definition,
                Some("1".into()),
                None,
                "footnote:[note *b*]"
            ),
            (
                FootnoteKind::Definition,
                Some("2".into()),
                Some("x".into()),
                "footnote:x[named]"
            ),
            (
                FootnoteKind::Reference,
                Some("2".into()),
                None,
                "footnote:x[]"
            ),
            (
                FootnoteKind::Unresolved,
                None,
                Some("y".into()),
                "footnote:y[]"
            ),
        ]
    );
    // The definition's text keeps the earlier substitutions as parts.
    let (first, _) = footnotes(inlines)[0];
    assert_eq!(
        first.content,
        [
            AttributePart::Text("note ".into()),
            AttributePart::Open(adoctra_core::AttributeFormat::Strong),
            AttributePart::Text("b".into()),
            AttributePart::Close(adoctra_core::AttributeFormat::Strong),
        ]
    );
}

#[test]
fn the_document_lists_footnotes_in_registration_order_with_spans() {
    let source = "== Sec footnote:[In title]\n\nBody footnote:[body\nwrapped \\] text].\n";
    let document = parse(source).document;
    let list: Vec<_> = document
        .footnotes
        .iter()
        .map(|f| {
            (
                f.index.as_str(),
                content(f),
                &source[f.span.start..f.span.end],
            )
        })
        .collect();
    // Like Ruby, the section title registers while reading and conversion
    // restarts the counter from the header attributes.
    assert_eq!(
        list,
        [
            ("1", "In title".into(), "footnote:[In title]"),
            (
                "1",
                "body wrapped ] text".into(),
                "footnote:[body\nwrapped \\] text]"
            ),
        ]
    );
}

#[test]
fn escaped_empty_and_legacy_macros_follow_ruby() {
    let source =
        "\\footnote:[esc] footnote:[] footnoteref:[r,legacy] footnoteref:[r] footnoteref:[]\n";
    let result = parse(source);
    let messages: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "found deprecated footnoteref macro: footnoteref:[r,legacy]; use footnote macro with target instead",
            "found deprecated footnoteref macro: footnoteref:[r]; use footnote macro with target instead",
        ]
    );
    assert_eq!(result.document.footnotes.len(), 1);
    assert_eq!(result.document.footnotes[0].id.as_deref(), Some("r"));
    let blocks = result.document.into_flat_blocks();
    let inlines = paragraph(&blocks[0]);
    let text: String = inlines
        .iter()
        .map(|i| match i {
            Inline::Text { value, .. } => value.clone(),
            Inline::Footnote { footnote, .. } => footnote.marker(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(text, "footnote:[esc] footnote:[] [1] [1] footnoteref:[]");
    // compat-mode silences the deprecation.
    assert!(parse(":compat-mode:\n\nfootnoteref:[r,x]\n")
        .diagnostics
        .is_empty());
}

#[test]
fn asciidoc_cells_keep_their_own_footnotes_with_the_root_counter() {
    let source = "Before footnote:x[outer].\n\n|===\n|cell footnote:[in cell] a|nested footnote:[in nested] footnote:x[]\n|===\n";
    let result = parse(source);
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(
        result.diagnostics[0].message,
        "invalid footnote reference: x"
    );
    let document = result.document;
    let outer: Vec<_> = document
        .footnotes
        .iter()
        .map(|f| (f.index.as_str(), content(f)))
        .collect();
    assert_eq!(outer, [("1", "outer".into()), ("2", "in cell".into())]);
    let blocks = document.into_flat_blocks();
    let BlockKind::Table(table) = &blocks[1].kind else {
        panic!()
    };
    let CellContent::Blocks { footnotes, .. } = &table.body[0][1].content else {
        panic!()
    };
    let nested: Vec<_> = footnotes
        .iter()
        .map(|f| (f.index.as_str(), content(f)))
        .collect();
    assert_eq!(nested, [("3", "in nested".into())]);
}

#[test]
fn attribute_owned_footnotes_register_once_at_the_definition() {
    let document =
        parse(":fn: pass:m[footnote:[from attr]]\n\nA {fn} and {fn} then footnote:[body].\n")
            .document;
    let list: Vec<_> = document
        .footnotes
        .iter()
        .map(|f| (f.index.as_str(), content(f)))
        .collect();
    assert_eq!(list, [("1", "from attr".into()), ("2", "body".into())]);
    let blocks = document.into_flat_blocks();
    let parts = paragraph(&blocks[0])
        .iter()
        .find_map(|inline| match inline {
            Inline::AttributeRef { parts, .. } => Some(parts),
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(parts.as_slice(), [AttributePart::Footnote(f)] if f.index.as_deref() == Some("1"))
    );
}

#[test]
fn borrowed_xref_labels_drop_the_footnote_link() {
    let document = parse("== Intro footnote:[note]\n\nSee <<_intro_1>>.\n").document;
    let blocks = document.into_flat_blocks();
    let Inline::CrossReference { children, .. } = &paragraph(&blocks[1])[1] else {
        panic!()
    };
    let Inline::Substituted { parts, .. } = &children[1] else {
        panic!("{children:?}")
    };
    assert_eq!(
        parts[0],
        AttributePart::OpenWithAttributes {
            format: adoctra_core::AttributeFormat::Superscript,
            id: None,
            role: Some("footnote".into()),
        }
    );
}

#[test]
fn footnote_macros_are_highlighted_as_inline_macros() {
    let source = "A footnote:[x *b*] z\n";
    let tokens = tokens(source);
    assert!(tokens.iter().any(|t| t.kind == TokenKind::InlineMacro
        && &source[t.span.start..t.span.end] == "footnote:[x *b*]"));
}

#[test]
fn unused_and_removed_attribute_definitions_keep_catalog_order_and_origins() {
    for newline in ["\n", "\r\n"] {
        let source =
            ":note: pass:q,m[footnote:n[Stored *Ж* note]]\n\n:note!:\n\nBody footnote:[Next].\n"
                .replace('\n', newline);
        let result = parse(&source);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let catalog = &result.document.footnotes;
        assert_eq!(catalog.len(), 2);
        assert_eq!(
            (
                catalog[0].index.as_str(),
                catalog[0].id.as_deref(),
                content(&catalog[0])
            ),
            ("1", Some("n"), "Stored Ж note".into())
        );
        assert!(catalog[0]
            .content
            .contains(&AttributePart::Open(adoctra_core::AttributeFormat::Strong)));
        assert_eq!(
            &source[catalog[0].span.start..catalog[0].span.end],
            ":note: pass:q,m[footnote:n[Stored *Ж* note]]"
        );
        assert_eq!(
            (catalog[1].index.as_str(), content(&catalog[1])),
            ("2", "Next".into())
        );
        assert_eq!(
            &source[catalog[1].span.start..catalog[1].span.end],
            "footnote:[Next]"
        );
        assert!(result.document.final_attributes.get("note").is_none());
    }
}

#[test]
fn unused_nested_attribute_footnotes_stay_in_the_cell_catalog() {
    let source = "Outer footnote:o[Outer].\n\n[cols=a,options=noheader]\n|===\n|:note: pass:m[footnote:i[Inner]]\n\nCell.\n|===\n\nAfter footnote:[Last].\n";
    let document = parse(source).document;
    assert_eq!(
        document
            .footnotes
            .iter()
            .map(|f| (f.index.as_str(), content(f)))
            .collect::<Vec<_>>(),
        [("1", "Outer".into()), ("2", "Last".into())]
    );
    let BlockKind::Table(table) = &document.blocks[1].kind else {
        panic!()
    };
    let CellContent::Blocks { footnotes, .. } = &table.body[0][0].content else {
        panic!()
    };
    assert_eq!(footnotes.len(), 1);
    assert_eq!(
        (footnotes[0].index.as_str(), content(&footnotes[0])),
        ("1", "Inner".into())
    );
    assert_eq!(
        &source[footnotes[0].span.start..footnotes[0].span.end],
        ":note: pass:m[footnote:i[Inner]]"
    );
}

#[test]
fn unused_footnote_content_restores_passes_and_definition_time_xrefs() {
    let source =
        ":note: pass:m[footnote:n[See xref:s[] and pass:q[*Ж*]]]\n\n[[s]]\n== Target\n\nBody.\n";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    let catalog = &result.document.footnotes;
    assert_eq!(catalog.len(), 1);
    assert_eq!(content(&catalog[0]), "See [s] and Ж");
    assert!(catalog[0]
        .content
        .contains(&AttributePart::Open(adoctra_core::AttributeFormat::Strong)));
    assert_eq!(
        &source[catalog[0].span.start..catalog[0].span.end],
        ":note: pass:m[footnote:n[See xref:s[] and pass:q[*Ж*]]]"
    );
}

#[test]
fn lazy_outer_title_footnotes_do_not_leak_into_the_converting_cell() {
    let source = "|===\na|<<s>> footnote:[one]\n|===\n\n[#s]\n== T footnote:[two]";
    let options = adoctra_core::ParseOptions {
        embedded: true,
        ..Default::default()
    };
    let document = adoctra_core::parse_with_options(source, &options).document;
    assert_eq!(document.footnotes.len(), 1);
    assert_eq!(content(&document.footnotes[0]), "two");
    assert_eq!(
        &source[document.footnotes[0].span.start..document.footnotes[0].span.end],
        "footnote:[two]"
    );
    let BlockKind::Table(table) = &document.blocks[0].kind else {
        panic!()
    };
    let CellContent::Blocks { footnotes, .. } = &table.body[0][0].content else {
        panic!()
    };
    assert_eq!(footnotes.len(), 1);
    assert_eq!(content(&footnotes[0]), "one");
    assert_eq!(
        &source[footnotes[0].span.start..footnotes[0].span.end],
        "footnote:[one]"
    );
}
