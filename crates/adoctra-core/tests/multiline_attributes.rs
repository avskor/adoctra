use adoctra_core::{
    parse, parse_with_options, tokens, tokens_with_options, BlockKind, Inline, ParseOptions,
    SourceId, Span, TokenKind,
};

#[test]
fn continuation_markers_whitespace_and_incomplete_values() {
    for (source, expected) in [
        (":x: one \\\n  two \\\n\tthree", "one two three"),
        (":x: one +\n  two +\n three", "one two three"),
        (":x: one \\\n two +", "one two +"),
        (":x: one +\n two \\", "one two \\"),
        (":x: one + \\\n two", "one +\ntwo"),
        (":x: one \\\n\n", "one"),
        (":x: one \\", "one"),
        (":x: one \\\n two \\", "one two"),
        (":x: one\\", "one\\"),
        (":x: \\", "\\"),
        (":x: one \\\n\u{a0}two", "one \u{a0}two"),
        (":x: one \\\n \\\n", "one \\"),
        (":x: one \\\n// text", "one // text"),
        (":x: one \\\n:y: text", "one :y: text"),
    ] {
        let result = parse(source);
        assert_eq!(
            result.document.attributes.get("x"),
            Some(expected),
            "{source:?}"
        );
        assert!(result.document.blocks.is_empty(), "{result:?}");
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert!(tokens(source)
            .iter()
            .all(|t| t.kind == TokenKind::AttributeEntry));
    }
}

#[test]
fn header_body_snapshots_substitutions_and_locked_unsets() {
    let source = ":x: old\n\nBefore {x}.\n\n:x: {x} \\\n again\n\nAfter {x}.\n\n:x!: ignored \\\n swallowed\n\nFinal {x}.";
    let result = parse(source);
    assert_eq!(result.document.attributes.get("x"), Some("old"));
    assert_eq!(result.document.blocks.len(), 3);
    for (block, expected) in
        result
            .document
            .flat_blocks()
            .iter()
            .zip([Some("old"), Some("old again"), None])
    {
        assert_eq!(block.attribute_context.get("x"), expected);
        let BlockKind::Paragraph { inlines } = &block.kind else {
            panic!("{block:?}")
        };
        let reference = inlines
            .iter()
            .find(|i| matches!(i, Inline::AttributeRef { .. }))
            .unwrap();
        let Inline::AttributeRef { value, span, .. } = reference else {
            unreachable!()
        };
        assert_eq!(value.as_deref(), expected);
        assert_eq!(&source[span.start..span.end], "{x}");
    }
    let mut options = ParseOptions::default();
    options.attributes.set("x", "locked");
    let locked = parse_with_options(source, &options);
    assert_eq!(locked.document.blocks.len(), 3);
    assert!(locked
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.attribute_context.get("x") == Some("locked")));
    assert_eq!(tokens(source), tokens_with_options(source, &options));
}

#[test]
fn header_metadata_and_unicode_crlf_spans() {
    let source = "\u{feff}= Title\r\n:x: Я \\ \t\r\n  🙂\r\nJane Doe\r\n:y: one +\r\n two\r\nv1.0\r\n\r\n{x} / {y} / <<missing>>";
    let options = ParseOptions {
        source_id: SourceId(7),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert_eq!(result.document.attributes.get("x"), Some("Я 🙂"));
    assert_eq!(result.document.attributes.get("y"), Some("one two"));
    let header = result.document.header.as_ref().unwrap();
    assert_eq!(header.authors[0].name, "Jane Doe");
    assert_eq!(
        header.revision.as_ref().unwrap().number.as_deref(),
        Some("1.0")
    );
    let highlighted = tokens_with_options(source, &options);
    for (text, kind) in [
        (":x: Я \\", TokenKind::AttributeEntry),
        ("  🙂", TokenKind::AttributeEntry),
        (":y: one +", TokenKind::AttributeEntry),
        (" two", TokenKind::AttributeEntry),
        ("Jane Doe", TokenKind::Author),
        ("v1.0", TokenKind::Revision),
        ("{x}", TokenKind::AttributeRef),
        ("<<missing>>", TokenKind::CrossReference),
    ] {
        let start = source.find(text).unwrap();
        let span = Span::in_source(options.source_id, start, start + text.len());
        assert!(
            highlighted.iter().any(|t| t.kind == kind && t.span == span),
            "{text:?}: {highlighted:?}"
        );
        if text == "<<missing>>" {
            assert_eq!(result.diagnostics[0].span, span);
        }
    }
}

#[test]
fn preprocessing_uses_old_context_and_tokens_follow_parser_boundaries() {
    let source =
        ":x: old\n:x: new \\\nifeval::[\"{x}\" == \"old\"]\n two \\\nendif::[]\n three\n\n{x}";
    let result = parse(source);
    assert_eq!(result.document.attributes.get("x"), Some("new two three"));
    assert_eq!(result.conditional_directives.len(), 2);
    let highlighted = tokens(source);
    assert_eq!(
        highlighted
            .iter()
            .filter(|t| t.kind == TokenKind::AttributeEntry)
            .count(),
        4
    );
    for (source, count) in [
        ("====\n:x: one \\\n====\n\n{x}", 1),
        ("----\n:x: one \\\n two\n----", 0),
        ("....\n:x: one \\\n two\n....", 0),
        ("++++\n:x: one \\\n two\n++++", 0),
        ("Text\n:x: one \\\ncontinuation", 0),
        ("* Item\n:x: one \\\ncontinuation", 0),
        ("////\n:x: one \\\n two\n////", 0),
        ("ifdef::missing[]\n:x: one \\\n two\nendif::[]", 0),
        ("* Item\n+\n--\n:x: one \\\n two\n\n{x}\n--", 2),
    ] {
        assert_eq!(
            tokens(source)
                .iter()
                .filter(|t| t.kind == TokenKind::AttributeEntry)
                .count(),
            count,
            "{source}"
        );
    }
}

#[test]
fn partial_unicode_inputs_and_long_continuations_are_safe() {
    let source = "\u{feff}= Я\r\n:x: Я \\ \t\r\n 🙂 \\\nifdef::x[]\n more\nendif::[]\n\n{x}";
    for end in (0..=source.len()).filter(|&end| source.is_char_boundary(end)) {
        let partial = &source[..end];
        let result = parse(partial);
        for span in tokens(partial)
            .into_iter()
            .map(|t| t.span)
            .chain(result.diagnostics.iter().map(|d| d.span))
            .chain(result.provenance.iter().flatten().copied())
        {
            assert!(
                partial.get(span.start..span.end).is_some(),
                "{partial:?}: {span:?}"
            );
        }
    }
    let source = format!(":x: start \\\n{}end", " next \\\n".repeat(10_000));
    let result = parse(&source);
    assert_eq!(result.document.attributes.get("x").unwrap().len(), 50_009);
    assert_eq!(tokens(&source).len(), 10_002);
}

#[test]
fn unicode_suffix_is_significant_for_continuation_detection() {
    let source = ":x: one \\\u{a0}\n\n{x}";
    let result = parse(source);
    assert_eq!(result.document.attributes.get("x"), Some("one \\\u{a0}"));
    assert_eq!(result.document.blocks.len(), 1);
}
