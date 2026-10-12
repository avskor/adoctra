use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributeFormat as F, AttributePart as P,
    Attributes, BlockKind, Inline, ParseOptions, SourceMap, TokenKind,
};

fn parts(source: &str) -> Vec<P> {
    parse(source)
        .document
        .attributes
        .raw_parts("v")
        .unwrap_or_default()
        .to_vec()
}

#[test]
fn quotes_produce_semantic_boundaries_and_flat_text_without_backend_tags() {
    for (marker, format) in [
        ("*", F::Strong),
        ("_", F::Emphasis),
        ("`", F::Monospace),
        ("#", F::Mark),
        ("^", F::Superscript),
        ("~", F::Subscript),
    ] {
        let source = format!(":v: pass:c,q[{marker}Ж&{marker}]\n\n{{v}}");
        let doc = parse(&source).document;
        assert_eq!(doc.attributes.get("v"), Some("Ж&"));
        let expected = [P::Open(format), P::Text("Ж&".into()), P::Close(format)];
        assert_eq!(doc.attributes.raw_parts("v").unwrap(), expected);
        let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
            panic!()
        };
        assert!(
            matches!(&inlines[0], Inline::AttributeRef { parts, value: Some(value), span, .. }
            if parts == &expected && value == "Ж&" && &source[span.start..span.end] == "{v}")
        );
    }
}

#[test]
fn protection_after_quotes_turns_generated_markup_into_literal_text() {
    let doc = parse(":v: pass:q,c[*A & B*]\n\n{v}").document;
    assert_eq!(doc.attributes.get("v"), Some("<strong>A & B</strong>"));
    assert!(doc.attributes.raw_parts("v").is_none());
    assert_eq!(
        parts(":v: pass:c,q,specialcharacters[*A*]"),
        [P::Open(F::Strong), P::Text("A".into()), P::Close(F::Strong)]
    );
}

#[test]
fn attribute_expansion_preserves_structure_and_selected_order() {
    assert_eq!(
        parts(":name: _Ada_\n:v: pass:a,q[*{name}*]"),
        [
            P::Open(F::Strong),
            P::Open(F::Emphasis),
            P::Text("Ada".into()),
            P::Close(F::Emphasis),
            P::Close(F::Strong)
        ]
    );
    assert_eq!(
        parts(":name: _Ada_\n:v: pass:q,a[*{name}*]"),
        [
            P::Open(F::Strong),
            P::Text("_Ada_".into()),
            P::Close(F::Strong)
        ]
    );
    let source = ":name: pass:q[*Ada*]\n:v: {name}";
    assert_eq!(
        parts(source),
        [
            P::Open(F::Strong),
            P::Raw("Ada".into()),
            P::Close(F::Strong)
        ]
    );
    assert_eq!(
        parts(":v: pass:q,quotes[\\*Ada*]"),
        [P::Raw("*Ada*".into())]
    );
}

#[test]
fn replacements_preserve_formatting_and_cannot_join_across_boundaries() {
    let source = ":v: pass:c,q,r[*(C) &copy;*]\n\n{v}";
    assert_eq!(
        parts(source),
        [
            P::Open(F::Strong),
            P::Raw("&#169;".into()),
            P::Text(" ".into()),
            P::Raw("&copy;".into()),
            P::Close(F::Strong)
        ]
    );
    let doc = parse(":v: pass:q[**C**]\n\n({v})").document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    assert!(
        matches!(&inlines[1], Inline::AttributeRef { value: Some(value), parts, .. }
        if value == "C" && parts == &[P::Open(F::Strong), P::Raw("C".into()), P::Close(F::Strong)])
    );
    assert_eq!(
        parts(":v: pass:q,r[&co**py**;]"),
        [
            P::Raw("&co".into()),
            P::Open(F::Strong),
            P::Raw("py".into()),
            P::Close(F::Strong),
            P::Raw(";".into())
        ]
    );
}

