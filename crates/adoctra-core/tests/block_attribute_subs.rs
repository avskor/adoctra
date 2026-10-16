use adoctra_core::{
    ast::Attribution, parse, parse_with_options, tokens_with_options, Attributes, Block, BlockKind,
    Inline, ParseOptions, Severity, SourceId, TokenKind,
};

fn text(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Inline::Text { value, .. }
            | Inline::Raw { value, .. }
            | Inline::Substituted { value, .. } => value.clone(),
            Inline::AttributeRef { name, value, .. } => {
                value.clone().unwrap_or_else(|| format!("{{{name}}}"))
            }
            Inline::Strong { children, .. } | Inline::Emphasis { children, .. } => text(children),
            _ => panic!("{node:?}"),
        })
        .collect()
}

fn attribution(block: &Block) -> &Attribution {
    match &block.kind {
        BlockKind::Quote { attribution, .. } | BlockKind::Verse { attribution, .. } => {
            attribution.as_ref().expect("attribution")
        }
        _ => panic!("{block:?}"),
    }
}

fn first_body(block: &Block) -> String {
    match &block.kind {
        BlockKind::Paragraph { inlines } => text(inlines),
        BlockKind::Quote { blocks, .. } => first_body(&blocks[0]),
        _ => panic!("{block:?}"),
    }
}

#[test]
fn unquoted_values_are_raw_source_and_expanded_references() {
    let source = ":who: A<b>\n\n[quote, *Ann* & {who} (C)]\nx";
    let doc = parse(source).document;
    let author = attribution(&doc.blocks[0]).author.as_ref().unwrap();
    let [Inline::Raw { value, span }, Inline::AttributeRef {
        name,
        value: resolved,
        span: ref_span,
        ..
    }, Inline::Raw { value: tail, .. }] = author.as_slice()
    else {
        panic!("{author:?}");
    };
    // No quotes, replacements or escaping: Ruby emits the source as is.
    assert_eq!(value, "*Ann* & ");
    assert_eq!(&source[span.start..span.end], "*Ann* & ");
    assert_eq!(tail, " (C)");
    // Definition values keep their own protection inside the reference.
    assert_eq!((name.as_str(), resolved.as_deref()), ("who", Some("A<b>")));
    assert_eq!(&source[ref_span.start..ref_span.end], "{who}");
}

#[test]
fn single_quoted_values_get_normal_substitutions_and_double_quotes_do_not() {
    let doc = parse(":a: X\n\n[quote, \"*d* {a}\", '*s* \\{a} (C)']\nx").document;
    let attr = attribution(&doc.blocks[0]);
    assert_eq!(text(attr.author.as_ref().unwrap()), "*d* X");
    let cite = attr.citetitle.as_ref().unwrap();
    assert!(matches!(cite[0], Inline::Strong { .. }), "{cite:?}");
    // The early a-stage removes the escape; the normal pass expands it again.
    assert_eq!(text(cite), "s X ©");
}

#[test]
fn expanded_values_split_fields_and_shared_references_keep_their_span() {
    let source = ":a: X, Y\n\n[quote, {a}]\nx";
    let doc = parse(source).document;
    let attr = attribution(&doc.blocks[0]);
    let start = source.find("{a}").unwrap();
    for (part, expected) in [(&attr.author, "X"), (&attr.citetitle, "Y")] {
        let [Inline::AttributeRef { span, .. }] = part.as_deref().unwrap() else {
            panic!("{part:?}");
        };
        assert_eq!((span.start, span.end), (start, start + 3));
        assert_eq!(text(part.as_ref().unwrap()), expected);
    }
}

#[test]
fn reading_effects_are_not_replayed_but_initial_metadata_enters_header() {
    let doc = parse(":n: 5\n\nA {n}\n\n[quote, {counter:n}]\n____\nB {n}\n____\n\nC {n}").document;
    assert_eq!(
        text(attribution(&doc.blocks[1]).author.as_ref().unwrap()),
        "6"
    );
    assert_eq!(first_body(&doc.blocks[1]), "B 5");
    assert_eq!(first_body(&doc.blocks[2]), "C 5");
    assert_eq!(doc.blocks[1].attribute_context.get("n"), Some("6"));

    let doc = parse(":n: 5\n\n[quote, a, '{counter:n} {n}']\n____\nB {n}\n____").document;
    assert_eq!(doc.attributes.get("n"), Some("6"));
    let cite = attribution(&doc.blocks[0]).citetitle.as_ref().unwrap();
    assert_eq!(text(cite), "6 6");
    assert_eq!(first_body(&doc.blocks[0]), "B 6");
}

