use adoctra_core::{
    parse, parse_with_options, tokens_with_options, BlockKind, Inline, ParseOptions, SourceId,
    Span, TokenKind,
};

fn paragraph(source: &str) -> String {
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    result
        .document
        .blocks
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Paragraph { inlines } => Some(
                inlines
                    .iter()
                    .map(|i| match i {
                        Inline::Text { value, .. } => value.as_str(),
                        _ => panic!("unexpected inline {i:?}"),
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("|")
}

#[test]
fn predicates_nest_and_do_not_apply_hidden_attributes() {
    assert_eq!(paragraph(":a:\n:b:\n\nifdef::a+b[all]\nifndef::a+missing[unless all]\nifndef::a,missing[hidden]\nifdef::missing[]\n:leaked:\nifndef::other[]\nhidden\nendif::[]\nendif::[]\nifndef::leaked[clean]"), "all unless all clean");
}

#[test]
fn attributes_change_only_at_parser_boundaries_and_hard_overrides_win() {
    assert_eq!(paragraph("body\n:a:\nifdef::a[hidden]\nifndef::a[shown]\n\n:a:\n\nifdef::a[present]\n\n:a!:\n\nifndef::a[absent]"), "body :a: shown|present|absent");
    let mut options = ParseOptions::default();
    options.attributes.set("a", "");
    let result = parse_with_options(":a!:\n\nifdef::a[locked]\nifndef::a[hidden]", &options);
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.document.attributes.get("a"), Some(""));
    assert!(
        matches!(&result.document.blocks[0].kind, BlockKind::Paragraph { inlines } if matches!(&inlines[0], Inline::Text { value, .. } if value == "locked"))
    );
}

#[test]
fn container_conditions_use_attributes_at_capture_and_mutations_survive_afterward() {
    let source = "====\n:a:\n\nifdef::a[hidden]\nifndef::a[inside]\n====\n\nifdef::a[after]";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    let BlockKind::Example { blocks } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert_eq!(blocks.len(), 1);
    assert!(
        matches!(&blocks[0].kind, BlockKind::Paragraph { inlines } if matches!(&inlines[0], Inline::Text { value, .. } if value == "inside"))
    );
    assert_eq!(
        result.document.blocks[1].attribute_context.get("a"),
        Some("")
    );
    assert_eq!(
        &source[result.document.blocks[1].span.start..result.document.blocks[1].span.end],
        "after"
    );
}

#[test]
fn header_conditions_see_derived_attributes_and_retain_declaration_spans() {
    let source = ":a:\r\nifdef::a[= Заголовок]\r\nifdef::a[Иван Иванов]\r\nifdef::author[v1.2, 2026-10-11: Draft]\r\n\r\n{author}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let header = result.document.header.unwrap();
    assert_eq!(
        &source[header.title_span.start..header.title_span.end],
        "= Заголовок"
    );
    let author = &header.authors[0];
    assert_eq!(&source[author.span.start..author.span.end], "Иван Иванов");
    let rev = header.revision.unwrap();
    assert_eq!(
        &source[rev.span.start..rev.span.end],
        "v1.2, 2026-10-11: Draft"
    );
    assert_eq!(
        result.document.attributes.get("author"),
        Some("Иван Иванов")
    );
    assert_eq!(result.document.attributes.get("revnumber"), Some("1.2"));
}

#[test]
fn unicode_crlf_comments_and_conditionals_compose_source_maps() {
    let source = ":a:\r\n\r\nЯ *до\r\n// комментарий\r\nifdef::missing[]\r\nскрыто\r\nendif::[]\r\nifdef::a[после*]";
    let options = ParseOptions {
        source_id: SourceId(17),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert!(result.diagnostics.is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    let Inline::Strong { children, span } = &inlines[1] else {
        panic!("{inlines:?}")
    };
    assert_eq!(
        *span,
        Span::in_source(
            SourceId(17),
            source.find('*').unwrap(),
            source.rfind('*').unwrap() + 1
        )
    );
    assert_eq!(
        children
            .iter()
            .map(|i| match i {
                Inline::Text { value, .. } => value.as_str(),
                _ => panic!(),
            })
            .collect::<String>(),
        "до  после"
    );
    assert_eq!(
        &source[result.comments[0].start..result.comments[0].end],
        "// комментарий"
    );
    assert_eq!(
        &source[result.inactive[0].start..result.inactive[0].end],
        "скрыто"
    );
    for span in result
        .comments
        .iter()
        .chain(&result.inactive)
        .chain(&result.conditional_directives)
    {
        assert_eq!(span.source_id, SourceId(17));
        assert!(source.get(span.start..span.end).is_some());
    }
}

#[test]
fn selected_inline_nodes_and_xref_diagnostics_have_original_offsets() {
    let source = "ifndef::missing[*Привет* <<unknown>>]";
    let result = parse(source);
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert!(
        matches!(&inlines[0], Inline::Strong { span, .. } if &source[span.start..span.end] == "*Привет*")
    );
    let diagnostic = &result.diagnostics[0];
    assert_eq!(
        &source[diagnostic.span.start..diagnostic.span.end],
        "<<unknown>>"
    );
}

#[test]
fn conditions_process_verbatim_but_not_top_level_comment_bodies() {
    let result = parse(":a:\n\n////\nendif::[]\nifdef::missing[]\n////\n\n----\nifdef::a[shown]\nifdef::missing[hidden]\n\\ifdef::a[plain]\n----");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(
        matches!(&result.document.blocks[0].kind, BlockKind::Listing { text } if text == "shown\nifdef::a[plain]")
    );
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.conditional_directives.len(), 3);
    assert_eq!(
        paragraph(":a:\n\n[comment]\nhidden\nendif::[]\nifdef::missing[]\n\nifdef::a[shown]"),
        "shown"
    );
}

#[test]
fn malformed_directives_diagnose_without_losing_the_document() {
    let source = "endif::[]\nifdef::[]\nifndef::missing[]\nshown\nendif::other[]\nendif::[text]";
    let options = ParseOptions {
        source_id: SourceId(4),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert_eq!(result.diagnostics.len(), 5, "{:?}", result.diagnostics);
    for d in &result.diagnostics {
        assert_eq!(d.span.source_id, SourceId(4));
        assert!(source.get(d.span.start..d.span.end).unwrap().contains("::"));
    }
    assert_eq!(result.document.blocks.len(), 1);
}

#[test]
fn tokens_hide_disabled_syntax_and_map_enabled_content() {
    let source = ":a:\n\nifdef::missing[]\n*hidden*\n----\nendif::[]\nifdef::a[*shown*]\n\n_after_";
    let options = ParseOptions {
        source_id: SourceId(9),
        ..Default::default()
    };
    let tokens = tokens_with_options(source, &options);
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Inactive && &source[t.span.start..t.span.end] == "*hidden*"));
    assert!(!tokens.iter().any(|t| t.kind == TokenKind::ListingDelimiter));
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Strong && &source[t.span.start..t.span.end] == "*shown*"));
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Emphasis && &source[t.span.start..t.span.end] == "_after_"));
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::ConditionalDirective));
    assert!(
        tokens
            .iter()
            .all(|t| t.span.source_id == SourceId(9)
                && source.get(t.span.start..t.span.end).is_some())
    );
    assert!(tokens
        .windows(2)
        .all(|w| w[0].span.start <= w[1].span.start));
}

