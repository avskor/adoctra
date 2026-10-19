use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributePart, Attributes, Block, BlockKind,
    Inline, ParseOptions, Severity, SourceId, Span, TokenKind,
};

fn inlines(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Heading { inlines, .. } | BlockKind::Paragraph { inlines } => inlines,
        _ => panic!("{block:?}"),
    }
}

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
            Inline::Strong { children, .. } | Inline::CrossReference { children, .. } => {
                text(children)
            }
            _ => panic!("{node:?}"),
        })
        .collect()
}

#[test]
fn implicit_doctitle_and_displayed_title_have_separate_execution_phases() {
    let source = "= H {counter:n}\n\n== S {counter:n}\n\n{n} {counter:n}\n\n== More {counter:n}\n\n{counter:n}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    let doc = &result.document;
    let values: Vec<_> = doc.flat_blocks().iter().map(|b| text(inlines(b))).collect();
    assert_eq!(values, ["H 2", "S 2", "2 3", "More 3", "4"]);
    assert_eq!(doc.attributes.get("doctitle"), Some("H 1"));
    assert_eq!(doc.attributes.get("n"), Some("1"));
    assert_eq!(doc.header.as_ref().unwrap().title, "H {counter:n}");
    assert_eq!(doc.flat_blocks()[0].attribute_context.get("n"), Some("1"));
    assert_eq!(doc.flat_blocks()[3].attribute_context.get("n"), Some("3"));
    assert_eq!(doc.flat_blocks()[1].id.as_deref(), Some("_s_2"));
    assert_eq!(doc.flat_blocks()[3].id.as_deref(), Some("_more_3"));
    let reference = inlines(doc.flat_blocks()[0])
        .iter()
        .find(|n| matches!(n, Inline::AttributeRef { .. }))
        .unwrap();
    assert_eq!(
        &source[reference.span().start..reference.span().end],
        "{counter:n}"
    );
    assert!(
        matches!(reference, Inline::AttributeRef { parts, .. } if parts == &[AttributePart::Raw("2".into())])
    );
}

#[test]
fn section_effects_affect_reader_and_definitions_but_are_not_replayed() {
    let source = "== S {set:x:yes}{counter:n}\n\nifdef::x[]\nIncluded {x} {counter:n}\nendif::[]\n\n:copy: {counter:n}\n\n{copy} {n} {counter:n}";
    let result = parse(source);
    let values: Vec<_> = result
        .document
        .flat_blocks()
        .iter()
        .map(|b| text(inlines(b)))
        .collect();
    assert_eq!(values, ["S 1", "Included {x} 1", "2 1 2"]);
    assert_eq!(result.document.attributes.get("x"), None);
    assert_eq!(
        result.document.flat_blocks()[1].attribute_context.get("x"),
        Some("yes")
    );
    assert_eq!(
        result.document.flat_blocks()[2].attribute_context.get("n"),
        Some("2")
    );
}

#[test]
fn displayed_header_effects_do_not_change_prior_reading() {
    let source = "= H {set:x:header}\n:x!: \n\n== S {x}\n\nifdef::x[]\nWrong\nendif::[]\n\n{x}";
    let doc = parse(source).document;
    assert_eq!(doc.flat_blocks().len(), 3);
    assert_eq!(text(inlines(doc.flat_blocks()[1])), "S {x}");
    assert_eq!(text(inlines(doc.flat_blocks()[2])), "header");
    assert_eq!(doc.attributes.get("x"), None);
}

#[test]
fn explicit_declarations_replay_after_the_displayed_header() {
    let doc = parse("= H {counter:n}\n\n:n: 1\n\n{counter:n}\n\n:n!:\n\n{counter:n}").document;
    let values: Vec<_> = doc.flat_blocks().iter().map(|b| text(inlines(b))).collect();
    assert_eq!(values, ["H 2", "2", "1"]);
}

