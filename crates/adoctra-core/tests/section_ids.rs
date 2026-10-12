use adoctra_core::{
    parse, parse_with_options, tokens_with_options, Attributes, BlockKind, Inline, ParseOptions,
    SourceMap, TokenKind,
};

#[test]
fn section_ids_remove_raw_entities_but_keep_protected_spellings() {
    for (value, expected) in [
        ("pass:c,r[(C) ->]", ""),
        ("pass:[A &copy; &#65; &#x41; B]", "_a_b"),
        ("pass:[<b>Alpha</b> &copy; Beta]", "_alpha_beta"),
        ("pass:c[<b>Alpha</b> &copy;]", "_balphab"),
        ("pass:[&amp;copy;]", ""),
        (
            "pass:[&#1; &#1234567; &#x1; &#x123456; &a; &ab123;]",
            "_1_1234567_x1_x123456_a_ab123",
        ),
    ] {
        let source = format!(":v: {value}\n\n== {{v}}");
        let result = parse(&source);
        assert_eq!(
            result.document.blocks[0].id.as_deref(),
            Some(expected),
            "{value}"
        );
        let BlockKind::Heading { inlines, .. } = &result.document.blocks[0].kind else {
            panic!()
        };
        let Inline::AttributeRef { span, .. } = &inlines[0] else {
            panic!()
        };
        assert_eq!(&source[span.start..span.end], "{v}");
    }
}

#[test]
fn malformed_tags_and_entity_grammar_follow_the_reference_without_decoding() {
    for (value, expected) in [
        ("<>", ""),
        ("<b", "_b"),
        ("<<b>X</b>", "_x"),
        ("A <b &gt; B", "_a_b_b"),
        ("A <b>Ж</b> &amp;amp;copy;", "_a_ж_copy"),
        ("&AB12; &#X41; &#999999; &#xFFFFF;", ""),
        ("&a; &abc123; &#0; &#x1;", "_a_abc123_0_x1"),
    ] {
        let result = parse(&format!(":v: pass:[{value}]\n\n== {{v}}"));
        assert_eq!(
            result.document.blocks[0].id.as_deref(),
            Some(expected),
            "{value}"
        );
    }
    let source = format!(":v: pass:[{}A]\n\n== {{v}}", "<".repeat(20_000));
    assert_eq!(parse(&source).document.blocks[0].id.as_deref(), Some("_a"));
}

#[test]
fn sanitized_ids_deduplicate_and_forward_references_keep_original_title_nodes() {
    let source = ":v: pass:[<b>Alpha</b> &copy;]\n\n<<_alpha>> <<_alpha_2>>\n\n== {v}\n\n== Alpha";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert_eq!(blocks[1].id.as_deref(), Some("_alpha"));
    assert_eq!(blocks[2].id.as_deref(), Some("_alpha_2"));
    let BlockKind::Paragraph { inlines } = &blocks[0].kind else {
        panic!()
    };
    let Inline::CrossReference {
        target,
        children,
        span,
    } = &inlines[0]
    else {
        panic!()
    };
    assert_eq!(target, "_alpha");
    assert_eq!(&source[span.start..span.end], "<<_alpha>>");
    let Inline::AttributeRef {
        value, parts, span, ..
    } = &children[0]
    else {
        panic!()
    };
    assert_eq!(value.as_deref(), Some("<b>Alpha</b> &copy;"));
    assert_eq!(parts, result.document.attributes.raw_parts("v").unwrap());
    assert_eq!(&source[span.start..span.end], "{v}");
}

#[test]
fn formatting_and_anchor_boundaries_do_not_join_a_new_entity_for_id_generation() {
    for title in ["{v}*py*;", "{v}link:target[py];", "{v}[[point]]py;"] {
        let source = format!(":v: pass:[&co]\n\n== {title}");
        assert_eq!(
            parse(&source).document.blocks[0].id.as_deref(),
            Some("_copy"),
            "{title}"
        );
    }
}

#[test]
fn changed_attribute_snapshots_and_host_raw_values_determine_each_id() {
    let source = ":v: pass:[Alpha &copy;]\n\n== {v}\n\n:v: pass:[Beta &#65;]\n\n== {v}";
    let result = parse(source);
    assert_eq!(result.document.attributes.get("v"), Some("Alpha &copy;"));
    for (block, value, id) in [
        (&result.document.blocks[0], "Alpha &copy;", "_alpha"),
        (&result.document.blocks[1], "Beta &#65;", "_beta"),
    ] {
        assert_eq!(block.id.as_deref(), Some(id));
        assert_eq!(block.attribute_context.get("v"), Some(value));
    }
    let mut attributes = Attributes::new();
    attributes.set("v", "<b>Host</b> &#65;");
    let result = parse_with_options(
        ":v: ignored\n\n== {v}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    assert_eq!(result.document.blocks[0].id.as_deref(), Some("_host"));
    assert_eq!(
        result.document.attributes.get("v"),
        Some("<b>Host</b> &#65;")
    );
}

#[test]
fn unicode_crlf_and_partial_input_keep_physical_spans_and_tokens() {
    let source = "\u{feff}:v: pass:[<b>Жёлтый</b> &#65; café]\r\n\r\n== {v}";
    let mut sources = SourceMap::new();
    let source_id = sources.add("section.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    let block = &result.document.blocks[0];
    assert_eq!(block.id.as_deref(), Some("_жёлтый_café"));
    assert_eq!(sources.slice(block.span), Some("== {v}"));
    let BlockKind::Heading { inlines, .. } = &block.kind else {
        panic!()
    };
    assert_eq!(sources.slice(inlines[0].span()), Some("{v}"));
    assert!(tokens_with_options(source, &options)
        .iter()
        .any(|t| t.kind == TokenKind::Heading && t.span == block.span));
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
