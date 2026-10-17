use adoctra_core::{
    parse, parse_with_options, tokens_with_options, Block, BlockKind, Inline, ParseOptions,
    Severity, SourceId,
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
            | Inline::Mark { children, .. }
            | Inline::CrossReference { children, .. } => text(children),
            Inline::Anchor { .. } => String::new(),
            _ => panic!("{node:?}"),
        })
        .collect()
}

fn inlines(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines } => inlines,
        _ => panic!("{block:?}"),
    }
}

/// The display text of every cross reference in the last paragraph.
fn xrefs(source: &str) -> Vec<String> {
    let doc = parse(source).document;
    inlines(doc.blocks.last().unwrap())
        .iter()
        .filter_map(|n| match n {
            Inline::CrossReference { children, .. } => Some(text(children)),
            _ => None,
        })
        .collect()
}

#[test]
fn only_ordinary_paragraphs_and_leading_list_anchors_are_cataloged() {
    let source = "p\n\n[NOTE]\nA [[a,R]]\n\n[example]\nB [[b,R]]\n\n[sidebar]\nC [[c,R]]\n\n[verse]\nD [[d,R]]\n\n[quote]\nE [[e,R]]\n\n* item [[f,R]]\n\n<<a>> <<b>> <<c>> <<d>> <<e>> <<f>>";
    assert_eq!(xrefs(source), ["[a]", "[b]", "[c]", "[d]", "[e]", "[f]"]);
    let source = "p\n\nNOTE: A [[a,R]]\n\n====\nB [[b,S]]\n====\n\n* [[c,T]] item\n+\nD [[d,U]]\n\n[Custom style]\nE [[e,V]]\n\n<<a>> <<b>> <<c>> <<d>> <<e>>";
    assert_eq!(xrefs(source), ["R", "S", "T", "U", "V"]);
}

#[test]
fn reftext_uses_reading_state_while_the_paragraph_converts_separately() {
    // Reading: the block reftext counts to 1, the catalog to 2. Conversion
    // starts from the header state and counts the paragraph to 1.
    let source = "p\n\n[[x,{counter:n}]]\npara\n\nA [[a,R {counter:n}]] {n}\n\n<<a>>";
    let doc = parse(source).document;
    assert_eq!(text(inlines(&doc.blocks[2])), "A  1");
    assert_eq!(xrefs(source), ["R 2"]);
}

#[test]
fn empty_expanded_reftext_skips_shorthand_but_not_macro_anchors() {
    let source = ":attribute-missing: drop\n\nx [[a,{nope}]] anchor:b[{nope}] y\n\n<<a>> <<b>>";
    // `b` is registered without reftext and falls back like a textless anchor.
    assert_eq!(xrefs(source), ["[a]", "[b]"]);
    let result = parse(source);
    let unresolved: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.message.starts_with("unresolved cross reference"))
        .collect();
    assert_eq!(unresolved.len(), 1);
    assert!(unresolved[0].message.contains("'a'"));
}

#[test]
fn macro_reftext_unescapes_brackets_and_display_skips_them() {
    let source = "p\n\nx anchor:a[R \\] {counter:n}] y\n\n<<a>>";
    assert_eq!(xrefs(source), ["R ] 1"]);
    let doc = parse(source).document;
    assert_eq!(text(inlines(&doc.blocks[1])), "x  y");
}

#[test]
fn escaped_bibliography_and_adjacent_anchors_follow_the_scan() {
    let source =
        "p\n\nA \\[[a,R]] \\anchor:b[S] [[[c,T]]] [[d]][[e,E]]\n\n<<a>> <<b>> <<c>> <<d>> <<e>>";
    assert_eq!(xrefs(source), ["[a]", "[b]", "[c]", "[d]", "[e]"]);
    let doc = parse(source).document;
    let nodes = inlines(&doc.blocks[1]);
    // An escaped anchor renders its source without the backslash; the
    // retained text keeps its own physical bytes.
    let Inline::Text { value, span } = &nodes[1] else {
        panic!("{nodes:?}");
    };
    assert_eq!(value, "[[a,R]]");
    assert_eq!(&source[span.start..span.end], "[[a,R]]");
}

#[test]
fn cataloged_anchor_with_quote_metadata_formats_per_reference() {
    let doc = parse("p\n\nA [[a,[.r]#X#]]\n\n<<a>>").document;
    let Inline::CrossReference { children, .. } = &inlines(&doc.blocks[2])[0] else {
        panic!()
    };
    assert!(
        matches!(children[0], Inline::Substituted { .. }),
        "{children:?}"
    );
    assert_eq!(text(children), "X");
}

#[test]
fn duplicates_follow_reading_order_with_one_warning() {
    let source = "p\n\n<<a>>\n\nx [[a,F]]\n\n[[a,R]]\npara\n\n<<a>>";
    assert_eq!(xrefs(source), ["F"]);
    let result = parse(source);
    let duplicates = result
        .diagnostics
        .iter()
        .filter(|d| d.message.contains("already in use"))
        .count();
    assert_eq!(duplicates, 1);
}

