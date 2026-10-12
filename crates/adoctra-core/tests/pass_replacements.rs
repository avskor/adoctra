use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributePart, Attributes, BlockKind, Inline,
    ParseOptions, SourceMap, TokenKind,
};

fn raw(value: &str) -> AttributePart {
    AttributePart::Raw(value.into())
}

fn literal(value: &str) -> AttributePart {
    AttributePart::Text(value.into())
}

#[test]
fn replacement_entities_preserve_stage_order_and_unique_aliases() {
    for (subs, expected, parts) in [
        ("r", "&#169; ->", vec![raw("&#169; ->")]),
        ("replacements", "&#169; ->", vec![raw("&#169; ->")]),
        (
            "c,r",
            "&#169; &#8594;",
            vec![raw("&#169;"), literal(" "), raw("&#8594;")],
        ),
        ("r,c", "&#169; ->", vec![]),
        ("r,c,replacements", "&#169; ->", vec![]),
        (
            "c,r,specialcharacters",
            "&#169; &#8594;",
            vec![raw("&#169;"), literal(" "), raw("&#8594;")],
        ),
        ("none,r", "&#169; ->", vec![raw("&#169; ->")]),
    ] {
        let doc = parse(&format!(":v: pass:{subs}[(C) ->]\n\n{{v}} ")).document;
        assert_eq!(doc.attributes.get("v"), Some(expected), "{subs}");
        assert_eq!(
            doc.attributes.raw_parts("v").unwrap_or_default(),
            parts,
            "{subs}"
        );
    }
}

#[test]
fn attributes_before_replacements_share_a_stream_across_protection_boundaries() {
    let doc = parse(":left: pass:[(]\n:right: C) ->\n:v: pass:a,r[{left}{right}]\n\n{v}").document;
    assert_eq!(doc.attributes.get("v"), Some("&#169; &#8594;"));
    assert_eq!(
        doc.attributes.raw_parts("v").unwrap(),
        [raw("&#169;"), literal(" "), raw("&#8594;")]
    );
    assert_eq!(doc.attributes.get("right"), Some("C) ->"));
    let doc = parse(":left: pass:[(]\n:right: C) ->\n:v: pass:r,a[{left}{right}]\n\n{v}").document;
    assert_eq!(doc.attributes.get("v"), Some("(C) ->"));
    assert_eq!(
        doc.attributes.raw_parts("v").unwrap(),
        [raw("("), literal("C) ->")]
    );
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    assert!(matches!(&inlines[0], Inline::AttributeRef { value: Some(v), .. } if v == "© →"));
}

#[test]
fn escapes_and_entity_restoration_run_once_per_selected_stage() {
    for (body, expected) in [
        (r"\(C) \\(R) \... it\'s", r"(C) \(R) ... it's"),
        ("&amp;copy; &amp;amp;copy; &#65;", "&copy; &amp;copy; &#65;"),
        (r"\&amp;copy;", "&amp;copy;"),
        ("<=> -&gt; &lt;=", "<=> &#8594; &#8656;"),
        (
            "-- é--Ж e\u{301}--a",
            "&#8201;&#8212;&#8201;é&#8212;&#8203;Ж e\u{301}&#8212;&#8203;a",
        ),
    ] {
        let doc = parse(&format!(":v: pass:r,replacements[{body}]\n\n{{v}}")).document;
        assert_eq!(doc.attributes.get("v"), Some(expected), "{body}");
        assert_eq!(doc.attributes.raw_parts("v").unwrap(), [raw(expected)]);
    }
}

#[test]
fn multiline_entries_preserve_line_boundaries_until_replacements() {
    let source = ":v: pass:c,r[a + \\\n -- + \\\n b]\n\n{v}";
    let doc = parse(source).document;
    assert_eq!(
        doc.attributes.get("v"),
        Some("a +&#8201;&#8212;&#8201;+\nb")
    );
}

#[test]
fn body_assignments_keep_header_and_block_snapshots() {
    let source = ":v: pass:r[(C)]\n\n{v}\n\n:v: pass:c,r[->]\n\n{v}\n\n:v!:\n\nEnd";
    let doc = parse(source).document;
    assert_eq!(doc.attributes.get("v"), Some("&#169;"));
    assert_eq!(doc.blocks[0].attribute_context.get("v"), Some("&#169;"));
    assert_eq!(doc.blocks[1].attribute_context.get("v"), Some("&#8594;"));
    assert_eq!(doc.blocks[2].attribute_context.get("v"), None);
    for block in &doc.blocks[..2] {
        let BlockKind::Paragraph { inlines } = &block.kind else {
            panic!()
        };
        let Inline::AttributeRef { parts, span, .. } = &inlines[0] else {
            panic!()
        };
        assert!(parts.iter().all(AttributePart::is_raw));
        assert_eq!(&source[span.start..span.end], "{v}");
    }
}

#[test]
fn author_derivation_uses_replacement_spelling_and_keeps_declarations() {
    let source = "= Doc\nJohn Doe\n:author: pass:c,r[Анна--Мария O'Neil]\n\n{author}";
    let doc = parse(source).document;
    assert_eq!(
        doc.attributes.get("firstname"),
        Some("Анна&#8212;&#8203;Мария")
    );
    assert_eq!(doc.attributes.get("lastname"), Some("O&#8217;Neil"));
    assert_eq!(doc.attributes.get("authorinitials"), Some("АO"));
    let author = &doc.header.unwrap().authors[0];
    assert_eq!(author.name, "John Doe");
    assert_eq!(&source[author.span.start..author.span.end], "John Doe");
}

#[test]
fn host_values_are_already_substituted_and_locked_entries_are_consumed() {
    let mut attributes = Attributes::new();
    attributes.set("v", "pass:r[(C)]");
    let doc = parse_with_options(
        ":v: pass:r[(R)]\n\n{v}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    assert_eq!(doc.attributes.get("v"), Some("pass:r[(C)]"));
    assert_eq!(doc.attributes.raw_parts("v").unwrap(), [raw("pass:r[(C)]")]);
}

#[test]
fn unicode_crlf_spans_tokens_and_partial_entries_use_physical_source() {
    let source = "\u{feff}:v: pass:c,r[До (C) &copy;]\r\n\r\nЖ {v}";
    let mut sources = SourceMap::new();
    let source_id = sources.add("pass.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    let node = inlines
        .iter()
        .find(|n| matches!(n, Inline::AttributeRef { .. }))
        .unwrap();
    assert_eq!(sources.slice(node.span()), Some("{v}"));
    assert!(tokens_with_options(source, &options)
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == node.span()));
    for end in (0..=source.len()).filter(|&end| source.is_char_boundary(end)) {
        let partial = &source[..end];
        let _ = parse_with_options(partial, &options);
        for token in tokens_with_options(partial, &options) {
            assert!(token.span.end <= end);
            assert!(partial.is_char_boundary(token.span.start));
            assert!(partial.is_char_boundary(token.span.end));
        }
    }
}
