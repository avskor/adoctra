use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributeFormat as F, AttributePart as P,
    Attributes, BlockKind, Inline, ParseOptions, SourceMap, TokenKind,
};

fn paragraph(source: &str) -> Vec<Inline> {
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let BlockKind::Paragraph { inlines } = result.document.blocks[0].kind.clone() else {
        panic!("expected paragraph")
    };
    inlines
}

#[test]
fn direct_targets_and_parameters_expand_once_before_macros() {
    let source = ":target: page?a=1&b=2\n:role: red\n\nlink:{target}[*Ж&*,id=anchor,role={role},title=A&B,window=_blank,opts=nofollow]";
    let nodes = paragraph(source);
    let Inline::Substituted { value, parts, span } = &nodes[0] else {
        panic!("{nodes:?}")
    };
    assert_eq!(value, "Ж&");
    assert_eq!(
        &source[span.start..span.end],
        source.split("\n\n").last().unwrap()
    );
    let P::OpenLink(link) = &parts[0] else {
        panic!()
    };
    assert_eq!(link.target, "page?a=1&amp;b=2");
    assert_eq!(link.id.as_deref(), Some("anchor"));
    assert_eq!(link.role.as_deref(), Some("red"));
    assert_eq!(link.title.as_deref(), Some("A&amp;B"));
    assert_eq!(link.window.as_deref(), Some("_blank"));
    assert_eq!(link.rel.as_deref(), Some("nofollow noopener"));
    assert_eq!(
        &parts[1..],
        &[
            P::Open(F::Strong),
            P::Text("Ж&".into()),
            P::Close(F::Strong),
            P::CloseLink
        ]
    );
    let references: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::AttributeRef {
                name,
                value,
                parts,
                span,
            } => {
                assert_eq!(value.as_deref(), Some(""));
                assert!(parts.is_empty());
                assert_eq!(&source[span.start..span.end], format!("{{{name}}}"));
                Some(name.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(references, ["target", "role"]);
    let doc = parse(source).document;
    assert_eq!(doc.attributes.get("target"), Some("page?a=1&b=2"));
    assert_eq!(doc.blocks[0].attribute_context.get("role"), Some("red"));
}

#[test]
fn direct_links_keep_precise_spans_after_length_changing_replacements() {
    let source = "Ж(C) link:page(C)[Label(C)] … https://example.org.";
    let nodes = paragraph(source);
    let links: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::Substituted { value, parts, span } => Some((value, parts, *span)),
            _ => None,
        })
        .collect();
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].0, "Label©");
    assert!(matches!(&links[0].1[0], P::OpenLink(l) if l.target == "page&#169;"));
    assert_eq!(
        &source[links[0].2.start..links[0].2.end],
        "link:page(C)[Label(C)]"
    );
    assert_eq!(
        &source[links[1].2.start..links[1].2.end],
        "https://example.org"
    );
    for span in links.iter().map(|l| l.2) {
        assert!(adoctra_core::tokens(source)
            .iter()
            .any(|t| t.kind == TokenKind::Link && t.span == span));
    }
}

#[test]
fn formatted_labels_are_consumed_but_source_quote_tokens_survive() {
    let source = "link:page[*Bold _Italic_*]";
    let nodes = paragraph(source);
    let Inline::Substituted { value, parts, span } = &nodes[0] else {
        panic!()
    };
    assert_eq!(value, "Bold Italic");
    assert_eq!((span.start, span.end), (0, source.len()));
    assert_eq!(
        &parts[1..],
        &[
            P::Open(F::Strong),
            P::Text("Bold ".into()),
            P::Open(F::Emphasis),
            P::Text("Italic".into()),
            P::Close(F::Emphasis),
            P::Close(F::Strong),
            P::CloseLink,
        ]
    );
    let tokens = adoctra_core::tokens(source);
    for (kind, syntax) in [
        (TokenKind::Link, source),
        (TokenKind::Strong, "*Bold _Italic_*"),
        (TokenKind::Emphasis, "_Italic_"),
    ] {
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == kind && &source[t.span.start..t.span.end] == syntax),
            "{tokens:?}"
        );
    }
    let nodes = paragraph("*link:page[Bold]*");
    let Inline::Strong { children, span } = &nodes[0] else {
        panic!("{nodes:?}")
    };
    assert_eq!((span.start, span.end), (0, 17));
    assert!(
        matches!(&children[0], Inline::Substituted { value, span, .. } if value == "Bold" && span.start == 1 && span.end == 16)
    );
}

#[test]
fn target_quote_boundaries_and_double_label_markers_keep_source_spans() {
    let nodes = paragraph("link:page[A&##Mark##B]");
    assert!(matches!(&nodes[0], Inline::Substituted { parts, value, .. }
        if value == "A&MarkB" && parts.contains(&P::Open(F::Mark))));
    for target in [
        "*page*",
        "page_*name*",
        "foo_bar_baz",
        "page;*name*",
        "page}*name*",
    ] {
        let source = format!("link:{target}[Label]");
        let nodes = paragraph(&source);
        assert!(
            matches!(&nodes[0], Inline::Substituted { parts, .. }
            if matches!(&parts[0], P::OpenLink(l) if l.target == target)),
            "{nodes:?}"
        );
        assert!(!adoctra_core::tokens(&source)
            .iter()
            .any(|t| t.kind == TokenKind::Strong || t.kind == TokenKind::Emphasis));
    }
    let nodes = paragraph("link:page[a**Bold**b]");
    assert!(matches!(&nodes[0], Inline::Substituted { value, parts, .. }
        if value == "aBoldb" && parts.contains(&P::Open(F::Strong))));
    let source = "**link:page[Bold]**";
    let nodes = paragraph(source);
    let Inline::Strong { children, span } = &nodes[0] else {
        panic!("{nodes:?}")
    };
    assert_eq!(&source[span.start..span.end], source);
    assert!(
        matches!(&children[0], Inline::Substituted { value, parts, span }
        if value == "Bold" && matches!(&parts[0], P::OpenLink(l) if l.target == "page")
        && &source[span.start..span.end] == "link:page[Bold]")
    );
}

