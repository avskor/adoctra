use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributeFormat as F, AttributePart as P,
    Attributes, BlockKind, Inline, ParseOptions, SourceMap, TokenKind,
};

fn parts(source: &str) -> Vec<P> {
    parse(source)
        .document
        .attributes
        .raw_parts("v")
        .unwrap_or_default()
        .to_vec()
}

#[test]
fn quotes_produce_semantic_boundaries_and_flat_text_without_backend_tags() {
    for (marker, format) in [
        ("*", F::Strong),
        ("_", F::Emphasis),
        ("`", F::Monospace),
        ("#", F::Mark),
        ("^", F::Superscript),
        ("~", F::Subscript),
    ] {
        let source = format!(":v: pass:c,q[{marker}Ж&{marker}]\n\n{{v}}");
        let doc = parse(&source).document;
        assert_eq!(doc.attributes.get("v"), Some("Ж&"));
        let expected = [P::Open(format), P::Text("Ж&".into()), P::Close(format)];
        assert_eq!(doc.attributes.raw_parts("v").unwrap(), expected);
        let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
            panic!()
        };
        assert!(
            matches!(&inlines[0], Inline::AttributeRef { parts, value: Some(value), span, .. }
            if parts == &expected && value == "Ж&" && &source[span.start..span.end] == "{v}")
        );
    }
}

#[test]
fn protection_after_quotes_turns_generated_markup_into_literal_text() {
    let doc = parse(":v: pass:q,c[*A & B*]\n\n{v}").document;
    assert_eq!(doc.attributes.get("v"), Some("<strong>A & B</strong>"));
    assert!(doc.attributes.raw_parts("v").is_none());
    assert_eq!(
        parts(":v: pass:c,q,specialcharacters[*A*]"),
        [P::Open(F::Strong), P::Text("A".into()), P::Close(F::Strong)]
    );
}

#[test]
fn attribute_expansion_preserves_structure_and_selected_order() {
    assert_eq!(
        parts(":name: _Ada_\n:v: pass:a,q[*{name}*]"),
        [
            P::Open(F::Strong),
            P::Open(F::Emphasis),
            P::Text("Ada".into()),
            P::Close(F::Emphasis),
            P::Close(F::Strong)
        ]
    );
    assert_eq!(
        parts(":name: _Ada_\n:v: pass:q,a[*{name}*]"),
        [
            P::Open(F::Strong),
            P::Text("_Ada_".into()),
            P::Close(F::Strong)
        ]
    );
    let source = ":name: pass:q[*Ada*]\n:v: {name}";
    assert_eq!(
        parts(source),
        [
            P::Open(F::Strong),
            P::Raw("Ada".into()),
            P::Close(F::Strong)
        ]
    );
    assert_eq!(
        parts(":v: pass:q,quotes[\\*Ada*]"),
        [P::Raw("*Ada*".into())]
    );
}

#[test]
fn replacements_preserve_formatting_and_cannot_join_across_boundaries() {
    let source = ":v: pass:c,q,r[*(C) &copy;*]\n\n{v}";
    assert_eq!(
        parts(source),
        [
            P::Open(F::Strong),
            P::Raw("&#169;".into()),
            P::Text(" ".into()),
            P::Raw("&copy;".into()),
            P::Close(F::Strong)
        ]
    );
    let doc = parse(":v: pass:q[**C**]\n\n({v})").document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    assert!(
        matches!(&inlines[1], Inline::AttributeRef { value: Some(value), parts, .. }
        if value == "C" && parts == &[P::Open(F::Strong), P::Raw("C".into()), P::Close(F::Strong)])
    );
    assert_eq!(
        parts(":v: pass:q,r[&co**py**;]"),
        [
            P::Raw("&co".into()),
            P::Open(F::Strong),
            P::Raw("py".into()),
            P::Close(F::Strong),
            P::Raw(";".into())
        ]
    );
}

