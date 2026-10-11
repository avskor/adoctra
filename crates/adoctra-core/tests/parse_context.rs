use adoctra_core::{
    parse, parse_with_options, tokens_with_options,
    visit::{self, VisitorMut},
    Attributes, Block, BlockKind, Inline, ParseOptions, SourceId, SourceMap, Span,
};
use std::sync::Arc;

fn attribute_values(block: &Block) -> Vec<&str> {
    let inlines = match &block.kind {
        BlockKind::Paragraph { inlines } | BlockKind::Heading { inlines, .. } => inlines,
        _ => panic!("expected paragraph or heading"),
    };
    inlines
        .iter()
        .filter_map(|i| match i {
            Inline::AttributeRef { value, .. } => value.as_deref(),
            _ => None,
        })
        .collect()
}

#[test]
fn title_uses_header_attributes_without_moving_original_byte_ranges() {
    let source = "= Привет {who}\r\n:who: мир\r\n\r\nHello {who}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.document.blocks.len(), 2);
    assert_eq!(attribute_values(&result.document.blocks[0]), ["мир"]);
    assert_eq!(attribute_values(&result.document.blocks[1]), ["мир"]);
    assert_eq!(result.document.attributes.get("who"), Some("мир"));
    let BlockKind::Heading { inlines, .. } = &result.document.blocks[0].kind else {
        panic!()
    };
    let span = inlines.last().unwrap().span();
    assert_eq!(&source[span.start..span.end], "{who}");
    assert_eq!(
        result.document.blocks[0].span,
        Span::new(0, "= Привет {who}".len())
    );
}

#[test]
fn snapshots_share_storage_until_an_assignment_and_nested_changes_persist() {
    let source = ":who: first\n\n{who}\n\n{who}\n\n====\n:who: inside\n\n{who}\n====\n\n{who}";
    let doc = parse(source).document;
    assert!(Arc::ptr_eq(
        &doc.blocks[0].attribute_context,
        &doc.blocks[1].attribute_context
    ));
    assert_eq!(attribute_values(&doc.blocks[0]), ["first"]);
    assert_eq!(
        doc.attributes.get("who"),
        Some("first"),
        "document attributes are the header snapshot"
    );
    let BlockKind::Example { blocks } = &doc.blocks[2].kind else {
        panic!()
    };
    assert_eq!(doc.blocks[2].attribute_context.get("who"), Some("inside"));
    assert_eq!(attribute_values(&blocks[0]), ["inside"]);
    assert_eq!(attribute_values(&doc.blocks[3]), ["inside"]);
    assert!(!Arc::ptr_eq(
        &doc.blocks[0].attribute_context,
        &doc.blocks[3].attribute_context
    ));
    assert!(Arc::ptr_eq(
        &blocks[0].attribute_context,
        &doc.blocks[3].attribute_context
    ));
}

#[test]
fn attribute_unset_forms_and_case_insensitive_values() {
    for unset in [":!who:", ":who!:"] {
        let doc = parse(&format!(
            ":WHO: World\n\n{{WHO}} {{who}}\n\n{unset}\n\n{{who}}"
        ))
        .document;
        assert_eq!(attribute_values(&doc.blocks[0]), ["World", "World"]);
        assert_eq!(attribute_values(&doc.blocks[1]), Vec::<&str>::new());
        assert_eq!(doc.blocks[1].attribute_context.get("who"), None);
        let BlockKind::Paragraph { inlines } = &doc.blocks[1].kind else {
            panic!()
        };
        assert!(matches!(
            &inlines[0],
            Inline::AttributeRef { value: None, .. }
        ));
    }
}

#[test]
fn entries_inside_paragraphs_and_verbatim_do_not_change_attributes() {
    let source = ":who: first\n\n{who}\n:who: second\n{who}\n\n----\n:who: third\n----\n\n{who}";
    let doc = parse(source).document;
    assert_eq!(attribute_values(&doc.blocks[0]), ["first", "first"]);
    assert_eq!(attribute_values(&doc.blocks[2]), ["first"]);
    let tokens = adoctra_core::tokens(source);
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == adoctra_core::TokenKind::AttributeEntry)
            .count(),
        1
    );
}

