use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributeFormat, AttributePart as P, Block,
    BlockKind, Inline, ParseOptions, SourceId, Span, StemStyle, TokenKind,
};

fn formula(block: &Block) -> (StemStyle, &[Inline]) {
    match &block.kind {
        BlockKind::Stem { style, inlines } => (*style, inlines),
        _ => panic!("{block:?}"),
    }
}

fn parts(nodes: &[Inline]) -> &[P] {
    match nodes {
        [Inline::Substituted { parts, .. }] => parts,
        _ => panic!("{nodes:?}"),
    }
}

#[test]
fn formulas_keep_raw_content_metadata_and_physical_ranges() {
    let source = ":stem: latex\r\n\r\n.Формула\r\n[stem#eq.red]\r\n++++\r\nЖ < x & {a} *b*\r\n// raw\r\n++++";
    let options = ParseOptions {
        source_id: SourceId(7),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let block = &result.document.blocks[0];
    assert_eq!(block.id.as_deref(), Some("eq"));
    assert_eq!(block.roles, ["red"]);
    assert_eq!(block.attrs.style.as_deref(), Some("latexmath"));
    let (style, nodes) = formula(block);
    assert_eq!(style, StemStyle::LatexMath);
    assert_eq!(parts(nodes), [P::Text("Ж < x & {a} *b*\n// raw".into())]);
    assert_eq!(
        block.span,
        Span::in_source(SourceId(7), source.find("++++").unwrap(), source.len())
    );
    assert_eq!(
        nodes[0].span(),
        Span::in_source(
            SourceId(7),
            source.find("Ж <").unwrap(),
            source.rfind("\r\n++++").unwrap()
        )
    );
    let tokens = tokens_with_options(source, &options);
    let math: Vec<_> = tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::StemDelimiter | TokenKind::StemText))
        .collect();
    assert_eq!(math.len(), 4);
    assert_eq!(math[1].kind, TokenKind::StemText);
    assert_eq!(
        &source[math[1].span.start..math[1].span.end],
        "Ж < x & {a} *b*"
    );
    assert!(math.iter().all(|t| t.span.source_id == SourceId(7)));
    assert!(!tokens.iter().any(|t| matches!(
        t.kind,
        TokenKind::Strong | TokenKind::AttributeRef | TokenKind::Comment
    )));
}

#[test]
fn selectors_distinguish_absent_empty_and_explicit_aliases() {
    for (selector, expected) in [
        ("stem", StemStyle::LatexMath),
        ("stem,,role=r", StemStyle::LatexMath),
        ("stem,\"\"", StemStyle::AsciiMath),
        ("stem,tex", StemStyle::LatexMath),
        ("stem,unknown", StemStyle::AsciiMath),
        ("asciimath,tex", StemStyle::AsciiMath),
        ("latexmath,asciimath", StemStyle::LatexMath),
    ] {
        let result = parse(&format!(":stem: latex\n\n[{selector}]\n++++\nx\n++++"));
        assert_eq!(
            formula(&result.document.blocks[0]).0,
            expected,
            "{selector}"
        );
    }
    let result = parse(":stem: latex\n\n[stem]\n++++\n{set:stem:asciimath}x\n++++\n\n:stem: asciimath\n\n[stem]\n++++\ny\n++++");
    assert_eq!(formula(&result.document.blocks[0]).0, StemStyle::LatexMath);
    assert_eq!(formula(&result.document.blocks[1]).0, StemStyle::AsciiMath);
}

#[test]
fn selected_substitutions_use_shared_conversion_state_and_trim_after_expansion() {
    let result = parse(":a: Ж\n\n[stem,subs=\"+attributes,+quotes\"]\n++++\n{empty}\n{a} *x* {counter:n}\n{empty}\n++++\n\n{counter:n}");
    let nodes = formula(&result.document.blocks[0]).1;
    assert!(parts(nodes).contains(&P::Open(AttributeFormat::Strong)));
    let Inline::Substituted { value, .. } = &nodes[0] else {
        panic!()
    };
    assert_eq!(value, "Ж x 1");
    assert_eq!(result.document.final_attributes.get("n"), Some("2"));
    assert_eq!(result.document.attributes.get("n"), None);
    let result = parse("[stem,subs=none]\n++++\n<b>x</b>\n++++");
    assert_eq!(
        parts(formula(&result.document.blocks[0]).1),
        [P::Raw("<b>x</b>".into())]
    );
}

#[test]
fn macros_resolve_references_and_register_footnotes() {
    let result = parse("[stem,subs=\"specialcharacters,macros\"]\n++++\n<<target>> footnote:[Note]\n++++\n\n[[target]]\n== Target");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.document.footnotes.len(), 1);
    let p = parts(formula(&result.document.blocks[0]).1);
    assert!(p.iter().any(|p| matches!(p, P::OpenXref(_))));
    assert!(p
        .iter()
        .any(|p| matches!(p, P::Text(s) if s.contains("Target"))));
}

