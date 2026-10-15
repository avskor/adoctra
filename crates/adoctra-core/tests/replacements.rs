use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributePart, Attributes, BlockKind, Inline,
    ParseOptions, SourceMap, TokenKind,
};

fn inlines(block: &adoctra_core::Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines }
        | BlockKind::Heading { inlines, .. }
        | BlockKind::Verse { inlines, .. } => inlines,
        _ => panic!("expected inline content"),
    }
}

fn text(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Inline::Text { value, .. }
            | Inline::Raw { value, .. }
            | Inline::Substituted { value, .. } => value.clone(),
            Inline::AttributeRef { value, .. } => value.clone().unwrap_or_default(),
            Inline::Strong { children, .. } | Inline::Link { children, .. } => text(children),
            _ => panic!("unexpected inline {node:?}"),
        })
        .collect()
}

#[test]
fn replacements_cross_text_and_attribute_boundaries_without_losing_references() {
    let source =
        ":middle: C\n:left: (T\n:right: M)\n:empty:\n\n({middle}) {left}{right} (R{empty})";
    let result = parse(source);
    let nodes = inlines(&result.document.blocks[0]);
    assert_eq!(text(nodes), "© ™ ®");
    let refs: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::AttributeRef {
                name, value, span, ..
            } => Some((name.as_str(), value.as_deref(), *span)),
            _ => None,
        })
        .collect();
    assert_eq!(refs.len(), 4);
    for (name, _, span) in &refs {
        assert_eq!(&source[span.start..span.end], format!("{{{name}}}"));
    }
    assert_eq!(refs[0].1, Some(""));
    assert_eq!(refs[1].1, Some("™"));
    assert_eq!(refs[2].1, Some(""));
    assert_eq!(result.document.attributes.get("left"), Some("(T"));
    assert!(
        matches!(&nodes[0], Inline::Text { span, .. } if &source[span.start..span.end] == "({middle})")
    );
}

#[test]
fn entities_from_source_and_split_references_have_raw_nodes_and_complete_spans() {
    let source = ":name: copy\n\nДо &{name}; &#65; &#x1F642; после";
    let doc = parse(source).document;
    let nodes = inlines(&doc.blocks[0]);
    assert_eq!(text(nodes), "До &copy; &#65; &#x1F642; после");
    let raw: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::Raw { value, span } => Some((value.as_str(), &source[span.start..span.end])),
            _ => None,
        })
        .collect();
    assert_eq!(
        raw,
        [
            ("&copy;", "&{name};"),
            ("&#65;", "&#65;"),
            ("&#x1F642;", "&#x1F642;")
        ]
    );
    assert!(nodes
        .iter()
        .any(|n| matches!(n, Inline::AttributeRef { value: Some(v), .. } if v.is_empty())));
}

#[test]
fn stages_preserve_protection_and_do_not_rescan_restored_entities() {
    let doc =
        parse(":gt: &gt;\n:lt: <\n:raw: pass:[-> -&gt;]\n\n-{gt} {lt}-> {raw} &amp;copy;").document;
    assert_eq!(text(inlines(&doc.blocks[0])), "-&gt; <→ -> → &amp;copy;");
    let doc = parse(":slash: pass:[\\]\n:name: copy\n\n{slash}&{name}; \\(C) \\...").document;
    assert_eq!(text(inlines(&doc.blocks[0])), "&copy; (C) ...");
    assert!(inlines(&doc.blocks[0])
        .iter()
        .all(|n| !matches!(n, Inline::Raw { .. })));
}

#[test]
fn reference_order_and_unicode_boundaries_are_observable() {
    for (source, expected) in [
        ("a -- -- b", "a\u{2009}—\u{2009}-- b"),
        ("a\t--\tb", "a\t--\tb"),
        (
            "é--Ж e\u{301}--Ж ‿--a",
            "é—\u{200b}Ж e\u{301}—\u{200b}Ж ‿—\u{200b}a",
        ),
        ("a'1 1'a é'Ж ²'a", "a'1 1’a é’Ж ²'a"),
        ("(C)--a &copy;--a", "©--a &copy;--a"),
        ("`'", "’"),
        ("a\\-- b", "a-- b"),
    ] {
        assert_eq!(
            text(inlines(&parse(source).document.blocks[0])),
            expected,
            "{source}"
        );
    }
}

