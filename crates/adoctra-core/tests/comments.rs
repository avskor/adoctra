use adoctra_core::{
    parse, parse_with_options, tokens_with_options, BlockKind, Inline, ParseOptions, Severity,
    SourceMap, TokenKind,
};

#[test]
fn comments_in_header_do_not_become_metadata_or_attributes() {
    let source = "// leading\n= Doc\n// author follows\nJane Doe\n////\n:author: Hidden\n\n= Hidden\n////\nv2: Draft\n// end\n\n{author} {revnumber}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.comments.len(), 4);
    assert_eq!(result.document.blocks.len(), 2);
    assert_eq!(result.document.attributes.get("author"), Some("Jane Doe"));
    assert_eq!(result.document.attributes.get("revnumber"), Some("2"));
    let header = result.document.header.unwrap();
    assert_eq!(header.authors[0].name, "Jane Doe");
    assert_eq!(header.revision.unwrap().remark.as_deref(), Some("Draft"));
}

#[test]
fn removed_lines_keep_unicode_crlf_and_nested_inline_positions() {
    let source = "Привет *жирный\r\n// скрыто ** <<missing>>\r\n_курсив_* и <<target>>\r\n\r\n[[target]]\r\nЦель";
    let mut sources = SourceMap::new();
    sources.add("unused", "");
    let id = sources.add("doc", source);
    let options = ParseOptions {
        source_id: id,
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        sources.slice(result.comments[0]),
        Some("// скрыто ** <<missing>>")
    );
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    let Inline::Strong { span, children } = &inlines[1] else {
        panic!("{inlines:?}")
    };
    assert_eq!(
        sources.slice(*span),
        Some("*жирный\r\n// скрыто ** <<missing>>\r\n_курсив_*")
    );
    let Inline::Emphasis { span, children } = &children[1] else {
        panic!("{children:?}")
    };
    assert_eq!(sources.slice(*span), Some("_курсив_"));
    assert_eq!(sources.slice(children[0].span()), Some("курсив"));
    let xref = inlines.last().unwrap();
    assert_eq!(sources.slice(xref.span()), Some("<<target>>"));
    let tokens = tokens_with_options(source, &options);
    let comments: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Comment)
        .map(|t| t.span)
        .collect();
    assert_eq!(comments, result.comments);
    assert!(tokens.iter().all(|token| token.span.source_id == id));
}

#[test]
fn comment_block_breaks_paragraph_and_preserves_pending_metadata() {
    let source = "before\n/////\n====\n////\n:who: hidden\n/////\n.Title\n// note\n[[id]]\n////\nsecret\n////\nafter";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.document.blocks.len(), 2);
    let after = &result.document.blocks[1];
    assert_eq!(after.id.as_deref(), Some("id"));
    assert!(after.title.is_some());
    assert!(result.document.attributes.get("who").is_none());
    assert_eq!(result.comments.len(), 3);
    assert_eq!(&source[after.span.start..after.span.end], "after");
}

#[test]
fn unterminated_comment_is_a_warning_at_the_opening_fence() {
    let source = "visible\n\n/////\nsecret\n////\nmore";
    let result = parse_with_options(
        source,
        &ParseOptions {
            source_id: adoctra_core::SourceId(9),
            ..Default::default()
        },
    );
    assert_eq!(result.document.blocks.len(), 1);
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].end, source.len());
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.severity, Severity::Warning);
    assert_eq!(diagnostic.span.source_id.0, 9);
    assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "/////");
    assert!(parse("////").document.blocks.is_empty());
}

#[test]
fn verbatim_regions_preserve_comment_syntax_and_header_markers() {
    for (style, fence) in [
        ("", "----"),
        ("", "...."),
        ("", "++++"),
        ("[verse]\n", "____"),
        ("[literal]\n", "--"),
        ("[source]\n", "--"),
        ("[pass]\n", "--"),
        ("[verse]\n", "--"),
    ] {
        let source = format!("{style}{fence}\n// visible\n////\n:who: visible\n////\n{fence}");
        let result = parse(&source);
        assert!(result.comments.is_empty(), "{source}");
        assert!(result.diagnostics.is_empty(), "{source}");
        assert!(
            !tokens_with_options(&source, &Default::default())
                .iter()
                .any(|t| t.kind == TokenKind::Comment),
            "{source}"
        );
    }
    for style in ["literal", "verse"] {
        let source = format!("[{style}]\none\n// visible\n////\ntwo\n////");
        assert!(parse(&source).comments.is_empty(), "{source}");
    }
}

#[test]
fn triple_slashes_inline_slashes_and_indented_lines_are_not_comments() {
    let source = "text // visible\n/// visible\n//// x\n // visible\n\\// visible";
    let result = parse(source);
    assert!(result.comments.is_empty());
    assert_eq!(result.document.blocks.len(), 1);
}

