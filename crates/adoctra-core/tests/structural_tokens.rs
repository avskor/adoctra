use adoctra_core::{
    parse, parse_resource, parse_with_options, tokens, BlockKind, Inline, MemoryResources,
    ParseOptions, SourceId, SourceMap, TokenKind,
};

#[test]
fn multiline_quotes_share_ast_ranges_in_unicode_crlf_paragraphs_and_lists() {
    for source in [
        "Ж *первая\r\nвторая* строка",
        "* Ж *первая\r\n  вторая* строка",
        "> Ж *первая\r\n> вторая* строка",
    ] {
        let result = parse_with_options(
            source,
            &ParseOptions {
                source_id: SourceId(7),
                ..Default::default()
            },
        );
        struct Strong(Vec<adoctra_core::Span>);
        impl adoctra_core::visit::VisitorMut for Strong {
            fn visit_inline(&mut self, node: &mut Inline) {
                if let Inline::Strong { span, .. } = node {
                    self.0.push(*span);
                }
                adoctra_core::visit::walk_inline_mut(self, node);
            }
        }
        let mut strong = Strong(Vec::new());
        let mut document = result.document.clone();
        adoctra_core::visit::walk_document_mut(&mut strong, &mut document);
        assert!(!strong.0.is_empty(), "{source}");
        for span in strong.0 {
            assert!(
                result
                    .tokens
                    .iter()
                    .any(|t| t.kind == TokenKind::Strong && t.span == span),
                "{source}: {:?}",
                result.tokens
            );
        }
        assert!(result
            .tokens
            .iter()
            .all(|t| t.span.source_id == SourceId(7)));
    }
}

#[test]
fn paragraph_continuations_do_not_become_block_metadata_or_macros() {
    let source = "text\n.Title *bold*\nSee [[inline]] here\nimage::pic.png[]\nNOTE: still text";
    let result = parse(source);
    assert!(matches!(
        result.document.blocks[0].kind,
        BlockKind::Paragraph { .. }
    ));
    assert!(!result.tokens.iter().any(|t| matches!(
        t.kind,
        TokenKind::BlockTitle | TokenKind::BlockMacro | TokenKind::AdmonitionLabel
    )));
    assert!(result.tokens.iter().any(|t| t.kind == TokenKind::Strong));
    assert!(result.tokens.iter().any(|t| t.kind == TokenKind::Anchor));
    assert_eq!(result.tokens, tokens(source));
}

#[test]
fn multiline_inline_scanning_stops_at_cell_and_paragraph_boundaries() {
    for source in ["*first\n\nsecond*", "|===\n|*first |second*\n|==="] {
        assert!(
            !parse(source)
                .tokens
                .iter()
                .any(|t| t.kind == TokenKind::Strong),
            "{source}"
        );
    }
    let source = "|===\n|*first\nsecond*\n|===";
    assert!(parse(source)
        .tokens
        .iter()
        .any(|t| t.kind == TokenKind::Strong));
}

#[test]
fn verbatim_blocks_do_not_reinterpret_source_as_structure() {
    for style in ["source,rust", "literal"] {
        let source = format!("[{style}]\n----\n.Title\n[[anchor]]\n*first\nsecond*\n----");
        let result = parse(&source);
        assert!(!result.tokens.iter().any(|t| matches!(
            t.kind,
            TokenKind::Strong | TokenKind::BlockTitle | TokenKind::Anchor
        )));
        assert!(result
            .tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::ListingText | TokenKind::LiteralText)));
    }
    let source = "[subs=quotes]\n----\n*first\nsecond*\n----";
    assert!(!parse(source)
        .tokens
        .iter()
        .any(|t| t.kind == TokenKind::Strong));
}