#[test]
fn locked_counter_registry_spans_header_sections_and_conversion() {
    let mut attributes = Attributes::new();
    attributes.set("n", "8");
    attributes.set("x", "host");
    let doc = parse_with_options(
        "= H {counter:n}\n\n== S {counter:n} {set:x:section}{x}\n\n{counter:n} {n} {x}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    assert_eq!(text(inlines(doc.flat_blocks()[0])), "H 11");
    assert_eq!(text(inlines(doc.flat_blocks()[1])), "S 10 host");
    assert_eq!(text(inlines(doc.flat_blocks()[2])), "12 8 host");
    assert_eq!(doc.attributes.get("n"), Some("8"));
}

#[test]
fn section_title_is_cached_for_ids_and_repeated_xrefs() {
    let doc = parse("[[here]]\n== S *{counter:n}*\n\n<<here>> <<here>> {counter:n}").document;
    assert_eq!(text(inlines(doc.flat_blocks()[0])), "S 1");
    assert_eq!(text(inlines(doc.flat_blocks()[1])), "S 1 S 1 1");
    assert!(matches!(
        &inlines(doc.flat_blocks()[0])[1],
        Inline::Strong { .. }
    ));
}

#[test]
fn passes_and_repeated_link_params_share_section_state() {
    let doc = parse("== S pass:a[{counter:n}] {counter:n} link:page['\\{counter:n}',role=x]\n\n== Next {n}\n\n{counter:n}").document;
    assert_eq!(text(inlines(doc.flat_blocks()[0])), "S 3 1 2");
    assert!(
        matches!(&inlines(doc.flat_blocks()[0])[5], Inline::Substituted { parts, .. }
        if matches!(&parts[0], AttributePart::OpenLink(link) if link.target == "page")
            && parts[1] == AttributePart::Raw("2".into()))
    );
    assert_eq!(text(inlines(doc.flat_blocks()[1])), "Next 3");
    assert_eq!(text(inlines(doc.flat_blocks()[2])), "1");
}

#[test]
fn implicit_title_keeps_missing_references_before_normal_display_policy() {
    let source = "= H {set:attribute-missing:warn}{missing} {counter:n}\n\n{counter:n}";
    let result = parse(source);
    assert_eq!(
        result.document.attributes.get("doctitle"),
        Some("H {missing} 1")
    );
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    let d = &result.diagnostics[0];
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(&source[d.span.start..d.span.end], "{missing}");
    assert_eq!(
        text(inlines(result.document.flat_blocks()[0])),
        "H {missing} 2"
    );
}

#[test]
fn host_doctitle_does_not_collide_with_body_placeholder_and_has_empty_origin() {
    let mut attributes = Attributes::new();
    attributes.set("doctitle", "A {counter:n}");
    let result = parse_with_options(
        "B {counter:n}",
        &ParseOptions {
            source_id: SourceId(7),
            attributes,
            ..Default::default()
        },
    );
    assert_eq!(text(inlines(result.document.flat_blocks()[0])), "A 1");
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "B 2");
    for node in inlines(result.document.flat_blocks()[0]) {
        assert_eq!(node.span(), Span::in_source(SourceId(7), 0, 0));
    }
}

#[test]
fn transformed_doctitle_uses_the_enclosing_declaration_origin() {
    let source = ":attribute-missing: warn\n= Source {counter:n}\n:doctitle: pass:none[New {counter:n} {missing}]\n\n{counter:n}";
    let result = parse(source);
    assert_eq!(
        text(inlines(result.document.flat_blocks()[0])),
        "New 2 {missing}"
    );
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "3");
    assert_eq!(result.diagnostics.len(), 1);
    let origin = result.diagnostics[0].span;
    assert_eq!(
        &source[origin.start..origin.end],
        ":doctitle: pass:none[New {counter:n} {missing}]"
    );
    for node in inlines(result.document.flat_blocks()[0]) {
        assert_eq!(node.span(), origin);
    }
    assert!(!adoctra_core::tokens(source)
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == origin));
}