#[test]
fn incomplete_quote_and_link_runs_remain_safe_for_editor_input() {
    for source in ["*a ".repeat(10_000), "link:page[ ".repeat(10_000)] {
        let nodes = paragraph(&source);
        assert!(nodes.iter().all(|n| matches!(n, Inline::Text { .. })));
        assert!(!adoctra_core::tokens(&source)
            .iter()
            .any(|t| t.kind == TokenKind::Link));
    }
}

#[test]
fn macros_can_consume_only_one_quote_boundary_without_losing_the_other() {
    let nodes = paragraph("*link:page[Bold* rest]");
    assert!(
        matches!(&nodes[0], Inline::Substituted { parts, .. } if parts == &[P::Open(F::Strong)])
    );
    assert!(matches!(&nodes[1], Inline::Substituted { parts, value, .. }
        if value == "Bold rest" && parts.contains(&P::Close(F::Strong)) && parts.last() == Some(&P::CloseLink)));
    let nodes = paragraph("link:page[*Bold] rest*");
    assert!(
        matches!(&nodes[0], Inline::Substituted { parts, .. } if parts.contains(&P::Open(F::Strong)))
    );
    assert!(
        matches!(nodes.last().unwrap(), Inline::Substituted { parts, .. } if parts == &[P::Close(F::Strong)])
    );
}

#[test]
fn escaped_macros_are_literal_and_escaped_label_closers_are_consumed() {
    for (source, expected) in [
        (r"\link:page[Text]", "link:page[Text]"),
        (r"\https://example.org[Text]", "https://example.org[Text]"),
        (r"\mailto:a@example.org[Text]", "mailto:a@example.org[Text]"),
        (r"\https://example.org", "https://example.org"),
        (r"\a@example.org", "a@example.org"),
    ] {
        let nodes = paragraph(source);
        assert!(
            matches!(&nodes[..], [Inline::Text { value, .. }] if value == expected),
            "{nodes:?}"
        );
        assert!(!adoctra_core::tokens(source)
            .iter()
            .any(|t| t.kind == TokenKind::Link));
    }
    let nodes = paragraph(r"link:page[*A\]B*]");
    assert!(matches!(&nodes[0], Inline::Substituted { value, parts, .. }
        if value == "A]B" && parts.contains(&P::Open(F::Strong))));
}

#[test]
fn host_attribute_content_does_not_restart_quotes_or_attributes() {
    let mut attributes = Attributes::new();
    attributes.set("target", "{other}");
    attributes.set("other", "page");
    attributes.set("label", "*Literal*");
    let result = parse_with_options(
        "link:{target}[{label}]",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert!(
        matches!(&inlines[0], Inline::Substituted { parts, value, .. }
        if value == "*Literal*" && matches!(&parts[..], [P::OpenLink(l), P::Raw(_), P::CloseLink] if l.target == "{other}")),
        "{inlines:?}"
    );
}

#[test]
fn bare_links_are_processed_once_and_mailto_parameters_use_uri_encoding() {
    let nodes = paragraph("https://example.org");
    assert!(matches!(&nodes[..], [Inline::Substituted { parts, .. }]
        if parts.iter().filter(|p| matches!(p, P::OpenLink(_))).count() == 1));
    let nodes = paragraph(":email: a@example.org\n\nmailto:{email}[Mail,Hello world,Body & text]");
    assert!(
        matches!(&nodes[0], Inline::Substituted { parts, value, .. }
        if value == "Mail" && matches!(&parts[0], P::OpenLink(l)
            if l.target == "mailto:a@example.org?subject=Hello%20world&amp;body=Body%20%26amp%3B%20text")),
        "{nodes:?}"
    );
}

#[test]
fn physical_unicode_spans_and_partial_input_work_for_all_consumed_syntax() {
    let source = "\u{feff}:target: страница\r\n\r\nЖ(C) link:{target}[*Жёлтый*,role=red]";
    let mut sources = SourceMap::new();
    let source_id = sources.add("direct.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    let link = inlines
        .iter()
        .find(|n| matches!(n, Inline::Substituted { .. }))
        .unwrap();
    assert_eq!(
        sources.slice(link.span()),
        Some("link:{target}[*Жёлтый*,role=red]")
    );
    let tokens = tokens_with_options(source, &options);
    for (kind, syntax) in [
        (TokenKind::Link, "link:{target}[*Жёлтый*,role=red]"),
        (TokenKind::AttributeRef, "{target}"),
        (TokenKind::Strong, "*Жёлтый*"),
    ] {
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == kind && sources.slice(t.span) == Some(syntax)),
            "{tokens:?}"
        );
    }
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        let partial = &source[..end];
        let _ = parse_with_options(partial, &options);
        for token in tokens_with_options(partial, &options) {
            assert_eq!(token.span.source_id, source_id);
            assert!(token.span.end <= end);
            assert!(partial.get(token.span.start..token.span.end).is_some());
        }
    }
}
