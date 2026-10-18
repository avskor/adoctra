//! Inline STEM follows Ruby's final passthrough extraction stage.
use adoctra_core::{
    parse, tokens, AttributeFormat as F, AttributePart as P, Block, BlockKind, Inline, Span, Stem,
    StemStyle as S, TokenKind,
};

fn inlines(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines } | BlockKind::Heading { inlines, .. } => inlines,
        BlockKind::Section { blocks, .. } => inlines(&blocks[0]),
        _ => panic!("{block:?}"),
    }
}

fn formulas(nodes: &[Inline]) -> Vec<(&Stem, Span)> {
    nodes
        .iter()
        .filter_map(|node| match node {
            Inline::Stem { stem, span } => Some((stem, *span)),
            _ => None,
        })
        .collect()
}

fn span(source: &str, text: &str) -> Span {
    let start = source.find(text).unwrap();
    Span::new(start, start + text.len())
}

#[test]
fn source_formulas_keep_notation_parts_and_spans() {
    let source = "Ж stem:[x < y & z] latexmath:[$\\sqrt{4}$] asciimath:[a_b^2]";
    let doc = parse(source).document;
    let found = formulas(inlines(&doc.blocks[0]));
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].0.style, S::AsciiMath);
    assert_eq!(found[0].0.parts, [P::Text("x < y & z".into())]);
    assert_eq!(found[0].1, span(source, "stem:[x < y & z]"));
    assert_eq!(found[1].0.style, S::LatexMath);
    assert_eq!(found[1].0.parts, [P::Text(r"\sqrt{4}".into())]);
    assert_eq!(found[1].1, span(source, r"latexmath:[$\sqrt{4}$]"));
    assert_eq!(found[2].0.parts, [P::Text("a_b^2".into())]);
}

#[test]
fn generic_stem_uses_the_effective_attribute_and_exact_aliases() {
    for (alias, expected) in [
        ("tex", S::LatexMath),
        ("latex", S::LatexMath),
        ("latexmath", S::LatexMath),
        ("", S::AsciiMath),
        ("unknown", S::AsciiMath),
        ("LaTeX", S::AsciiMath),
    ] {
        let doc = parse(&format!(
            ":stem: {alias}\n\nstem:[x] asciimath:[y]\n\n:stem!:\n\nstem:[z]"
        ))
        .document;
        assert_eq!(formulas(inlines(&doc.blocks[0]))[0].0.style, expected);
        assert_eq!(formulas(inlines(&doc.blocks[0]))[1].0.style, S::AsciiMath);
        assert_eq!(formulas(inlines(&doc.blocks[1]))[0].0.style, S::AsciiMath);
    }
}

#[test]
fn extraction_protects_content_and_selected_substitutions_are_ordered() {
    let doc = parse(
        ":x: VALUE\n\nstem:[{x} *x* (C)] stem:a[{x}] stem:q[*x*] stem:r[(C)] stem:none[<b>x</b>]",
    )
    .document;
    let found = formulas(inlines(&doc.blocks[0]));
    assert_eq!(found[0].0.parts, [P::Text("{x} *x* (C)".into())]);
    assert_eq!(found[1].0.parts, [P::Text("VALUE".into())]);
    assert_eq!(
        found[2].0.parts,
        [P::Open(F::Strong), P::Raw("x".into()), P::Close(F::Strong)]
    );
    assert_eq!(found[3].0.parts, [P::Raw("&#169;".into())]);
    assert_eq!(found[4].0.parts, [P::Raw("<b>x</b>".into())]);
}

#[test]
fn passes_run_before_stem_and_can_extract_nested_formulas() {
    let doc = parse("stem:[+<x>+] +stem:[y]+ pass:m[stem:[z\\]] pass:a[stem:[w]]").document;
    let found = formulas(inlines(&doc.blocks[0]));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].0.parts, [P::Text("<x>".into())]);
    assert_eq!(found[1].0.parts, [P::Text("z".into())]);
}

#[test]
fn attributes_only_generate_math_when_a_nested_macro_pass_extracts_it() {
    let doc = parse(":a: stem:[x]\n:b: pass:m[stem:[y]]\n\n{a} {b}").document;
    let nodes = inlines(&doc.blocks[0]);
    assert!(formulas(nodes).is_empty());
    assert!(nodes.iter().any(|node| matches!(node, Inline::AttributeRef { name, value: Some(value), .. } if name == "a" && value == "stem:[x]")));
    assert!(nodes.iter().any(|node| matches!(node, Inline::AttributeRef { name, parts, .. } if name == "b" && matches!(parts.as_slice(), [P::Stem(stem)] if stem.parts == [P::Text("y".into())]))));
}