#[test]
fn heading_side_effects_preserve_unicode_crlf_and_stable_tokens() {
    let source = "\u{feff}= Ж {counter:n:Я}\r\n\r\n== Б {counter:n}\r\n\r\n{counter:n}";
    let options = ParseOptions {
        source_id: SourceId(9),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert_eq!(text(inlines(result.document.flat_blocks()[0])), "Ж а");
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "Б а");
    let tokens = tokens_with_options(source, &options);
    for spelling in ["{counter:n:Я}", "{counter:n}"] {
        for (at, _) in source.match_indices(spelling) {
            assert!(tokens
                .iter()
                .any(|token| token.kind == TokenKind::AttributeRef
                    && token.span == Span::in_source(SourceId(9), at, at + spelling.len())));
        }
    }
    assert_eq!(tokens, tokens_with_options(source, &options));
    assert_eq!(
        format!("{result:?}"),
        format!("{:?}", parse_with_options(source, &options))
    );
}

#[test]
fn setext_header_executes_early_attributes_before_compat_passthrough() {
    let source = "`{counter:n}`\n=============\n\n== S {counter:n}\n\n{counter:n}";
    let doc = parse(source).document;
    assert_eq!(doc.attributes.get("n"), Some("1"));
    assert_eq!(doc.attributes.get("compat-mode"), Some(""));
    assert_eq!(text(inlines(doc.flat_blocks()[0])), "{counter:n}");
    assert_eq!(text(inlines(doc.flat_blocks()[1])), "S 2");
    assert_eq!(text(inlines(doc.flat_blocks()[2])), "2");
}

#[test]
fn section_leveloffset_effect_only_changes_subsequent_headings() {
    let doc = parse("== One {set:leveloffset:+1}\n\n== Two\n\n{x}").document;
    assert!(matches!(
        &doc.flat_blocks()[0].kind,
        BlockKind::Heading { level: 1, .. }
    ));
    assert!(matches!(
        &doc.flat_blocks()[1].kind,
        BlockKind::Heading { level: 2, .. }
    ));
}

#[test]
fn incomplete_unicode_titles_remain_error_tolerant() {
    let source = "= Ж pass:n[{counter:n}] {set:x:✓}\n\n== Б link:page['\\{counter:n}',role=x]";
    for end in source.char_indices().map(|(i, _)| i).chain([source.len()]) {
        let partial = &source[..end];
        let result = parse(partial);
        assert!(result.diagnostics.iter().all(|d| d.span.end <= end));
        assert!(adoctra_core::tokens(partial)
            .iter()
            .all(|t| t.span.end <= end));
    }
}