#[test]
fn line_comments_in_lists_keep_siblings_and_nested_lists() {
    let source = "* one\n// note\nwrapped\n** nested\n// second\n* two";
    let result = parse(source);
    assert_eq!(result.comments.len(), 2);
    assert_eq!(result.document.blocks.len(), 1);
    let BlockKind::List { items, .. } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].blocks.len(), 1);
    assert_eq!(
        items[0].inlines,
        vec![Inline::Text {
            value: "one wrapped".into(),
            span: adoctra_core::Span::new(2, source.find("\n**").unwrap())
        }]
    );
}

#[test]
fn block_comment_ends_list_unless_attached_with_continuation() {
    let detached = parse("* one\n////\nhidden\n////\n* two");
    assert_eq!(detached.document.blocks.len(), 2);
    let attached = parse("* one\n+\n////\nhidden\n////\n* two");
    assert_eq!(attached.document.blocks.len(), 1);
    let BlockKind::List { items, .. } = &attached.document.blocks[0].kind else {
        panic!()
    };
    assert_eq!(items.len(), 2);
    assert!(items[0].blocks.is_empty());
    let canceled = parse("* one\n+\n// hidden\n----\nx\n----");
    assert_eq!(canceled.document.blocks.len(), 2);
    assert!(matches!(
        canceled.document.blocks[1].kind,
        BlockKind::Listing { .. }
    ));
}

#[test]
fn hidden_attributes_and_other_fences_have_no_effect() {
    let result = parse("====\n////\n----\n:who: hidden\n////\n// ignored\nvisible\n====");
    assert!(result.diagnostics.is_empty());
    let BlockKind::Example { blocks } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].attribute_context.get("who").is_none());
}

#[test]
fn enclosing_container_fence_is_seen_before_nested_comments() {
    let source = "====\n////\n====\n:who: hidden\n////\n// ignored\nvisible\n====";
    let result = parse(source);
    let BlockKind::Example { blocks } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert!(blocks.is_empty());
    assert_eq!(result.document.blocks.len(), 1);
    assert_eq!(result.comments.len(), 2);
    assert_eq!(result.comments[0].end, source.find("\n====\n:who").unwrap());
    assert_eq!(result.diagnostics.len(), 2);
}

#[test]
fn comment_style_discards_paragraph_or_open_body_and_consumes_metadata() {
    for body in [
        "secret <<missing>>\n:who: hidden",
        "--\nsecret <<missing>>\n\n:who: hidden\n--",
    ] {
        let source = format!(".Hidden\n[#secret]\n[comment]\n{body}\n\nVisible");
        let result = parse(&source);
        assert!(result.diagnostics.is_empty());
        assert_eq!(result.document.blocks.len(), 1);
        assert!(result.document.blocks[0].id.is_none());
        assert!(result.document.blocks[0].title.is_none());
        assert!(result.document.blocks[0]
            .attribute_context
            .get("who")
            .is_none());
        assert_eq!(result.comments.len(), 1);
        assert_eq!(
            &source[result.comments[0].start..result.comments[0].end],
            body
        );
        let tokens = tokens_with_options(&source, &Default::default());
        assert_eq!(
            tokens
                .iter()
                .filter(|t| t.kind == TokenKind::Comment)
                .count(),
            1
        );
        assert!(!tokens.iter().any(|t| t.kind == TokenKind::CrossReference));
    }
    let result = parse("[comment]\n====\nvisible\n====");
    assert!(result.comments.is_empty());
    assert!(matches!(
        result.document.blocks[0].kind,
        BlockKind::Example { .. }
    ));
}

#[test]
fn unresolved_reference_after_comment_has_original_source_span() {
    let source = "before\n// hidden\n<<missing>>";
    let result = parse(source);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        result.diagnostics[0].span.start,
        source.find("<<missing>>").unwrap()
    );
    assert_eq!(result.diagnostics[0].span.end, source.len());
}

#[test]
fn comment_markers_after_list_or_admonition_prefix_are_content() {
    for source in ["* // visible\n// hidden", "NOTE: // visible\n// hidden"] {
        let result = parse(source);
        assert_eq!(result.comments.len(), 1);
        assert_eq!(
            &source[result.comments[0].start..result.comments[0].end],
            "// hidden"
        );
        let inlines = match &result.document.blocks[0].kind {
            BlockKind::List { items, .. } => &items[0].inlines,
            BlockKind::Admonition { blocks, .. } => {
                let BlockKind::Paragraph { inlines } = &blocks[0].kind else {
                    panic!()
                };
                inlines
            }
            _ => panic!(),
        };
        let Inline::Text { value, span } = &inlines[0] else {
            panic!()
        };
        assert_eq!(value, "// visible");
        assert_eq!(&source[span.start..span.end], "// visible");
    }
}