#[test]
fn nested_references_and_footnotes_are_resolved_and_collected() {
    let doc = parse("[[t]]Target.\n\nstem:c,m[<<t>>] stem:m[footnote:[Note\\]]").document;
    let found = formulas(inlines(&doc.blocks[1]));
    assert!(
        matches!(found[0].0.parts.as_slice(), [P::OpenXref(x), P::Text(label), P::CloseXref] if x.target == "#t" && label == "[t]")
    );
    assert!(matches!(found[1].0.parts.as_slice(), [P::Footnote(_)]));
    assert_eq!(doc.footnotes.len(), 1);
}

#[test]
fn escaped_macros_empty_content_and_dollars_follow_the_reader() {
    let source = r"\stem:[*x*] stem:[] latexmath:[$$] latexmath:[ $x$ ] stem:[a\]b]";
    let doc = parse(source).document;
    let found = formulas(inlines(&doc.blocks[0]));
    // The nonempty macro regexp consumes `] latexmath:[$$` after stem:[].
    assert_eq!(found[0].0.parts, [P::Text("] latexmath:[$$".into())]);
    assert_eq!(found[1].0.parts, [P::Text(" $x$ ".into())]);
    assert_eq!(found[2].0.parts, [P::Text("a]b".into())]);
    let doc = parse("latexmath:[$$]").document;
    assert_eq!(
        formulas(inlines(&doc.blocks[0]))[0].0.parts,
        Vec::<P>::new()
    );
}

#[test]
fn unicode_crlf_and_multiline_highlighting_preserve_source_ranges() {
    let source = "Ж stem:[а\r\n*б* {x}] Z";
    let doc = parse(source).document;
    let found = formulas(inlines(&doc.blocks[0]));
    assert_eq!(found[0].1, span(source, "stem:[а\r\n*б* {x}]"));
    assert_eq!(found[0].0.parts, [P::Text("а\n*б* {x}".into())]);
    let ts = tokens(source);
    assert_eq!(
        ts.iter()
            .filter(|t| t.kind == TokenKind::InlineMacro)
            .map(|t| t.span)
            .collect::<Vec<_>>(),
        [found[0].1]
    );
    assert!(
        !ts.iter()
            .any(|t| matches!(t.kind, TokenKind::Strong | TokenKind::AttributeRef)),
        "{ts:?}"
    );
    let ts = tokens("pass:m[stem:[x]]");
    assert!(ts.iter().any(|t| t.kind == TokenKind::PassthroughText));
    assert!(
        !ts.iter().any(|t| t.kind == TokenKind::InlineMacro),
        "{ts:?}"
    );
}

#[test]
fn invalid_selectors_and_partial_editor_input_never_panic() {
    let source = "Ж stem:bogus[x]";
    let result = parse(source);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].span, span(source, "bogus"));
    assert!(result.diagnostics[0]
        .message
        .contains("invalid substitution type"));
    for source in [
        "stem:[Ж\\]",
        "stem:[]",
        "latexmath:[$]",
        "stem:normal[pass:m[stem:[x\\]]]",
        "\\stem:[x]\\",
    ] {
        for (end, _) in source
            .char_indices()
            .chain(std::iter::once((source.len(), ' ')))
        {
            parse(&source[..end]);
            tokens(&source[..end]);
        }
    }
    let unfinished = "stem:[".repeat(2000);
    parse(&unfinished);
    tokens(&unfinished);
}

#[test]
fn notation_is_fixed_before_inner_attribute_side_effects() {
    let doc = parse("stem:a[{set:stem:latex}x] stem:[y]\n\nstem:[z]").document;
    let first = formulas(inlines(&doc.blocks[0]));
    assert_eq!(first[0].0.style, S::AsciiMath);
    assert_eq!(first[1].0.style, S::AsciiMath);
    assert_eq!(formulas(inlines(&doc.blocks[1]))[0].0.style, S::LatexMath);
}

#[test]
fn section_ids_and_borrowed_labels_keep_math_and_strip_inner_links() {
    let doc = parse("== Math stem:c,m[link:x[Label\\]]\n\n<<_math_label>>").document;
    let BlockKind::Section { blocks, .. } = &doc.blocks[0].kind else {
        panic!()
    };
    assert_eq!(blocks[0].id.as_deref(), Some("_math_label"));
    let found = formulas(inlines(&blocks[0]));
    assert!(matches!(found[0].0.parts.first(), Some(P::OpenLink(_))));
    let Inline::CrossReference { children, .. } = &inlines(&blocks[1])[0] else {
        panic!()
    };
    let label = formulas(children);
    assert_eq!(label[0].0.parts, [P::Text("Label".into())]);
    let source = "== Math stem:[x]\n\n<<_math_x>>";
    let ts = tokens(source);
    let reference = span(source, "<<_math_x>>");
    assert!(
        ts.iter()
            .any(|t| t.kind == TokenKind::CrossReference && t.span == reference),
        "{ts:?}"
    );
    assert!(
        !ts.iter()
            .any(|t| t.kind == TokenKind::InlineMacro && t.span == reference),
        "{ts:?}"
    );
}