fn values(source: &str, embedded: bool) -> Vec<String> {
    let options = ParseOptions {
        embedded,
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    result
        .document
        .flat_blocks()
        .iter()
        .filter(|b| !matches!(b.kind, BlockKind::Toc))
        .map(|b| text(inlines(b)))
        .collect()
}

fn footnotes(source: &str, embedded: bool) -> Vec<(String, String)> {
    let options = ParseOptions {
        embedded,
        ..Default::default()
    };
    parse_with_options(source, &options)
        .document
        .footnotes
        .iter()
        .map(|f| {
            let content = f
                .content
                .iter()
                .map(|part| match part {
                    AttributePart::Text(text) | AttributePart::Raw(text) => text.as_str(),
                    _ => "",
                })
                .collect();
            (f.index.clone(), content)
        })
        .collect()
}

#[test]
fn titles_without_generated_ids_execute_where_they_convert() {
    let source = ":sectids!:\n\n== A {counter:n}\n\np {n}\n\n[discrete]\n== D {counter:n}\n\nq {n}";
    let result = parse(source);
    let doc = &result.document;
    let blocks = doc.flat_blocks();
    let values: Vec<_> = blocks.iter().map(|b| text(inlines(b))).collect();
    assert_eq!(values, ["A 1", "p 1", "D 2", "q 2"]);
    assert_eq!(blocks[0].id, None);
    // The reading snapshot does not see the conversion effect.
    assert_eq!(blocks[1].attribute_context.get("n"), None);
    let reference = inlines(blocks[0])
        .iter()
        .find(|n| matches!(n, Inline::AttributeRef { .. }))
        .unwrap();
    let span = reference.span();
    assert_eq!(&source[span.start..span.end], "{counter:n}");
    assert_eq!(inlines(blocks[0])[0].span(), Span::new(15, 17));
}

#[test]
fn explicit_ids_defer_titles_without_attribute_references() {
    // Like Ruby, a footnote in such a title numbers where it converts: at
    // the heading, or first for a standalone document's `<title>`.
    let source = "a footnote:[one]\n\n[#s]\n== T footnote:[two]\n\nb footnote:[three]";
    assert_eq!(
        footnotes(source, true),
        [("1", "one"), ("2", "two"), ("3", "three")].map(|(a, b)| (a.into(), b.into()))
    );
    assert_eq!(
        footnotes(source, false),
        [("1", "two"), ("2", "one"), ("3", "three")].map(|(a, b)| (a.into(), b.into()))
    );
    // A reference converts it first.
    assert_eq!(
        footnotes(
            "= D\n\n<<s>>\n\n[[s]]\n== T footnote:[two]\n\nb footnote:[three]",
            false
        ),
        [("1", "two"), ("2", "three")].map(|(a, b)| (a.into(), b.into()))
    );
    // With an attribute reference it converts while reading.
    assert_eq!(
        values("[#a]\n== A {counter:n}\n\np {n}", false),
        ["A 1", "p {n}"]
    );
    assert_eq!(
        values("== T {counter:n} [[s]]\n\np {n}", false),
        ["T 1", "p {n}"]
    );
}

#[test]
fn a_table_of_contents_converts_titles_first() {
    let body = "== A {counter:n}\n\np {n}\n\n=== A1 {counter:n}\n\n==== A2 {counter:n}\n\n== B {counter:n}\n\nq {n}";
    // The automatic TOC lists levels up to `toclevels` before the body.
    assert_eq!(
        values(&format!("= D\n:sectids!:\n:toc:\n\n{body}"), false),
        ["D", "A 1", "p 3", "A1 2", "A2 4", "B 3", "q 4"]
    );
    assert_eq!(
        values(
            &format!("= D\n:sectids!:\n:toc:\n:toclevels: 1\n\n{body}"),
            true
        ),
        ["D", "A 1", "p 2", "A1 3", "A2 4", "B 2", "q 4"]
    );
    // The preamble's TOC converts before its content.
    assert_eq!(
        values(
            "= D\n:sectids!:\n:toc: preamble\n\npre {n}\n\n== A {counter:n}\n\np {n}",
            false
        ),
        ["D", "pre 1", "A 1", "p 1"]
    );
    // A macro converts the outline with the state at its position.
    assert_eq!(
        values(
            "= D\n:sectids!:\n:toc: macro\n\n== A {counter:n}\n\np {n}\n\ntoc::[levels=0]\n\n== B {counter:n}\n\n=== C {counter:n}\n\nq {n}",
            false
        ),
        ["D", "A 1", "p 1", "B 2", "C 3", "q 3"]
    );
    // Without a TOC each title converts at its heading.
    assert_eq!(
        values(&format!("= D\n:sectids!:\n\n{body}"), false),
        ["D", "A 1", "p 1", "A1 2", "A2 3", "B 4", "q 4"]
    );
}

#[test]
fn a_standalone_title_converts_the_first_section_with_the_header_state() {
    let source = ":sectids!:\n\npara\n\n:x: 1\n\n== A {x} {counter:n}\n\np {n}";
    assert_eq!(values(source, false), ["para", "A {x} 1", "p 1"]);
    assert_eq!(values(source, true), ["para", "A 1 1", "p 1"]);
}

#[test]
fn deferred_titles_keep_their_tokens() {
    let options = ParseOptions::default();
    let kinds = |source: &str| -> Vec<(TokenKind, String)> {
        tokens_with_options(source, &options)
            .iter()
            .map(|t| (t.kind, source[t.span.start..t.span.end].to_owned()))
            .collect()
    };
    let body = "== A *b* {counter:n} footnote:[f] <<s>>\r\n\r\n[[s]]\r\n== S _Я_\r\n";
    let deferred = kinds(&format!(":sectids!:\r\n\r\n{body}"));
    let read = kinds(body);
    assert_eq!(deferred[1..], read[..]);
    assert!(read.contains(&(TokenKind::AttributeRef, "{counter:n}".into())));
}
