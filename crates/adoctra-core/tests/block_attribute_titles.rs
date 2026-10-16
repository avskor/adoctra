use adoctra_core::{
    parse, parse_with_options, tokens_with_options, BlockKind, Inline, ParseOptions, Severity,
    SourceId,
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
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::CrossReference { children, .. } => text(children),
            _ => panic!("{node:?}"),
        })
        .collect()
}

#[test]
fn named_title_shares_the_title_key_with_dot_title_lines() {
    let doc = parse(".A\n[title=B]\npara\n\n[title=B]\n.A\npara").document;
    assert_eq!(text(doc.blocks[0].title.as_ref().unwrap()), "B");
    assert_eq!(text(doc.blocks[1].title.as_ref().unwrap()), "A");
}

#[test]
fn unchanged_titles_keep_exact_spans_and_get_title_substitutions() {
    let source = "Ж\r\n\r\n[quote, A, title='*T* (C)']\r\nx";
    let doc = parse(source).document;
    let title = doc.blocks[1].title.as_ref().unwrap();
    // Single quotes do not request sub_result for title; title subs apply once.
    let Inline::Strong { span, .. } = &title[0] else {
        panic!("{title:?}");
    };
    assert_eq!(&source[span.start..span.end], "*T*");
    assert_eq!(text(title), "T ©");
}

#[test]
fn transformed_titles_take_the_field_range() {
    let source = ":x: X, Y\n:attribute-missing: warn\n\n[title=\"{x} \\{nope} *b*\"]\npara";
    let options = ParseOptions {
        source_id: SourceId(3),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    let title = result.document.blocks[0].title.as_ref().unwrap();
    assert_eq!(text(title), "X, Y {nope} b");
    let field = source.find("{x}").unwrap()..source.find("\"]").unwrap();
    for node in title {
        assert_eq!(
            (node.span().start, node.span().end),
            (field.start, field.end)
        );
    }
    // The escape is removed by the early a-stage, so title subs warn on the
    // whole transformed field.
    let warning = result
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Warning)
        .unwrap();
    assert_eq!(
        &source[warning.span.start..warning.span.end],
        "{x} \\{nope} *b*"
    );
    assert_eq!(warning.span.source_id, SourceId(3));
}

#[test]
fn anchored_titles_with_references_are_cached_while_reading() {
    let doc = parse(
        ":n: 5\n\npara\n\n[#i,title=\\{counter:n}]\npara\n\n[title=\\{counter:n}]\npara\n\n{n}",
    )
    .document;
    // The anchored title executes while reading, the unanchored one during
    // conversion, which starts from the header state again.
    assert_eq!(text(doc.blocks[1].title.as_ref().unwrap()), "6");
    assert_eq!(text(doc.blocks[2].title.as_ref().unwrap()), "6");
    assert_eq!(doc.blocks[2].attribute_context.get("n"), Some("6"));
}

#[test]
fn block_titles_skip_sections_and_discrete_headings_discard_them() {
    let doc = parse(".T\n== S\n\n== U\n\npara").document;
    assert!(doc.blocks[0].title.is_none() && doc.blocks[1].title.is_none());
    assert_eq!(text(doc.blocks[2].title.as_ref().unwrap()), "T");
    // A later title replaces the carried one; section metadata stays.
    let doc = parse("[title=T]\n[#s]\n== S\n\n.U\npara").document;
    assert_eq!(doc.blocks[0].id.as_deref(), Some("s"));
    assert_eq!(text(doc.blocks[1].title.as_ref().unwrap()), "U");
    let doc = parse(".T\n[discrete]\n== H\n\npara").document;
    assert!(doc.blocks.iter().all(|b| b.title.is_none()));
    // A carried title with an expression executes once, on its content block.
    let doc = parse("p\n\n[title={counter:n}]\n== S\n\npara\n\n{n}").document;
    assert_eq!(text(doc.blocks[2].title.as_ref().unwrap()), "1");
}

#[test]
fn named_reftext_shares_the_anchor_key_and_keeps_reference_nodes() {
    let doc = parse("[[id,A]]\n[reftext=B]\npara\n\n[reftext=B]\n[[other,A]]\npara").document;
    assert_eq!(text(doc.blocks[0].reftext.as_ref().unwrap()), "B");
    assert_eq!(text(doc.blocks[1].reftext.as_ref().unwrap()), "A");
    let source = ":x: X, Y\n\n[#id,reftext='*R* {x}']\npara\n\n<<id>>";
    let doc = parse(source).document;
    let reftext = doc.blocks[0].reftext.as_ref().unwrap();
    assert!(matches!(reftext[0], Inline::Strong { .. }), "{reftext:?}");
    let reference = reftext
        .iter()
        .find(|n| matches!(n, Inline::AttributeRef { .. }))
        .unwrap();
    let at = source.find("{x}").unwrap();
    assert_eq!((reference.span().start, reference.span().end), (at, at + 3));
    let BlockKind::Paragraph { inlines } = &doc.blocks[1].kind else {
        panic!()
    };
    assert_eq!(text(inlines), "R X, Y");
}

#[test]
fn header_reftext_and_title_from_lists() {
    let doc = parse("[reftext=R {counter:n}]\n= Doc\n\n{reftext} {n}").document;
    assert_eq!(doc.attributes.get("reftext"), Some("R 1"));
    // A block title is not allowed above the document title: it moves on.
    let doc = parse("[title=T]\n= Doc\n\npara").document;
    assert!(doc.header.is_none());
    assert_eq!(text(doc.blocks[1].title.as_ref().unwrap()), "T");
}

#[test]
fn incomplete_title_and_reftext_fields_stay_in_bounds() {
    for source in [
        "[title='{counter:n}",
        "[title=\\{x}]",
        "[reftext=\"{x}\\\"]\npara",
        "[title=]\n== S",
        "[#i,title={]",
        "[title=Ж\\{",
    ] {
        let result = parse(source);
        for token in tokens_with_options(source, &ParseOptions::default()) {
            assert!(
                source.get(token.span.start..token.span.end).is_some(),
                "{source}"
            );
        }
        assert!(result
            .diagnostics
            .iter()
            .all(|d| source.get(d.span.start..d.span.end).is_some()));
        for block in &result.document.blocks {
            for node in block.title.iter().flatten() {
                assert!(
                    source.get(node.span().start..node.span().end).is_some(),
                    "{source}"
                );
            }
        }
    }
}