#[test]
fn formatting_boundaries_are_not_line_boundaries_or_removed_markup() {
    let doc = parse(":dash: -\n\n*x*{dash}-y a{dash}-*x* *-- x* *x --*").document;
    assert_eq!(text(inlines(&doc.blocks[0])), "x--y a--x -- x x --");
    let doc = parse(":c: C\n\n*({c})* link:https://example.org[({c}) &copy;]").document;
    assert_eq!(text(inlines(&doc.blocks[0])), "© © &copy;");
}

#[test]
fn host_parts_keep_markup_but_apply_replacements_across_part_boundaries() {
    let mut attributes = Attributes::new();
    attributes.set_parts(
        "v",
        vec![
            AttributePart::Raw("<b>(".into()),
            AttributePart::Text("C) -".into()),
            AttributePart::Raw("&gt;</b>".into()),
        ],
    );
    let source = "{v}";
    let doc = parse_with_options(
        source,
        &ParseOptions {
            attributes: attributes.clone(),
            ..Default::default()
        },
    )
    .document;
    let Inline::AttributeRef {
        value, parts, span, ..
    } = &inlines(&doc.blocks[0])[0]
    else {
        panic!()
    };
    assert_eq!(value.as_deref(), Some("<b>© →</b>"));
    assert_eq!(
        parts,
        &[
            AttributePart::Raw("<b>".into()),
            AttributePart::Text("© →".into()),
            AttributePart::Raw("</b>".into()),
        ]
    );
    assert_eq!(&source[span.start..span.end], "{v}");
    assert_eq!(doc.attributes.get("v"), attributes.get("v"));
    assert_eq!(doc.attributes.raw_parts("v"), attributes.raw_parts("v"));
}

#[test]
fn snapshots_source_coordinates_and_highlighting_survive_consumed_values() {
    let source = "\u{feff}:v: C\r\n\r\nДо ({v}) &copy;\r\n\r\n:v: R\r\n\r\n({v})";
    let mut sources = SourceMap::new();
    sources.add("unused", "");
    let source_id = sources.add("main.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let doc = parse_with_options(source, &options).document;
    assert_eq!(text(inlines(&doc.blocks[0])), "До © &copy;");
    assert_eq!(text(inlines(&doc.blocks[1])), "®");
    assert_eq!(doc.attributes.get("v"), Some("C"));
    assert_eq!(doc.blocks[0].attribute_context.get("v"), Some("C"));
    assert_eq!(doc.blocks[1].attribute_context.get("v"), Some("R"));
    for node in doc.blocks.iter().flat_map(inlines) {
        assert_eq!(node.span().source_id, source_id);
        assert!(sources.slice(node.span()).is_some());
    }
    let tokens = tokens_with_options(source, &options);
    let refs: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::AttributeRef)
        .collect();
    assert_eq!(refs.len(), 2);
    for token in refs {
        assert_eq!(sources.slice(token.span), Some("{v}"));
    }
}

#[test]
fn verse_handles_newlines_before_folding_and_verbatim_is_unchanged() {
    let source = ":dash: --\n\n[verse]\n____\na\n{dash}\nb\nc\n____\n\n----\n{dash} &copy;\n----";
    let doc = parse(source).document;
    assert_eq!(text(inlines(&doc.blocks[0])), "a\u{2009}—\u{2009}b\nc");
    assert!(matches!(&doc.blocks[1].kind, BlockKind::Listing { text } if text == "{dash} &copy;"));
}

#[test]
fn partial_inputs_keep_all_spans_in_bounds() {
    use adoctra_core::visit::{walk_document_mut, VisitorMut};
    struct Spans<'a>(&'a str);
    impl VisitorMut for Spans<'_> {
        fn visit_span(&mut self, span: &mut adoctra_core::Span) {
            assert!(
                self.0.get(span.start..span.end).is_some(),
                "{span:?} in {:?}",
                self.0
            );
        }
    }
    let source = "\u{feff}:v: C\r\n:raw: pass:[&amp;]\r\n\r\nДо ({v}) &{raw}copy; \\&#65; *({v})*";
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        let partial = &source[..end];
        let mut result = parse(partial);
        walk_document_mut(&mut Spans(partial), &mut result.document);
        for token in adoctra_core::tokens(partial) {
            assert!(partial.get(token.span.start..token.span.end).is_some());
        }
    }
}