#[test]
fn header_attribute_values_substitute_once_and_keep_inline_markup_raw() {
    let source =
        ":one: first\n:two: {one} *second*\n:escaped: \\{one}\n:one: changed\n\n{two} {escaped}";
    let doc = parse(source).document;
    assert_eq!(
        attribute_values(&doc.blocks[0]),
        ["first *second*", "{one}"]
    );
}

#[test]
fn hard_options_override_source_assignments_and_unsets() {
    let mut attributes = Attributes::new();
    attributes.set("WHO", "option");
    let options = ParseOptions {
        attributes,
        ..Default::default()
    };
    let doc = parse_with_options(
        "= {who}\n:who: header\n\n{who}\n\n:who!:\n\n{who}",
        &options,
    )
    .document;
    for block in &doc.blocks {
        assert_eq!(attribute_values(block), ["option"]);
    }
    assert_eq!(options.attributes.get("who"), Some("option"));
}

#[test]
fn captions_use_each_blocks_context_and_unset_does_not_advance_counter() {
    let doc = parse(".A\n====\na\n====\n\n:!example-caption:\n\n.B\n====\nb\n====\n\n:example-caption: Eg\n\n.C\n====\nc\n====").document;
    assert_eq!(
        doc.blocks
            .iter()
            .map(|b| b.caption.as_deref())
            .collect::<Vec<_>>(),
        [Some("Example 1. "), None, Some("Eg 2. ")]
    );
}

#[test]
fn captions_follow_ruby_block_completion_order_and_context() {
    let doc =
        parse(".Outer\n====\n:example-caption: Inner\n\n.Child\n[example]\nchild\n====").document;
    assert_eq!(doc.blocks[0].caption.as_deref(), Some("Inner 2. "));
    let BlockKind::Example { blocks } = &doc.blocks[0].kind else {
        panic!()
    };
    assert_eq!(blocks[0].caption.as_deref(), Some("Inner 1. "));
}

#[test]
fn named_source_is_preserved_in_nodes_list_items_tokens_and_diagnostics() {
    let mut sources = SourceMap::new();
    sources.add("unrelated.adoc", "unrelated");
    let source_id = sources.add(
        "chapter.adoc",
        ".Title\n[[q,Ref]]\n[quote, A, B]\n____\n* text *bold* <<missing>>\n\n____",
    );
    let source = sources.get(source_id).unwrap().text();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let mut result = parse_with_options(source, &options);
    struct Origins {
        expected: SourceId,
        count: usize,
    }
    impl VisitorMut for Origins {
        fn visit_span(&mut self, span: &mut Span) {
            assert_eq!(span.source_id, self.expected);
            self.count += 1;
        }
    }
    let mut origins = Origins {
        expected: source_id,
        count: 0,
    };
    visit::walk_document_mut(&mut origins, &mut result.document);
    assert!(origins.count >= 12, "visited {} spans", origins.count);
    assert_eq!(result.diagnostics.len(), 1);
    let span = result.diagnostics[0].span;
    assert_eq!(span.source_id, source_id);
    assert_eq!(sources.slice(span), Some("<<missing>>"));
    for token in tokens_with_options(source, &options) {
        assert_eq!(token.span.source_id, source_id);
        assert!(sources.slice(token.span).is_some());
    }
    assert_eq!(sources.get(source_id).unwrap().name(), "chapter.adoc");
}

#[test]
fn syntax_diagnostics_use_the_configured_source_too() {
    let result = parse_with_options(
        "----\nx",
        &ParseOptions {
            source_id: SourceId(7),
            ..Default::default()
        },
    );
    assert_eq!(
        result.diagnostics[0].span,
        Span::in_source(SourceId(7), 0, 4)
    );
}
