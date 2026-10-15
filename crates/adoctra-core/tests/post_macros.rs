use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributePart as P, Attributes, BlockKind,
    Inline, ParseOptions, SourceMap, TokenKind,
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
fn expanded_macros_keep_reference_spans_and_definitions() {
    for (value, target, label) in [
        (
            "https://example.org",
            "https://example.org",
            "https://example.org",
        ),
        ("link:page[Ж&]", "page", "Ж&"),
        ("a@example.org", "mailto:a@example.org", "a@example.org"),
        ("mailto:a@example.org[Mail]", "mailto:a@example.org", "Mail"),
    ] {
        let source = format!(":v: {value}\n\n{{v}}");
        let doc = parse(&source).document;
        assert_eq!(doc.attributes.get("v"), Some(value));
        assert!(doc.attributes.raw_parts("v").is_none());
        assert_eq!(doc.blocks[0].attribute_context.get("v"), Some(value));
        let nodes = paragraph(&source);
        assert!(
            matches!(&nodes[0], Inline::AttributeRef { value: Some(v), parts, span, .. }
            if v == label && matches!(&parts[0], P::OpenLink(l) if l.target == target)
            && parts.last() == Some(&P::CloseLink) && &source[span.start..span.end] == "{v}")
        );
    }
}

#[test]
fn macros_cross_text_references_and_empty_values_without_losing_tokens() {
    let source = ":scheme: ttps\n:empty:\n:domain: example.org\n\nh{scheme}{empty}://{domain}";
    let nodes = paragraph(source);
    let Inline::Substituted { parts, value, span } = &nodes[0] else {
        panic!("{nodes:?}")
    };
    assert_eq!(value, "https://example.org");
    assert!(matches!(&parts[0], P::OpenLink(l) if l.target == "https://example.org"));
    assert_eq!(&source[span.start..span.end], "h{scheme}{empty}://{domain}");
    let references: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::AttributeRef {
                name,
                value,
                span,
                parts,
            } => {
                assert_eq!(value.as_deref(), Some(""));
                assert!(parts.is_empty());
                assert_eq!(&source[span.start..span.end], format!("{{{name}}}"));
                Some(name.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(references, ["scheme", "empty", "domain"]);
    let tokens = adoctra_core::tokens(source);
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::AttributeRef)
            .count(),
        3
    );
}

#[test]
fn attribute_owned_macros_consume_neighbors_and_preserve_formatted_labels() {
    let source = ":a: link:page[\n:b: pass:q[*Bold*]\n\n{a}{b}] tail";
    let nodes = paragraph(source);
    let Inline::AttributeRef {
        value, parts, span, ..
    } = &nodes[0]
    else {
        panic!()
    };
    assert_eq!(value.as_deref(), Some("Bold"));
    assert_eq!(&source[span.start..span.end], "{a}");
    assert!(matches!(&parts[0], P::OpenLink(l) if l.target == "page"));
    assert!(matches!(
        &parts[1],
        P::Open(adoctra_core::AttributeFormat::Strong)
    ));
    assert!(matches!(&nodes[1], Inline::AttributeRef { value: Some(v), .. } if v.is_empty()));
    assert!(matches!(nodes.last().unwrap(), Inline::Text { value, .. } if value == " tail"));
}

#[test]
fn pass_links_are_rescanned_once_at_use_and_escaped_links_lose_one_layer() {
    let nodes = paragraph(":v: pass:m[https://example.org]\n\n{v}");
    assert!(matches!(&nodes[0], Inline::AttributeRef { parts, .. }
        if parts.iter().filter(|p| matches!(p, P::OpenLink(_))).count() == 2));
    let nodes = paragraph(":v: \\link:page[Text]\n\n{v}");
    assert!(
        matches!(&nodes[0], Inline::AttributeRef { value: Some(v), parts, .. }
        if v == "link:page[Text]" && parts.is_empty())
    );
    let nodes = paragraph(":v: pass:m[\\link:page[Text]]\n\n{v}");
    assert!(
        matches!(&nodes[0], Inline::AttributeRef { value: Some(v), parts, .. }
        if v == "Text" && matches!(&parts[0], P::OpenLink(l) if l.target == "page"))
    );
}