#[test]
fn author_derivation_preserves_formatting_and_original_declarations() {
    let source =
        "= Doc\nJohn Doe; Mary Roe\n:author_2: pass:c,q[*Анна_Мария* _Смирнова_]\n\n{authors}";
    let doc = parse(source).document;
    assert_eq!(doc.attributes.get("firstname_2"), Some("Анна Мария"));
    assert_eq!(doc.attributes.get("lastname_2"), Some("Смирнова"));
    assert_eq!(doc.attributes.get("authorinitials_2"), Some("АС"));
    assert!(doc
        .attributes
        .raw_parts("authors")
        .unwrap()
        .contains(&P::Open(F::Strong)));
    assert_eq!(doc.header.unwrap().authors[1].name, "Mary Roe");
    let doc =
        parse("= Doc\n:authors: pass:q[*Ada* Lovelace; _Grace_ Hopper]\n\n{authors}").document;
    assert_eq!(doc.attributes.get("authorcount"), Some("2"));
    assert!(doc
        .attributes
        .raw_parts("author_2")
        .unwrap()
        .contains(&P::Open(F::Emphasis)));
}

#[test]
fn snapshots_host_parts_and_forward_references_keep_boundaries() {
    let source = ":v: pass:q[*Alpha*]\n\n<<_alpha>>\n\n== {v}\n\n:v: pass:q[_Beta_]\n\n{v}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    let doc = result.document;
    assert_eq!(doc.blocks[1].id.as_deref(), Some("_alpha"));
    assert_eq!(doc.attributes.get("v"), Some("Alpha"));
    assert_eq!(doc.blocks[2].attribute_context.get("v"), Some("Beta"));
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    let Inline::CrossReference { children, .. } = &inlines[0] else {
        panic!()
    };
    assert!(
        matches!(&children[0], Inline::AttributeRef { parts, .. } if parts.contains(&P::Open(F::Strong)))
    );
    let mut attributes = Attributes::new();
    attributes.set_parts(
        "v",
        vec![P::Open(F::Mark), P::Text("Host".into()), P::Close(F::Mark)],
    );
    let doc = parse_with_options(
        ":v: ignored\n\n{v}",
        &ParseOptions {
            attributes: attributes.clone(),
            ..Default::default()
        },
    )
    .document;
    assert_eq!(doc.attributes.raw_parts("v"), attributes.raw_parts("v"));
}

#[test]
fn unicode_crlf_tokens_and_partial_values_keep_physical_reference_spans() {
    let source = "\u{feff}:v: pass:c,q[*Ж&* _é_]\r\n\r\nДо {v}";
    let mut sources = SourceMap::new();
    let source_id = sources.add("quotes.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let doc = parse_with_options(source, &options).document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    let span = inlines[1].span();
    assert_eq!(sources.slice(span), Some("{v}"));
    assert!(tokens_with_options(source, &options)
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == span));
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

#[test]
fn compat_quotes_use_the_cached_mode_after_header_completion() {
    let doc =
        parse(":compat-mode:\n:v: pass:q[+Header+]\n\n{v}\n\n:v: pass:q[+Body+]\n\n{v}").document;
    assert_eq!(doc.attributes.get("v"), Some("+Header+"));
    assert_eq!(doc.blocks[1].attribute_context.get("v"), Some("Body"));
    assert_eq!(
        doc.blocks[1].attribute_context.raw_parts("v").unwrap()[0],
        P::Open(F::Monospace)
    );
    let mut attributes = Attributes::new();
    attributes.set("compat-mode", "");
    let doc = parse_with_options(
        ":v: pass:q[+Host+]\n\n{v}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    assert_eq!(
        doc.attributes.raw_parts("v").unwrap()[0],
        P::Open(F::Monospace)
    );
}

#[test]
fn incomplete_marker_runs_do_not_recurse_or_rescan_each_suffix() {
    let value = "*a ".repeat(20_000);
    let doc = parse(&format!(":v: pass:q[{value}]\n\n{{v}}")).document;
    assert_eq!(doc.attributes.get("v"), Some(value.as_str()));
}

fn decorated(format: F, id: Option<&str>, role: Option<&str>) -> P {
    P::OpenWithAttributes {
        format,
        id: id.map(str::to_owned),
        role: role.map(str::to_owned),
    }
}

#[test]
fn quote_attributes_are_semantic_for_every_format_and_keep_reference_spans() {
    for (marker, format) in [
        ("*", F::Strong),
        ("**", F::Strong),
        ("_", F::Emphasis),
        ("__", F::Emphasis),
        ("`", F::Monospace),
        ("``", F::Monospace),
        ("#", F::Span),
        ("##", F::Span),
        ("^", F::Superscript),
        ("~", F::Subscript),
    ] {
        let source = format!(":v: pass:c,q[[.до#якорь.после]{marker}Ж&{marker}]\n\n{{v}}");
        let result = parse(&source);
        assert!(result.diagnostics.is_empty());
        let expected = [
            decorated(format, Some("якорь"), Some("до после")),
            P::Text("Ж&".into()),
            P::Close(format),
        ];
        assert_eq!(result.document.attributes.get("v"), Some("Ж&"));
        assert_eq!(result.document.attributes.raw_parts("v").unwrap(), expected);
        let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
            panic!()
        };
        assert!(
            matches!(&inlines[0], Inline::AttributeRef { parts, value: Some(value), span, .. }
            if parts == &expected && value == "Ж&" && &source[span.start..span.end] == "{v}")
        );
    }
}