#[test]
fn author_derivation_preserves_formatting_and_original_declarations() {
    let source =
        "= Doc\nJohn Doe; Mary Roe\n:author_2: pass:c,q[*Анна_Мария* _Смирнова_]\n\n{authors}";
    let doc = parse(source).document;
    assert_eq!(doc.attributes.get("firstname_2"), Some("Анна Мария"));
    assert_eq!(doc.attributes.get("lastname_2"), Some("Смирнова"));
    assert_eq!(doc.attributes.get("authorinitials_2"), Some("АС"));
    assert!(doc
        .attributes
        .raw_parts("authors")
        .unwrap()
        .contains(&P::Open(F::Strong)));
    assert_eq!(doc.header.unwrap().authors[1].name, "Mary Roe");
    let doc =
        parse("= Doc\n:authors: pass:q[*Ada* Lovelace; _Grace_ Hopper]\n\n{authors}").document;
    assert_eq!(doc.attributes.get("authorcount"), Some("2"));
    assert!(doc
        .attributes
        .raw_parts("author_2")
        .unwrap()
        .contains(&P::Open(F::Emphasis)));
}

#[test]
fn snapshots_host_parts_and_forward_references_keep_boundaries() {
    let source = ":v: pass:q[*Alpha*]\n\n<<_alpha>>\n\n== {v}\n\n:v: pass:q[_Beta_]\n\n{v}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    let doc = result.document;
    assert_eq!(doc.blocks[1].id.as_deref(), Some("_alpha"));
    assert_eq!(doc.attributes.get("v"), Some("Alpha"));
    assert_eq!(doc.blocks[2].attribute_context.get("v"), Some("Beta"));
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    let Inline::CrossReference { children, .. } = &inlines[0] else {
        panic!()
    };
    assert!(
        matches!(&children[0], Inline::AttributeRef { parts, .. } if parts.contains(&P::Open(F::Strong)))
    );
    let mut attributes = Attributes::new();
    attributes.set_parts(
        "v",
        vec![P::Open(F::Mark), P::Text("Host".into()), P::Close(F::Mark)],
    );
    let doc = parse_with_options(
        ":v: ignored\n\n{v}",
        &ParseOptions {
            attributes: attributes.clone(),
            ..Default::default()
        },
    )
    .document;
    assert_eq!(doc.attributes.raw_parts("v"), attributes.raw_parts("v"));
}

#[test]
fn unicode_crlf_tokens_and_partial_values_keep_physical_reference_spans() {
    let source = "\u{feff}:v: pass:c,q[*Ж&* _é_]\r\n\r\nДо {v}";
    let mut sources = SourceMap::new();
    let source_id = sources.add("quotes.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let doc = parse_with_options(source, &options).document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    let span = inlines[1].span();
    assert_eq!(sources.slice(span), Some("{v}"));
    assert!(tokens_with_options(source, &options)
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == span));
    for end in (0..=source.len()).filter(|&end| source.is_char_boundary(end)) {
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
fn compat_quotes_use_the_cached_mode_after_header_completion() {
    let doc =
        parse(":compat-mode:\n:v: pass:q[+Header+]\n\n{v}\n\n:v: pass:q[+Body+]\n\n{v}").document;
    assert_eq!(doc.attributes.get("v"), Some("+Header+"));
    assert_eq!(doc.blocks[1].attribute_context.get("v"), Some("Body"));
    assert_eq!(
        doc.blocks[1].attribute_context.raw_parts("v").unwrap()[0],
        P::Open(F::Monospace)
    );
    let mut attributes = Attributes::new();
    attributes.set("compat-mode", "");
    let doc = parse_with_options(
        ":v: pass:q[+Host+]\n\n{v}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    assert_eq!(
        doc.attributes.raw_parts("v").unwrap()[0],
        P::Open(F::Monospace)
    );
}

#[test]
fn incomplete_marker_runs_do_not_recurse_or_rescan_each_suffix() {
    let value = "*a ".repeat(20_000);
    let doc = parse(&format!(":v: pass:q[{value}]\n\n{{v}}")).document;
    assert_eq!(doc.attributes.get("v"), Some(value.as_str()));
}