#[test]
fn multiline_syntax_respects_selected_substitutions_and_passthroughs() {
    for (source, strong) in [
        ("[subs=quotes]\n*first\nsecond* {v}", true),
        ("[subs=none]\n*first\nsecond* {v}", false),
        ("pass:[*first\nsecond*]", false),
    ] {
        let result = parse(source);
        assert_eq!(
            result.tokens.iter().any(|t| t.kind == TokenKind::Strong),
            strong,
            "{source}"
        );
        assert!(
            !result
                .tokens
                .iter()
                .any(|t| t.kind == TokenKind::AttributeRef),
            "{source}"
        );
    }
}

#[test]
fn selected_and_indented_include_keeps_multiline_unicode_source_spans() {
    let mut resources = MemoryResources::new();
    resources
        .insert("/main.adoc", "include::part.adoc[tag=body,indent=0]")
        .unwrap();
    let part = "// tag::body[]\r\n  *Ж первая\r\n  вторая*\r\n// end::body[]";
    resources.insert("/part.adoc", part).unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/main.adoc", None, &resources).unwrap();
    let result = parse_resource(
        &mut sources,
        &ParseOptions {
            source_id,
            ..Default::default()
        },
        &resources,
    )
    .unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    let span = inlines
        .iter()
        .find(|n| matches!(n, Inline::Strong { .. }))
        .unwrap()
        .span();
    let fragments = &result.provenance[span.provenance.unwrap()];
    let tokens: Vec<_> = result
        .tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Strong)
        .map(|t| t.span)
        .collect();
    assert_eq!(&tokens, fragments);
    let text: String = tokens.iter().map(|&s| sources.slice(s).unwrap()).collect();
    assert_eq!(text, "*Ж первая\r\nвторая*");
}

#[test]
fn included_multiline_syntax_has_physical_fragments_and_single_resource_read() {
    use adoctra_core::{Resource, ResourceProvider, ResourceRequest, ResourceResult};
    use std::cell::Cell;
    struct Counted {
        resources: MemoryResources,
        reads: Cell<usize>,
    }
    impl ResourceProvider for Counted {
        fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
            self.reads.set(self.reads.get() + 1);
            self.resources.load(request)
        }
    }
    let root = "Ж *first\r\ninclude::part.adoc[]\r\nlast*";
    let part = "середина";
    let mut resources = MemoryResources::new();
    resources.insert("/main.adoc", root).unwrap();
    resources.insert("/part.adoc", part).unwrap();
    let resources = Counted {
        resources,
        reads: Cell::new(0),
    };
    let mut sources = SourceMap::new();
    let source_id = sources.load("/main.adoc", None, &resources).unwrap();
    resources.reads.set(0);
    let result = parse_resource(
        &mut sources,
        &ParseOptions {
            source_id,
            ..Default::default()
        },
        &resources,
    )
    .unwrap();
    assert_eq!(resources.reads.get(), 1);
    let strong: Vec<_> = result
        .tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Strong)
        .collect();
    assert!(
        strong.iter().any(|t| sources.slice(t.span) == Some(part)),
        "{strong:?}"
    );
    assert!(strong
        .iter()
        .any(|t| sources.slice(t.span).is_some_and(|s| s.contains("*first"))));
    assert!(strong
        .iter()
        .any(|t| sources.slice(t.span).is_some_and(|s| s.contains("last*"))));
    for token in &result.tokens {
        assert!(sources.slice(token.span).is_some(), "{token:?}");
        assert!(token.span.provenance.is_none());
    }
}

#[test]
fn incomplete_unicode_input_keeps_token_ranges_valid() {
    let source = "= Ж\r\n\r\n[quote]\r\n____\r\n*слово\r\nвторая* pass:[<b>]\r\n____\r\n\r\n|===\r\n|*ячейка\r\nтекст*\r\n|===";
    for end in source.char_indices().map(|(i, _)| i).chain([source.len()]) {
        let input = &source[..end];
        for token in parse(input).tokens {
            assert!(
                input.get(token.span.start..token.span.end).is_some(),
                "prefix {end}: {token:?}"
            );
        }
    }
}