#[test]
fn quoted_attribute_grammar_preserves_empty_roles_spacing_and_first_positional_value() {
    for (list, id, role) in [
        (".a..b", None, Some("a  b")),
        ("..", None, Some("")),
        ("#id.", Some("id"), None),
        (".#id", Some("id"), None),
        (".a#id.b.c", Some("id"), Some("a b c")),
        ("  red  blue ,ignored", None, Some("red  blue")),
        ("#id#next.r", Some("id#next"), Some("r")),
    ] {
        assert_eq!(
            parts(&format!(":v: pass:q[[{list}]*x*]"))[0],
            decorated(F::Strong, id, role)
        );
    }
    for list in [" ", ".", "#", ",ignored"] {
        assert_eq!(
            parts(&format!(":v: pass:q[[{list}]#x#]")),
            [P::Raw("x".into())]
        );
    }
    assert_eq!(parts(":v: pass:q[[]*x*]")[0], P::Raw("[]".into()));
}

#[test]
fn typographic_quotes_use_a_semantic_span_and_mark_with_empty_attributes_has_no_wrapper() {
    let p = parts(":v: pass:q[[#id.red]\"`x`\"]");
    assert_eq!(
        p,
        [
            decorated(F::Span, Some("id"), Some("red")),
            P::Raw("&#8220;x&#8221;".into()),
            P::Close(F::Span)
        ]
    );
    assert_eq!(parts(":v: pass:q[[ ]#x#]"), [P::Raw("x".into())]);
    assert_eq!(
        parts(":v: pass:q[[ ]*x*]"),
        [P::Open(F::Strong), P::Raw("x".into()), P::Close(F::Strong)]
    );
}

#[test]
fn escaped_lists_and_left_boundaries_follow_constrained_and_unconstrained_rules() {
    assert_eq!(
        parts(":v: pass:q[\\[.red]*x*]"),
        [
            P::Raw("[.red]".into()),
            P::Open(F::Strong),
            P::Raw("x".into()),
            P::Close(F::Strong)
        ]
    );
    assert_eq!(
        parts(":v: pass:q[\\[.red]^x^]"),
        [P::Raw("[.red]^x^".into())]
    );
    let p = parts(":v: pass:q[a[.red]*x*]");
    assert_eq!(p[0], P::Raw("a[.red]".into()));
    assert_eq!(p[1], P::Open(F::Strong));
    assert_eq!(
        parts(":v: pass:q[a[.red]**x**]")[1],
        decorated(F::Strong, None, Some("red"))
    );
}

#[test]
fn attribute_lists_expand_eagerly_and_later_stages_preserve_metadata() {
    assert_eq!(
        parts(":r: .red\n:v: pass:q[[{r}]*x*]")[0],
        decorated(F::Strong, None, Some("red"))
    );
    assert_eq!(
        parts(":r: blue\n:v: pass:q,a[[.a\\{r}]*x*]")[0],
        decorated(F::Strong, None, Some("ablue"))
    );
    assert_eq!(
        parts(":r: A&B\n:v: pass:q[[{r}]*x*]")[0],
        decorated(F::Strong, None, Some("A&amp;B"))
    );
    assert_eq!(
        parts(":v: pass:q[[.a{sp}b]*x*]")[0],
        decorated(F::Strong, None, Some("a b"))
    );
    assert_eq!(
        parts(":v: pass:q,r[[#(C).a...]*x*]")[0],
        decorated(F::Strong, Some("&#169;"), Some("a   "))
    );
    assert_eq!(
        parts(":v: pass:c,q,r[[.A&copy;B]*x*]")[0],
        decorated(F::Strong, None, Some("A&copy;B"))
    );
    let doc = parse(":v: pass:q,c[[#id.red]*x*]").document;
    assert_eq!(
        doc.attributes.get("v"),
        Some("<strong id=\"id\" class=\"red\">x</strong>")
    );
    assert!(doc.attributes.raw_parts("v").is_none());
    // Ordinary inline replacements also update roles without flattening them.
    let doc = parse(":v: pass:q[[.(C)]*x*]\n\n{v}").document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    assert!(
        matches!(&inlines[0], Inline::AttributeRef { parts, value: Some(value), .. }
        if value == "x" && parts[0] == decorated(F::Strong, None, Some("&#169;")))
    );
    assert_eq!(
        doc.attributes.raw_parts("v").unwrap()[0],
        decorated(F::Strong, None, Some("(C)"))
    );
}

