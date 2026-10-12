use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributePart, Attributes, BlockKind, Inline,
    ParseOptions, SourceMap, TokenKind,
};

fn reference(block: &adoctra_core::Block) -> (&str, &[AttributePart], adoctra_core::Span) {
    let inlines = match &block.kind {
        BlockKind::Paragraph { inlines } | BlockKind::Heading { inlines, .. } => inlines,
        _ => panic!("expected prose"),
    };
    match &inlines[0] {
        Inline::AttributeRef {
            value: Some(value),
            parts,
            span,
            ..
        } => (value, parts, *span),
        _ => panic!("expected resolved reference"),
    }
}

#[test]
fn named_and_numeric_entity_grammar_matches_the_reference() {
    for entity in [
        "&copy;",
        "&unknown;",
        "&Abc12;",
        "&#65;",
        "&#000065;",
        "&#x41;",
        "&#x1F642;",
    ] {
        let doc = parse(&format!(":v: {entity}\n\n{{v}}")).document;
        let (value, parts, _) = reference(&doc.blocks[0]);
        assert_eq!(value, entity);
        assert_eq!(parts, &[AttributePart::Raw(entity.to_owned())]);
        assert_eq!(doc.attributes.get("v"), Some(entity));
        assert!(doc.attributes.raw_parts("v").is_none());
    }
    for entity in [
        "&a;",
        "&a1;",
        "&ab123;",
        "&#1;",
        "&#1234567;",
        "&#x1;",
        "&#X41;",
        "&#x123456;",
        "&copy",
        "& copy;",
        "&копи;",
    ] {
        let doc = parse(&format!(":v: {entity}\n\n{{v}}")).document;
        let (value, parts, _) = reference(&doc.blocks[0]);
        assert_eq!(value, entity);
        assert!(parts.is_empty(), "{entity}");
    }
}

#[test]
fn entity_escapes_and_nested_protection_are_processed_once() {
    for (input, expected, raw) in [
        (r"\&copy;", "&copy;", false),
        (r"\\&copy;", r"\&copy;", false),
        (r"\&a;", r"\&a;", false),
        ("&amp;copy;", "&amp;copy;", true),
        ("&amp;amp;copy;", "&amp;amp;copy;", true),
    ] {
        let doc = parse(&format!(":v: {input}\n\n{{v}}")).document;
        let (value, parts, _) = reference(&doc.blocks[0]);
        assert_eq!(value, expected, "{input}");
        assert_eq!(parts.iter().any(AttributePart::is_raw), raw, "{input}");
        assert_eq!(doc.attributes.get("v"), Some(input));
        if input == "&amp;copy;" {
            assert_eq!(
                parts,
                &[
                    AttributePart::Raw("&amp;".to_owned()),
                    AttributePart::Text("copy;".to_owned())
                ]
            );
        }
    }
    let doc = parse(":v: pass:[\\&amp;copy; / &amp;amp;copy;]\n\n{v}").document;
    assert_eq!(reference(&doc.blocks[0]).0, "&amp;copy; / &amp;copy;");
    assert_eq!(
        doc.attributes.get("v"),
        Some("\\&amp;copy; / &amp;amp;copy;")
    );
}

#[test]
fn restoration_crosses_literal_raw_boundaries_without_changing_markup() {
    let mut attributes = Attributes::new();
    attributes.set_parts(
        "v",
        vec![
            AttributePart::Text("До < ".to_owned()),
            AttributePart::Raw("<b>&am".to_owned()),
            AttributePart::Text("p;copy;".to_owned()),
            AttributePart::Raw("</b>".to_owned()),
            AttributePart::Text(" > после".to_owned()),
        ],
    );
    let options = ParseOptions {
        attributes,
        ..Default::default()
    };
    let doc = parse_with_options("{v}", &options).document;
    let (value, parts, _) = reference(&doc.blocks[0]);
    assert_eq!(value, "До < <b>&copy;</b> > после");
    assert_eq!(
        parts,
        &[
            AttributePart::Text("До < ".to_owned()),
            AttributePart::Raw("<b>&copy;</b>".to_owned()),
            AttributePart::Text(" > после".to_owned()),
        ]
    );
    assert_eq!(
        doc.attributes.raw_parts("v"),
        options.attributes.raw_parts("v")
    );
    // The escape can be in a different part from the protected ampersand.
    let doc = parse(":slash: pass:[\\]\n:v: {slash}&copy;\n\n{v}").document;
    assert_eq!(reference(&doc.blocks[0]).0, "&copy;");
    assert!(reference(&doc.blocks[0]).1.is_empty());
}

#[test]
fn restored_references_keep_unicode_source_positions_and_attribute_snapshots() {
    let source = "\u{feff}:v: До &copy; после\r\n\r\n{v}\r\n\r\n:v: \\&copy;\r\n\r\n{v}";
    let mut sources = SourceMap::new();
    sources.add("unused", "");
    let source_id = sources.add("doc.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert!(result.diagnostics.is_empty());
    let doc = &result.document;
    assert_eq!(doc.attributes.get("v"), Some("До &copy; после"));
    assert_eq!(
        doc.blocks[0].attribute_context.get("v"),
        Some("До &copy; после")
    );
    assert_eq!(doc.blocks[1].attribute_context.get("v"), Some("\\&copy;"));
    assert_eq!(reference(&doc.blocks[1]).0, "&copy;");
    for block in &doc.blocks {
        let span = reference(block).2;
        assert_eq!(span.source_id, source_id);
        assert_eq!(sources.slice(span), Some("{v}"));
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
fn verse_applies_restoration_but_verbatim_keeps_the_original_reference() {
    let doc = parse(":v: &copy;\n\n[verse]\n____\n{v}\n____\n\n----\n{v}\n----").document;
    let BlockKind::Verse { inlines, .. } = &doc.blocks[0].kind else {
        panic!()
    };
    assert!(
        matches!(&inlines[0], Inline::AttributeRef { parts, .. } if parts == &[AttributePart::Raw("&copy;".to_owned())])
    );
    let BlockKind::Listing { text, .. } = &doc.blocks[1].kind else {
        panic!()
    };
    assert_eq!(text, "{v}");
}

#[test]
fn incomplete_entities_and_partial_unicode_input_remain_safe() {
    let source = ":raw: pass:[<b>]\r\n:v: {raw}До &#x1F642; \\&copy;\r\n\r\n{v}";
    for end in (0..=source.len()).filter(|&end| source.is_char_boundary(end)) {
        let partial = &source[..end];
        let result = parse(partial);
        for span in adoctra_core::tokens(partial)
            .into_iter()
            .map(|t| t.span)
            .chain(result.diagnostics.iter().map(|d| d.span))
        {
            assert!(
                partial.get(span.start..span.end).is_some(),
                "{partial:?}: {span:?}"
            );
        }
    }
    for value in ["&", "&amp", "&amp;", "&amp;#x", "\\", "\\&amp;cop"] {
        let mut attributes = Attributes::new();
        attributes.set("v", value);
        let doc = parse_with_options(
            "{v}",
            &ParseOptions {
                attributes,
                ..Default::default()
            },
        )
        .document;
        assert_eq!(reference(&doc.blocks[0]).0, value);
    }
}