#[test]
fn incomplete_and_deeply_nested_conditionals_are_safe_for_the_editor() {
    let source = ":a:\r\n\r\nifdef::a[Я *текст*]\r\nifndef::missing[]\r\nvisible\r\nendif::[]";
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        parse(&source[..end]);
        adoctra_core::tokens(&source[..end]);
    }
    let source = format!(
        "{}shown\n{}",
        "ifndef::missing[]\n".repeat(2048),
        "endif::[]\n".repeat(2048)
    );
    assert_eq!(paragraph(&source), "shown");
}

#[test]
fn empty_verbatim_tokens_after_a_deleted_region_have_valid_spans() {
    let source = "----\nifdef::missing[]\nhidden\nendif::[]\n\ntext\n----";
    for token in adoctra_core::tokens(source) {
        assert!(
            source.get(token.span.start..token.span.end).is_some(),
            "{token:?}"
        );
    }
}

#[test]
fn comments_generated_by_single_line_conditions_share_parser_and_token_ranges() {
    for source in [
        "ifndef::missing[// hidden]\nshown",
        "ifndef::missing[////]\nhidden\n////\nshown",
        "text\nifndef::missing[// hidden]\nmore",
        "[comment]\nifndef::missing[hidden]\n\nshown",
    ] {
        let result = parse(source);
        let comments: Vec<_> = adoctra_core::tokens(source)
            .into_iter()
            .filter(|t| t.kind == TokenKind::Comment)
            .map(|t| t.span)
            .collect();
        assert_eq!(result.comments, comments, "{source}");
    }
}
