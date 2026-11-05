use adoctra_core::{
    parse, parse_with_options, tokens, AttributeFormat as F, AttributePart as P, Attributes,
    BlockKind, Inline, ParseOptions, Span, TokenKind,
};

fn inlines(source: &str) -> Vec<Inline> {
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let BlockKind::Paragraph { inlines } = result.document.blocks[0].kind.clone() else {
        panic!()
    };
    inlines
}

fn parts(nodes: &[Inline]) -> Vec<P> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            Inline::Text { value, .. } => out.push(P::Text(value.clone())),
            Inline::Raw { value, .. } => out.push(P::Raw(value.clone())),
            Inline::Substituted { parts, .. } => out.extend(parts.clone()),
            _ => panic!("{node:?}"),
        }
    }
    out
}

#[test]
fn protected_regions_keep_literal_parts_and_full_source_spans() {
    for marker in ["+", "++", "$$"] {
        let body = "*Ж* {v} &amp; <b> (C) https://example.org";
        let source = format!(":v: expanded\n\nBefore {marker}{body}{marker} after");
        let nodes = inlines(&source);
        let Inline::Substituted { value, parts, span } = &nodes[1] else {
            panic!("{nodes:?}")
        };
        assert_eq!(value, body);
        assert_eq!(parts, &[P::Text(body.into())]);
        assert_eq!(
            &source[span.start..span.end],
            format!("{marker}{body}{marker}")
        );
        let ts = tokens(&source);
        assert!(ts
            .iter()
            .any(|t| t.kind == TokenKind::PassthroughText && t.span == *span));
        assert!(!ts.iter().any(|t| matches!(
            t.kind,
            TokenKind::Strong | TokenKind::AttributeRef | TokenKind::Link
        )));
    }
}

#[test]
fn raw_passthrough_and_pass_stages_remain_distinct() {
    let nodes = inlines("Before +++<b>{v} *Ж*</b>+++ pass:[<i>x</i>] pass:c[<b>x</b>] after");
    let ps = parts(&nodes);
    assert!(ps.contains(&P::Raw("<b>{v} *Ж*</b>".into())));
    assert!(ps.contains(&P::Raw("<i>x</i>".into())));
    assert!(ps.contains(&P::Text("<b>x</b>".into())));
    let nodes = inlines(":v: Ж\n\nBefore pass:c,q,a,r[*{v}* (C)] after");
    let ps = parts(&nodes);
    assert!(ps.contains(&P::Open(F::Strong)));
    assert!(ps.contains(&P::Text("Ж".into())));
    assert!(ps.contains(&P::Raw("&#169;".into())));
}

#[test]
fn metadata_and_legacy_forms_restore_neutral_wrappers() {
    let source = ":role: красный\n\nBefore [#id.{role}]+*Ж* {v}+ [x-]+*Bold*+ after";
    let nodes = inlines(source);
    let ps = parts(&nodes);
    assert!(ps.contains(&P::OpenWithAttributes {
        format: F::Span,
        id: Some("id".into()),
        role: Some("красный".into())
    }));
    assert!(ps.contains(&P::Text("*Ж* {v}".into())));
    assert!(ps.contains(&P::Open(F::Monospace)));
    assert!(ps.contains(&P::Open(F::Strong)));
    let nodes = inlines(r"Before \[x-]+*Bold*+ after");
    assert!(!parts(&nodes).contains(&P::Open(F::Monospace)));
    assert!(parts(&nodes).contains(&P::Text("*Bold*".into())));
}

#[test]
fn outer_quotes_and_links_can_consume_protected_regions() {
    let source = "Ж(C) *before +_Literal_ {v}+ after* link:++page?a=1&b=2++[+A [x] & {v}+]";
    let nodes = inlines(source);
    let Inline::Strong { children, span } = &nodes[1] else {
        panic!("{nodes:?}")
    };
    assert_eq!(
        &source[span.start..span.end],
        "*before +_Literal_ {v}+ after*"
    );
    assert!(parts(children).contains(&P::Text("_Literal_ {v}".into())));
    let (ps, span) = nodes
        .iter()
        .find_map(|n| match n {
            Inline::Substituted { parts, span, .. }
                if matches!(parts.first(), Some(P::OpenLink(_))) =>
            {
                Some((parts, span))
            }
            _ => None,
        })
        .unwrap();
    assert!(matches!(&ps[0], P::OpenLink(l) if l.target == "page?a=1&amp;b=2"));
    assert!(ps.contains(&P::Text("A [x] & {v}".into())));
    assert_eq!(
        &source[span.start..span.end],
        "link:++page?a=1&b=2++[+A [x] & {v}+]"
    );
}

#[test]
fn escaped_delimiters_leave_contents_for_later_stages() {
    let nodes = inlines(":v: Ж\n\nBefore \\+*Bold* {v} (C)+ after");
    assert!(nodes.iter().any(|n| matches!(n, Inline::Strong { .. })));
    assert!(nodes
        .iter()
        .any(|n| matches!(n, Inline::AttributeRef { value: Some(v), .. } if v == "Ж")));
    assert!(!tokens("Before \\+*Bold*+ after")
        .iter()
        .any(|t| t.kind == TokenKind::PassthroughText));
    let nodes = inlines(r"Before pass:[one\]two] after");
    assert!(parts(&nodes).contains(&P::Raw("one]two".into())));
}