#[test]
fn reading_warnings_and_reference_spans_keep_physical_origins() {
    let source = ":attribute-missing: warn\r\n\r\nЖ\r\n\r\nA [[a,Ё {nope} *b*]]\r\n\r\n<<a>>";
    let options = ParseOptions {
        source_id: SourceId(5),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    // The catalog and the paragraph conversion each warn, as in Ruby.
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .collect();
    assert_eq!(warnings.len(), 2, "{:?}", result.diagnostics);
    for warning in &warnings {
        assert_eq!(warning.span.source_id, SourceId(5));
        assert_eq!(&source[warning.span.start..warning.span.end], "{nope}");
    }
    let doc = result.document;
    let Inline::CrossReference { children, .. } = &inlines(&doc.blocks[2])[0] else {
        panic!()
    };
    assert_eq!(text(children), "Ё {nope} b");
    let strong = children
        .iter()
        .find(|n| matches!(n, Inline::Strong { .. }))
        .unwrap();
    assert_eq!(&source[strong.span().start..strong.span().end], "*b*");
    assert_eq!(strong.span().source_id, SourceId(5));
}

#[test]
fn incomplete_anchors_do_not_panic() {
    for source in [
        "x [[a,{counter:n}",
        "x [[a,{counter:n}]",
        "x anchor:a[R \\]",
        "x anchor:a[\\]]",
        "* [[a,",
        "\\[[",
        "x [[Ж,R]]\n\n<<Ж>>",
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
    }
}

#[test]
fn anchors_are_recognized_after_the_attribute_stage() {
    // A dropped reference leaves `[[a,]]`, which is not an anchor; the macro
    // form with empty brackets still is.
    let source = ":attribute-missing: drop\n\nx [[a,{nope}]] anchor:b[{nope}] y";
    let doc = parse(source).document;
    let nodes = inlines(&doc.blocks[0]);
    let anchors: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::Anchor { id, span } => Some((id.as_str(), &source[span.start..span.end])),
            _ => None,
        })
        .collect();
    assert_eq!(anchors, [("b", "anchor:b[{nope}]")]);
    assert_eq!(text(nodes), "x [[a,]]  y");

    // An expanded id or closer completes an anchor; the consumed reference
    // stays an empty node after it, keeping its own span.
    let source = ":id: foo\n:c: ]]\n\nx [[{id}]] [[a,b{c} y";
    let doc = parse(source).document;
    let nodes = inlines(&doc.blocks[0]);
    let mut anchors = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        if let Inline::Anchor { id, span } = node {
            anchors.push((id.as_str(), &source[span.start..span.end]));
            let Inline::AttributeRef { value, span, .. } = &nodes[i + 1] else {
                panic!("{nodes:?}");
            };
            assert_eq!(value.as_deref(), Some(""));
            assert!(source[span.start..span.end].starts_with('{'));
        }
    }
    assert_eq!(anchors, [("foo", "[[{id}]]"), ("a", "[[a,b{c}")]);
    assert_eq!(text(nodes), "x   y");
}

#[test]
fn attribute_generated_anchors_stay_parts_of_their_reference() {
    let source = ":a: [[q,R]]\n:x: pass:m[[[k]]z]\n\nv {a} {x}\n\n<<q>> <<k>>";
    let result = parse(source);
    let nodes = inlines(&result.document.blocks[0]);
    let anchors: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::AttributeRef { name, parts, .. } => Some((
                name.as_str(),
                parts
                    .iter()
                    .filter_map(|p| match p {
                        adoctra_core::AttributePart::Anchor(id) => Some(id.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(anchors, [("a", vec!["q"]), ("x", vec!["k"])]);
    // Neither anchor is cataloged while reading, as in Ruby.
    assert_eq!(xrefs(source), ["[q]", "[k]"]);
}

#[test]
fn anchor_consumes_quote_boundaries_and_formatted_reftext() {
    let source = "p\n\nx [[a,*b* _c_]] y *d [[e]] f*\n\n<<a>> <<e>>";
    assert_eq!(xrefs(source), ["b c", "[e]"]);
    let doc = parse(source).document;
    let nodes = inlines(&doc.blocks[1]);
    let Inline::Anchor { id, span } = &nodes[1] else {
        panic!("{nodes:?}");
    };
    assert_eq!(id, "a");
    assert_eq!(&source[span.start..span.end], "[[a,*b* _c_]]");
    let Inline::Strong { children, .. } = &nodes[3] else {
        panic!("{nodes:?}");
    };
    assert!(
        matches!(&children[1], Inline::Anchor { id, .. } if id == "e"),
        "{children:?}"
    );
}

#[test]
fn bracket_reftext_matches_the_lazy_regex() {
    assert_eq!(
        xrefs("p\n\nx [[a, ]]] y [[b,]]] z\n\n<<a>> <<b>>"),
        ["]", "]"]
    );
}

#[test]
fn late_anchor_tokens_cover_the_construct_and_keep_inner_syntax() {
    let source = ":id: foo\n\nx [[{id},*b*]] y";
    let tokens = tokens_with_options(source, &ParseOptions::default());
    let find = |kind: adoctra_core::TokenKind| {
        tokens
            .iter()
            .filter(|t| t.kind == kind)
            .map(|t| &source[t.span.start..t.span.end])
            .collect::<Vec<_>>()
    };
    assert_eq!(find(adoctra_core::TokenKind::Anchor), ["[[{id},*b*]]"]);
    assert_eq!(find(adoctra_core::TokenKind::AttributeRef), ["{id}"]);
    assert_eq!(find(adoctra_core::TokenKind::Strong), ["*b*"]);
}