#[test]
fn overwritten_and_orphan_lines_still_execute() {
    let source = "para\n\n[quote, {counter:n}]\n[quote, {counter:n}]\nx\n\n====\n[quote, {counter:n}]\n====\n\nafter";
    let doc = parse(source).document;
    assert_eq!(
        text(attribution(&doc.blocks[1]).author.as_ref().unwrap()),
        "2"
    );
    assert_eq!(doc.blocks[3].attribute_context.get("n"), Some("3"));
    // Reading effects stay out of the header snapshot after the first block.
    assert_eq!(doc.attributes.get("n"), None);
}

#[test]
fn locked_host_counters_advance_the_shared_registry() {
    let mut attributes = Attributes::new();
    attributes.set("n", "8");
    let doc = parse_with_options(
        "para\n\n[quote, {counter:n}, '{counter:n}']\nBody {counter:n} {n}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    let attr = attribution(&doc.blocks[1]);
    assert_eq!(text(attr.author.as_ref().unwrap()), "9");
    assert_eq!(text(attr.citetitle.as_ref().unwrap()), "10");
    assert_eq!(first_body(&doc.blocks[1]), "Body 11 8");
}

#[test]
fn missing_policies_apply_to_the_whole_line() {
    let doc = parse(":attribute-missing: drop-line\n\n[quote, P]\n[quote, {nope}]\nx").document;
    assert_eq!(
        text(attribution(&doc.blocks[0]).author.as_ref().unwrap()),
        "P"
    );
    let doc = parse(":attribute-missing: drop\n\n[quote, A {nope}, W]\nx").document;
    let attr = attribution(&doc.blocks[0]);
    // Field boundaries are found after expansion: the trailing blank is trimmed.
    assert_eq!(text(attr.author.as_ref().unwrap()), "A");
    assert_eq!(text(attr.citetitle.as_ref().unwrap()), "W");
}

#[test]
fn single_quoted_warnings_repeat_with_physical_origins() {
    let source = ":attribute-missing: warn\r\n\r\nЖ\r\n\r\n[quote, x {nope}, 'y {nope2}']\r\nx";
    let options = ParseOptions {
        source_id: SourceId(4),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .collect();
    assert_eq!(warnings.len(), 3, "{:?}", result.diagnostics);
    let slice = |i: usize| &source[warnings[i].span.start..warnings[i].span.end];
    assert_eq!(slice(0), "{nope}");
    assert_eq!(slice(1), "{nope2}");
    // The repeated normal pass points to the whole transformed field.
    assert_eq!(slice(2), "y {nope2}");
    assert!(warnings.iter().all(|w| w.span.source_id == SourceId(4)));
}

#[test]
fn tokens_project_line_references_without_executing_them_again() {
    let source = "para\n\n[quote, {counter:n}, '*w* {counter:n}']\nBody {n}";
    let options = ParseOptions::default();
    let first = tokens_with_options(source, &options);
    assert_eq!(first, tokens_with_options(source, &options));
    for (start, _) in source.match_indices("{counter:n}") {
        assert!(
            first.iter().any(|t| t.kind == TokenKind::AttributeRef
                && t.span.start == start
                && t.span.end == start + "{counter:n}".len()),
            "{first:?}"
        );
    }
    assert!(first
        .iter()
        .any(|t| t.kind == TokenKind::BlockAttributeList && t.span.start == 6));
    let doc = parse(source).document;
    assert_eq!(first_body(&doc.blocks[1]), "Body {n}");
}

#[test]
fn other_fields_use_the_expanded_line() {
    let doc = parse(":s: verse\n:a: X, Y\n\n[{s}#q{a}.r, A]\nl1\nl2").document;
    let block = &doc.blocks[0];
    assert!(matches!(block.kind, BlockKind::Verse { .. }));
    assert_eq!(block.id.as_deref(), Some("qX"));
    // `Y.r` is the expanded second field, so it is the author.
    assert_eq!(text(attribution(block).author.as_ref().unwrap()), "Y.r");
    assert!(block.roles.is_empty());
    // Single-quoted named values run (and keep) their effects too.
    let doc = parse("[quote, A, caption='{counter:n}']\nx\n\n{n}").document;
    assert_eq!(doc.blocks[0].attrs.named["caption"], "1");
}

#[test]
fn incomplete_lists_do_not_panic_or_escape_the_source() {
    for source in [
        "[quote, '{counter:n}",
        "[quote, \"{a}\\\"]",
        "[quote, '']\nx",
        "[quote, 'pass:[x]']",
        "[{counter:n},{counter:n}]",
        "[quote, Ж {",
        "[quote, Ж {nope}\\]",
    ] {
        let result = parse(source);
        let ts = tokens_with_options(source, &ParseOptions::default());
        for token in ts {
            assert!(
                source.get(token.span.start..token.span.end).is_some(),
                "{source}"
            );
        }
        assert!(result
            .diagnostics
            .iter()
            .all(|d| source.get(d.span.start..d.span.end).is_some()));
    }
}