#[test]
fn nested_slots_restore_once_and_expanded_attributes_are_not_reextracted() {
    let nodes = inlines("Before +one pass:[<b>two</b>] three+ after");
    let ps = parts(&nodes);
    assert!(ps.contains(&P::Text("one ".into())));
    assert!(ps.contains(&P::Raw("<b>two</b>".into())));
    assert!(ps.contains(&P::Text(" three".into())));
    let nodes = inlines(":v: +literal+\n\nBefore {v} after");
    assert!(nodes
        .iter()
        .any(|n| matches!(n, Inline::AttributeRef { value: Some(v), .. } if v == "+literal+")));
}

#[test]
fn compat_mode_replays_body_changes_and_preserves_backticks_literally() {
    let doc = parse("= Doc\n\n:compat-mode:\n\nBefore `*Bold* {v} (C)` after\n\n:compat-mode!:\n\nBefore +*Bold* {v} (C)+ after").document;
    for (index, mono) in [(1, true), (2, false)] {
        let BlockKind::Paragraph { inlines } = &doc.blocks[index].kind else {
            panic!()
        };
        let ps = parts(inlines);
        assert_eq!(ps.contains(&P::Open(F::Monospace)), mono);
        assert!(ps.contains(&P::Text("*Bold* {v} (C)".into())));
        assert!(!ps.contains(&P::Open(F::Strong)));
    }
    assert!(!tokens(":compat-mode:\n\nж\\`*Bold*`")
        .iter()
        .any(|t| t.kind == TokenKind::Monospace));
}

#[test]
fn source_and_host_slot_like_text_cannot_collide_with_private_placeholders() {
    let literal = "\u{96}0\u{97}";
    let mut attributes = Attributes::new();
    attributes.set("v", literal);
    let source = format!("Before {literal} +protected+ {{v}} after");
    let result = parse_with_options(
        &source,
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert!(inlines
        .iter()
        .any(|n| matches!(n, Inline::Text { value, .. } if value.contains(literal))));
    assert!(inlines
        .iter()
        .any(|n| matches!(n, Inline::AttributeRef { value: Some(value), .. } if value == literal)));
    assert!(inlines
        .iter()
        .any(|n| matches!(n, Inline::Substituted { value, .. } if value == "protected")));
}

#[test]
fn slot_like_attribute_values_alone_lengthen_the_private_prefix() {
    let literal = "\u{96}0\u{97}";
    let mut host = Attributes::new();
    host.set("v", literal);
    let entry = format!(":v: {literal}\n\n+protected+ {{v}} after");
    for (attributes, source) in [
        (host, "+protected+ {v} after"),
        (Attributes::new(), entry.as_str()),
    ] {
        let result = parse_with_options(
            source,
            &ParseOptions {
                attributes,
                ..Default::default()
            },
        );
        let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
            panic!()
        };
        assert!(
            inlines.iter().any(
                |n| matches!(n, Inline::AttributeRef { value: Some(value), .. } if value == literal)
            ),
            "{inlines:?}"
        );
        assert!(inlines
            .iter()
            .any(|n| matches!(n, Inline::Substituted { value, .. } if value == "protected")));
    }
}

#[test]
fn unicode_crlf_and_partial_input_keep_valid_byte_ranges() {
    let source =
        "\u{feff}:role: красный\r\n\r\nЖ(C) link:page[[{role}]+Ж& {v}+]; pass:c[one\\]two]";
    struct Check<'a>(&'a str);
    impl adoctra_core::visit::VisitorMut for Check<'_> {
        fn visit_span(&mut self, span: &mut Span) {
            assert!(self.0.get(span.start..span.end).is_some(), "{span:?}");
        }
    }
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        let source = &source[..end];
        let mut doc = parse(source).document;
        adoctra_core::visit::walk_document_mut(&mut Check(source), &mut doc);
        for token in tokens(source) {
            assert!(source.get(token.span.start..token.span.end).is_some());
        }
    }
}

#[test]
fn long_unfinished_delimiters_remain_tolerant() {
    for body in ["+a ".repeat(4000), "pass:".repeat(4000), "[".repeat(4000)] {
        let source = format!("Before {body}");
        let result = parse(&source);
        assert!(result.diagnostics.is_empty());
        assert!(!result.document.blocks.is_empty());
    }
}

#[test]
fn protected_closing_markers_do_not_end_explicit_xref_labels() {
    for (source, expected) in [
        ("Before xref:id[+a]b+] after", "a]b"),
        ("Before <<id,+a>>b+>> after", "a>>b"),
    ] {
        let result = parse(source);
        let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
            panic!()
        };
        let Inline::CrossReference { children, span, .. } = &inlines[1] else {
            panic!("{inlines:?}")
        };
        assert_eq!(&source[span.start..span.end], &source[7..source.len() - 6]);
        assert_eq!(parts(children), vec![P::Text(expected.into())]);
        assert!(matches!(inlines.last(), Some(Inline::Text { value, .. }) if value == " after"));
    }
}

#[test]
fn mailto_subject_encoding_preserves_the_pinned_reference_behavior() {
    let nodes = inlines("Before mailto:a@example.org[Mail,+A & B+,+Body & C+] after");
    let ps = parts(&nodes);
    assert!(ps.iter().any(|p| matches!(p, P::OpenLink(l)
        if l.target == "mailto:a@example.org?subject=%C2%960%C2%97&amp;body=%C2%961%C2%97")));
    // Ruby percent-encodes the slots before restoration. Preserve this pinned
    // behavior explicitly; ordinary link labels and metadata restore normally.
    assert!(!ps.iter().any(|p| p.text().contains('\u{96}')));
}