#[test]
fn styles_on_other_blocks_do_not_create_formulas() {
    let result = parse("[stem]\nx\n\n[latexmath]\n----\ny\n----\n\n[asciimath]\n--\nz\n--");
    assert!(matches!(
        result.document.blocks[0].kind,
        BlockKind::Paragraph { .. }
    ));
    assert!(matches!(
        result.document.blocks[1].kind,
        BlockKind::Listing { .. }
    ));
    assert!(matches!(
        result.document.blocks[2].kind,
        BlockKind::Open { .. }
    ));
    for style in ["stem", "latexmath", "asciimath"] {
        let source = format!("[{style}]\nSee [[a]]x <<a>>");
        assert!(parse(&source).diagnostics.is_empty(), "{source}");
    }
}

#[test]
fn empty_and_unterminated_formulas_remain_editable() {
    for source in [
        "[stem]\n++++",
        "[stem]\n++++\nЖ *x*",
        "[stem]\n++++\n\n++++",
    ] {
        let result = parse(source);
        formula(&result.document.blocks[0]);
        let tokens = adoctra_core::tokens(source);
        assert!(tokens
            .iter()
            .all(|t| source.get(t.span.start..t.span.end).is_some()));
        assert!(!tokens.iter().any(|t| t.kind == TokenKind::Strong));
        assert_eq!(result.diagnostics.is_empty(), source.ends_with("\n\n++++"));
    }
}

#[test]
fn block_selectors_warn_without_accepting_inline_shorthand() {
    let result = parse("[stem,subs=\"c,basic,quotes\"]\n++++\n*x*\n++++");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        result.diagnostics[0].message,
        "invalid substitution types for stem: c, basic"
    );
    assert!(
        parts(formula(&result.document.blocks[0]).1).contains(&P::Open(AttributeFormat::Strong))
    );
}

#[test]
fn math_callouts_convert_like_other_blocks() {
    // Ruby catalogs and converts the marks of any block whose subs include
    // callouts, formulas too.
    let result = parse("[stem,subs=verbatim]\n++++\nx <1>\n++++");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let [P::Text(text), P::Callout(callout)] = parts(formula(&result.document.blocks[0]).1) else {
        panic!()
    };
    assert_eq!(text, "x ");
    assert_eq!(callout.number, "1");
    assert_eq!(callout.id.as_deref(), Some("CO1-1"));
}

#[test]
fn final_attributes_follow_conversion_replay_including_trailing_entries() {
    for (source, enabled) in [
        (":stem:\n\nx", true),
        (":stem:\n\nx\n\n:stem!:", true),
        ("x\n\n:stem:", false),
        ("x\n\n:stem:\n\ny", true),
        (":stem:\n\nx\n\n:stem!:\n\ny", false),
        ("{set:stem:}\n\nx", true),
    ] {
        assert_eq!(
            parse(source)
                .document
                .final_attributes
                .get("stem")
                .is_some(),
            enabled,
            "{source}"
        );
    }
}

#[test]
fn nested_formula_tokens_follow_the_parser() {
    let source = "* item\n+\n[stem]\n++++\n*x*\n++++\n\n[cols=\"a\"]\n|===\n|[latexmath]\n++++\n*y*\n++++\n|===\n\n*bold*";
    let tokens = adoctra_core::tokens(source);
    let bodies: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::StemText)
        .map(|t| &source[t.span.start..t.span.end])
        .collect();
    assert_eq!(bodies, ["*x*", "*y*"]);
    let strong: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Strong)
        .map(|t| &source[t.span.start..t.span.end])
        .collect();
    assert_eq!(strong, ["*bold*"]);
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::StemDelimiter)
            .count(),
        4
    );
}

#[test]
fn included_formulas_keep_cross_source_provenance_and_tokens() {
    use adoctra_core::{parse_resource, tokens_resource, MemoryResources, SourceMap};
    let mut resources = MemoryResources::new();
    resources
        .insert("/main.adoc", "[stem]\n++++\ninclude::math.txt[]\n++++")
        .unwrap();
    resources.insert("/math.txt", "Ж < x\r\n*y*").unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let block = &result.document.blocks[0];
    assert!(block.span.provenance.is_some());
    let nodes = formula(block).1;
    assert_eq!(nodes[0].span().source_id, SourceId(1));
    assert_eq!(sources.slice(nodes[0].span()), Some("Ж < x\r\n*y*"));
    let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
    let bodies: Vec<_> = ts
        .iter()
        .filter(|t| t.kind == TokenKind::StemText)
        .collect();
    assert_eq!(bodies.len(), 2);
    assert!(bodies.iter().all(|t| t.span.source_id == SourceId(1)));
    assert!(ts.iter().any(|t| t.kind == TokenKind::IncludeDirective));
    assert!(!ts.iter().any(|t| t.kind == TokenKind::Strong));
}