#[test]
fn new_macro_pass_does_not_repeat_quotes_or_attribute_expansion() {
    let mut attributes = Attributes::new();
    attributes.set("v", "link:{target}[*Bold*]");
    attributes.set("target", "page");
    let result = parse_with_options(
        "{v}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert!(
        matches!(&inlines[0], Inline::AttributeRef { value: Some(v), parts, .. }
        if v == "*Bold*" && matches!(&parts[0], P::OpenLink(l) if l.target == "{target}"))
    );
}

#[test]
fn generated_link_fields_keep_lexical_protection_and_replacement_order() {
    let nodes = paragraph(
        ":v: link:page?a=1&b=2[Ж(C),role=red,title=A&B,window=_blank,opts=nofollow]\n\n{v}",
    );
    let Inline::AttributeRef { value, parts, .. } = &nodes[0] else {
        panic!()
    };
    assert_eq!(value.as_deref(), Some("Ж©"));
    let P::OpenLink(link) = &parts[0] else {
        panic!()
    };
    assert_eq!(link.target, "page?a=1&amp;b=2");
    assert_eq!(link.role.as_deref(), Some("red"));
    assert_eq!(link.title.as_deref(), Some("A&amp;B"));
    assert_eq!(link.rel.as_deref(), Some("nofollow noopener"));
}

#[test]
fn email_macros_observe_formatting_boundaries() {
    for body in ["*{v}*", "*Bold*{v}"] {
        let nodes = paragraph(&format!(":v: a@example.org\n\n{body}"));
        let reference = if let Inline::Strong { children, .. } = &nodes[0] {
            if body == "*{v}*" {
                &children[0]
            } else {
                &nodes[1]
            }
        } else {
            panic!()
        };
        assert!(
            matches!(reference, Inline::AttributeRef { value: Some(v), parts, .. }
            if v == "a@example.org" && parts.is_empty())
        );
    }
    let nodes = paragraph(":v: a@example.org\n\n*Email {v}*");
    let Inline::Strong { children, .. } = &nodes[0] else {
        panic!()
    };
    assert!(matches!(&children[1], Inline::AttributeRef { parts, .. }
        if matches!(&parts[0], P::OpenLink(l) if l.target == "mailto:a@example.org")));
}

#[test]
fn macros_after_length_changing_replacements_cover_consumed_source() {
    let source = ":s: ttps\n\nЖ(C) h{s}://example.org";
    let nodes = paragraph(source);
    let Inline::Substituted { value, span, .. } = nodes
        .iter()
        .find(|n| matches!(n, Inline::Substituted { .. }))
        .unwrap()
    else {
        panic!("{nodes:?}")
    };
    assert_eq!(value, "https://example.org");
    assert_eq!(
        nodes
            .iter()
            .filter_map(|n| match n {
                Inline::Text { value, .. } => Some(value.as_str()),
                _ => None,
            })
            .collect::<String>(),
        "Ж© "
    );
    assert_eq!(&source[span.start..span.end], "h{s}://example.org");
}

#[test]
fn unicode_partial_input_and_source_ids_remain_physical() {
    let source = "\u{feff}:s: ttps\r\n\r\nЖ h{s}://example.org";
    let mut sources = SourceMap::new();
    let source_id = sources.add("post.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let doc = parse_with_options(source, &options).document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    assert!(matches!(&inlines[1], Inline::Substituted { .. }));
    assert_eq!(sources.slice(inlines[1].span()), Some("h{s}://example.org"));
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        let partial = &source[..end];
        let _ = parse_with_options(partial, &options);
        for token in tokens_with_options(partial, &options) {
            assert_eq!(token.span.source_id, source_id);
            assert!(token.span.end <= end);
            assert!(partial.is_char_boundary(token.span.start));
            assert!(partial.is_char_boundary(token.span.end));
        }
    }
}

#[test]
fn plain_email_and_borrowed_section_titles_use_neutral_semantics() {
    let nodes = paragraph("Mail a@example.org.");
    assert!(matches!(&nodes[1], Inline::Substituted { value, parts, .. }
        if value == "a@example.org" && parts.iter().any(|p| matches!(p, P::OpenLink(l) if l.target == "mailto:a@example.org"))));
    let doc = parse(":s: ttps\n\n<<_httpsexample_org>>\n\n== h{s}://example.org").document;
    assert_eq!(doc.blocks[1].id.as_deref(), Some("_httpsexample_org"));
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    let Inline::CrossReference { children, .. } = &inlines[0] else {
        panic!()
    };
    assert!(matches!(&children[0], Inline::Substituted { parts, .. }
        if parts.iter().all(|p| !matches!(p, P::OpenLink(_) | P::CloseLink))));
}
