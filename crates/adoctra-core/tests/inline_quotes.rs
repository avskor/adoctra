use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributeFormat as F, AttributePart as P,
    Attributes, BlockKind, Inline, ParseOptions, SourceMap, TokenKind,
};

fn paragraph(source: &str) -> Vec<Inline> {
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let BlockKind::Paragraph { inlines } = result.document.blocks[0].kind.clone() else {
        panic!()
    };
    inlines
}

fn text(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|n| match n {
            Inline::Text { value, .. }
            | Inline::Raw { value, .. }
            | Inline::Substituted { value, .. } => value.clone(),
            Inline::AttributeRef { value, .. } => value.clone().unwrap_or_default(),
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Monospace { children, .. }
            | Inline::Mark { children, .. }
            | Inline::Superscript { children, .. }
            | Inline::Subscript { children, .. } => text(children),
            _ => panic!("{n:?}"),
        })
        .collect()
}

#[test]
fn escaped_source_markers_do_not_restart_quotes_but_later_stages_still_run() {
    for marker in ["*", "_", "`", "#", "^", "~"] {
        let source = format!(":v: Ж(C)\n\n\\{marker}{{v}}{marker}");
        let nodes = paragraph(&source);
        assert_eq!(text(&nodes), format!("{marker}Ж©{marker}"), "{nodes:?}");
        assert!(
            nodes
                .iter()
                .all(|n| matches!(n, Inline::Text { .. } | Inline::AttributeRef { .. })),
            "{nodes:?}"
        );
        assert!(adoctra_core::tokens(&source)
            .iter()
            .any(|t| t.kind == TokenKind::AttributeRef));
        assert!(!adoctra_core::tokens(&source).iter().any(|t| matches!(
            t.kind,
            TokenKind::Strong
                | TokenKind::Emphasis
                | TokenKind::Monospace
                | TokenKind::Mark
                | TokenKind::Superscript
                | TokenKind::Subscript
        )));
    }
    let nodes = paragraph(r"\*Literal _Italic_*");
    assert!(nodes.iter().any(|n| matches!(n, Inline::Emphasis { .. })));
    assert!(!nodes.iter().any(|n| matches!(n, Inline::Strong { .. })));
}

#[test]
fn shared_stage_order_preserves_nested_source_nodes_and_byte_ranges() {
    let source = "*Outer **Inner** Tail*";
    let nodes = paragraph(source);
    let Inline::Strong { children, span } = &nodes[0] else {
        panic!("{nodes:?}")
    };
    assert_eq!(&source[span.start..span.end], source);
    let Inline::Strong { children, span } = &children[1] else {
        panic!("{children:?}")
    };
    assert_eq!(&source[span.start..span.end], "**Inner**");
    assert_eq!(text(children), "Inner");
    assert_eq!(text(&nodes), "Outer Inner Tail");
    // Escaping the double-marker stage still permits the later single stage.
    let nodes = paragraph(r"\**Bold**");
    assert_eq!(text(&nodes), "*Bold*");
    assert!(matches!(&nodes[0], Inline::Strong { .. }));
}