#[test]
fn attributed_quotes_survive_author_derivation_snapshots_host_values_and_section_ids() {
    let doc =
        parse("= Doc\n:author: pass:q[[#first_id.first_name]*Ada_Lovelace*]\n\n{author}").document;
    assert_eq!(doc.attributes.get("firstname"), Some("Ada Lovelace"));
    assert_eq!(
        doc.attributes.raw_parts("author").unwrap()[0],
        decorated(F::Strong, Some("first id"), Some("first name"))
    );
    let source = "= Doc\nJohn Doe; Mary Roe\n:author_2: pass:q[[.имя]*Анна_Мария* [.фамилия]_Смирнова_]\n\n{authors}";
    let doc = parse(source).document;
    assert_eq!(doc.attributes.get("firstname_2"), Some("Анна Мария"));
    assert_eq!(doc.attributes.get("lastname_2"), Some("Смирнова"));
    assert!(doc
        .attributes
        .raw_parts("authors")
        .unwrap()
        .contains(&decorated(F::Strong, None, Some("имя"))));
    assert_eq!(doc.header.unwrap().authors[1].name, "Mary Roe");
    let doc = parse(
        ":v: pass:q[[#id.red]*Alpha*]\n\n<<_alpha>>\n\n== {v}\n\n:v: pass:q[[.blue]_Beta_]\n\n{v}",
    )
    .document;
    assert_eq!(doc.blocks[1].id.as_deref(), Some("_alpha"));
    assert_eq!(
        doc.attributes.raw_parts("v").unwrap()[0],
        decorated(F::Strong, Some("id"), Some("red"))
    );
    assert_eq!(
        doc.blocks[2].attribute_context.raw_parts("v").unwrap()[0],
        decorated(F::Emphasis, None, Some("blue"))
    );
    let mut attributes = Attributes::new();
    let host = vec![
        decorated(F::Span, Some("host"), Some("role")),
        P::Text("Host".into()),
        P::Close(F::Span),
    ];
    attributes.set_parts("v", host.clone());
    let doc = parse_with_options(
        ":v: ignored\n\n{v}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    assert_eq!(doc.attributes.raw_parts("v").unwrap(), host);
}

#[test]
fn attributed_quotes_keep_unicode_crlf_positions_and_tolerate_partial_lists() {
    let source = "\u{feff}:v: pass:c,q[[#якорь.роль]*Ж&*]\r\n\r\nДо {v}";
    let mut sources = SourceMap::new();
    let source_id = sources.add("quotes.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let doc = parse_with_options(source, &options).document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    assert_eq!(sources.slice(inlines[1].span()), Some("{v}"));
    assert!(tokens_with_options(source, &options)
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == inlines[1].span()));
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
    let input = format!(":v: pass:q[{}]", "[".repeat(20_000));
    assert_eq!(
        parse(&input).document.attributes.get("v").unwrap().len(),
        20_000
    );
}

#[test]
fn replacements_inside_metadata_keep_boundaries_after_multiple_edits_and_escape_removal() {
    for (input, expected) in [
        (r"\(C)-(R)-(TM)", "(C)-&#174;-&#8482;"),
        (r"\...", "..."),
        ("(C)(C)(C)", "&#169;&#169;&#169;"),
        ("&copy; &reg;", "&copy; &reg;"),
    ] {
        let source = format!(":v: pass:c,q,r[[{input}]*word*]");
        assert_eq!(
            parts(&source)[0],
            decorated(F::Strong, None, Some(expected))
        );
    }
    // Lexically identical raw input must remain raw, never acquire semantics.
    assert_eq!(
        parts(":v: pass:r[<strong class=\"(C)\">word</strong>]"),
        [P::Raw("<strong class=\"&#169;\">word</strong>".into())]
    );
}