#[test]
fn source_quote_metadata_is_semantic_and_preserves_consumed_references() {
    let source = ":role: red\n\nBefore [#anchor.{role}]*Ж&* after";
    let nodes = paragraph(source);
    let open = nodes
        .iter()
        .find_map(|n| match n {
            Inline::Substituted { parts, span, .. }
                if matches!(&parts[0], P::OpenWithAttributes { .. }) =>
            {
                Some((parts, span))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(&source[open.1.start..open.1.end], "[#anchor.{role}]*");
    assert_eq!(
        &open.0[..],
        &[P::OpenWithAttributes {
            format: F::Strong,
            id: Some("anchor".into()),
            role: Some("red".into())
        }]
    );
    assert_eq!(text(&nodes), "Before Ж& after");
    assert!(nodes.iter().any(
        |n| matches!(n, Inline::AttributeRef { name, value: Some(value), span, .. }
        if name == "role" && value.is_empty() && &source[span.start..span.end] == "{role}")
    ));
    let tokens = adoctra_core::tokens(source);
    for (kind, syntax) in [
        (TokenKind::Strong, "[#anchor.{role}]*Ж&*"),
        (TokenKind::AttributeRef, "{role}"),
    ] {
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == kind && &source[t.span.start..t.span.end] == syntax),
            "{tokens:?}"
        );
    }
    let doc = parse(source).document;
    assert_eq!(doc.attributes.get("role"), Some("red"));
    assert_eq!(doc.blocks[0].attribute_context.get("role"), Some("red"));
}

#[test]
fn typographic_quotes_and_decorated_wrappers_keep_both_overlapping_edits() {
    for (source, expected) in [
        ("A [red]\"`Жёлтый`\" B", "A “Жёлтый” B"),
        ("A [red]'`Жёлтый`' B", "A ‘Жёлтый’ B"),
    ] {
        let nodes = paragraph(source);
        assert_eq!(text(&nodes), expected, "{nodes:?}");
        assert!(nodes.iter().any(|n| matches!(n, Inline::Substituted { parts, .. }
            if matches!(&parts[..], [P::OpenWithAttributes { format: F::Span, role: Some(role), .. }] if role == "red"))));
        for node in nodes {
            let span = node.span();
            assert!(source.get(span.start..span.end).is_some(), "{node:?}");
        }
    }
}

#[test]
fn metadata_replacements_and_link_labels_keep_neutral_parts() {
    let source = ":role: red\n\nЖ(C) link:page[[{role}]*Bold(C)*]";
    let nodes = paragraph(source);
    let (parts, span) = nodes
        .iter()
        .find_map(|n| match n {
            Inline::Substituted { parts, span, .. } if matches!(&parts[0], P::OpenLink(_)) => {
                Some((parts, span))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        &source[span.start..span.end],
        "link:page[[{role}]*Bold(C)*]"
    );
    assert!(parts.iter().any(|p| matches!(p, P::OpenWithAttributes { format: F::Strong, role: Some(role), .. } if role == "red")));
    assert!(parts.contains(&P::Text("Bold©".into())));
    assert!(nodes.iter().any(|n| matches!(n, Inline::AttributeRef { name, value: Some(v), .. } if name == "role" && v.is_empty())));
    let nodes = paragraph("Before [id(C).role(C)]*Text(C)* after");
    assert!(nodes.iter().any(|n| matches!(n, Inline::Substituted { parts, .. }
        if matches!(&parts[0], P::OpenWithAttributes { role: Some(role), .. } if role == "id&#169;.role&#169;"))));
}

#[test]
fn ordinary_quotes_use_body_mode_while_pass_quotes_keep_the_header_cache() {
    let source = "= Doc\n\n:compat-mode:\n\n+Code+ 'Italic'\n\n:compat-mode!:\n\n'Literal' `Code`";
    let doc = parse(source).document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[1].kind else {
        panic!()
    };
    assert!(matches!(&inlines[0], Inline::Monospace { .. }));
    assert!(inlines.iter().any(|n| matches!(n, Inline::Emphasis { .. })));
    let BlockKind::Paragraph { inlines } = &doc.blocks[2].kind else {
        panic!()
    };
    assert_eq!(text(inlines), "'Literal' Code");
    assert!(!inlines.iter().any(|n| matches!(n, Inline::Emphasis { .. })));
    let doc = parse("= Doc\n:compat-mode:\n:v: pass:q[+Header+]\n\n:compat-mode!:\n:v: pass:q[+Body+]\n\n'Literal' {v}").document;
    assert_eq!(doc.attributes.get("v"), Some("+Header+"));
    let BlockKind::Paragraph { inlines } = &doc.blocks[1].kind else {
        panic!()
    };
    assert!(inlines.iter().any(|n| matches!(n, Inline::AttributeRef { parts, .. } if parts.contains(&P::Open(F::Monospace)))));
    assert_eq!(text(inlines), "'Literal' Body");
}

#[test]
fn host_values_and_expanded_attributes_do_not_restart_source_quotes() {
    let mut attributes = Attributes::new();
    attributes.set("compat-mode", "");
    attributes.set("role", "red");
    attributes.set("v", "*Literal*");
    let result = parse_with_options(
        "Before [{role}]+Code+ {v}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert_eq!(text(inlines), "Before Code *Literal*");
    assert!(inlines.iter().any(|n| matches!(n, Inline::Substituted { parts, .. }
        if matches!(&parts[0], P::OpenWithAttributes { format: F::Monospace, role: Some(role), .. } if role == "red"))));
}

#[test]
fn quotes_precede_attribute_name_recognition() {
    let nodes = paragraph("{*name*}");
    assert_eq!(text(&nodes), "{name}");
    assert!(nodes.iter().any(|n| matches!(n, Inline::Strong { .. })));
    assert!(!nodes
        .iter()
        .any(|n| matches!(n, Inline::AttributeRef { .. })));
}

#[test]
fn unicode_bom_crlf_and_every_partial_prefix_keep_valid_physical_spans() {
    let source = "\u{feff}:role: красный\r\n\r\nЖ(C) [{role}]\"`*Жёлтый*`\" \\_Literal_";
    let mut sources = SourceMap::new();
    let source_id = sources.add("quotes.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    struct Check<'a>(&'a str, adoctra_core::SourceId);
    impl adoctra_core::visit::VisitorMut for Check<'_> {
        fn visit_span(&mut self, span: &mut adoctra_core::Span) {
            assert_eq!(span.source_id, self.1);
            assert!(self.0.get(span.start..span.end).is_some(), "{span:?}");
        }
    }
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        let partial = &source[..end];
        let mut result = parse_with_options(partial, &options);
        adoctra_core::visit::walk_document_mut(
            &mut Check(partial, source_id),
            &mut result.document,
        );
        for token in tokens_with_options(partial, &options) {
            assert_eq!(token.span.source_id, source_id);
            assert!(partial.get(token.span.start..token.span.end).is_some());
        }
    }
}
