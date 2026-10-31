use adoctra_core::{
    parse, parse_with_options, tokens, AttributePart as P, Attributes, BlockKind, Inline,
    ParseOptions, Severity, Span, TokenKind,
};

fn paragraph(source: &str) -> Vec<Inline> {
    let result = parse(source);
    let BlockKind::Paragraph { inlines } = result.document.blocks[0].kind.clone() else {
        panic!("{result:?}")
    };
    inlines
}

fn text(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|n| match n {
            Inline::Text { value, .. }
            | Inline::Raw { value, .. }
            | Inline::Substituted { value, .. } => value.clone(),
            Inline::AttributeRef { name, value, .. } => {
                value.clone().unwrap_or_else(|| format!("{{{name}}}"))
            }
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Monospace { children, .. }
            | Inline::Mark { children, .. }
            | Inline::Superscript { children, .. }
            | Inline::Subscript { children, .. } => text(children),
            _ => panic!("{n:?}"),
        })
        .collect()
}

#[test]
fn both_escape_positions_remove_one_slash_without_creating_reference_tokens() {
    for (input, expected) in [
        (r"\{v}", "{v}"),
        (r"{v\}", "{v}"),
        (r"\{v\}", "{v}"),
        (r"\\{v}", r"\{v}"),
        (r"\\\{v}", r"\\{v}"),
        (r"{v\\}", r"{v\\}"),
        (r"\{no name}", r"\{no name}"),
    ] {
        let src = format!(":v: Ж\n\nBefore {input} after");
        let ns = paragraph(&src);
        assert_eq!(
            text(&ns),
            format!("Before {expected} after"),
            "{src}: {ns:?}"
        );
        assert!(
            !tokens(&src)
                .iter()
                .any(|t| t.kind == TokenKind::AttributeRef),
            "{src}"
        );
        for n in ns {
            assert!(src.get(n.span().start..n.span().end).is_some());
        }
    }
}

#[test]
fn intrinsic_values_have_reference_spans_raw_entity_semantics_and_no_quote_rescan() {
    let src = "{CPP} {nbsp} {asterisk}literal{asterisk} {empty}";
    let ns = paragraph(src);
    let Inline::AttributeRef {
        name,
        value,
        parts,
        span,
    } = &ns[0]
    else {
        panic!("{ns:?}")
    };
    assert_eq!(name, "CPP");
    assert_eq!(value.as_deref(), Some("C&#43;&#43;"));
    assert_eq!(parts, &[P::Raw("C&#43;&#43;".into())]);
    assert_eq!(*span, Span::new(0, 5));
    assert!(text(&ns).contains("*literal*"));
    assert!(!ns.iter().any(|n| matches!(n, Inline::Strong { .. })));
    assert_eq!(
        tokens(src)
            .iter()
            .filter(|t| t.kind == TokenKind::AttributeRef)
            .count(),
        5
    );
    assert!(ns.iter().any(|n| matches!(n, Inline::AttributeRef {name, value: Some(value), ..} if name == "empty" && value.is_empty())));
}

#[test]
fn document_and_host_values_override_intrinsics_without_mutating_snapshots() {
    let src = ":nbsp: literal\n\n{NBSP}\n\n:nbsp!:\n\n{nbsp}";
    let result = parse(src);
    assert_eq!(result.document.attributes.get("nbsp"), Some("literal"));
    let BlockKind::Paragraph { inlines } = &result.document.blocks[1].kind else {
        panic!()
    };
    assert_eq!(text(inlines), "&#160;");
    assert_eq!(
        result.document.blocks[1].attribute_context.get("nbsp"),
        None
    );
    let mut attributes = Attributes::new();
    attributes.set("nbsp", "locked");
    let result = parse_with_options(
        src,
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    for block in result.document.into_blocks() {
        let BlockKind::Paragraph { inlines } = block.kind else {
            panic!()
        };
        assert_eq!(text(&inlines), "locked");
    }
}

#[test]
fn reference_grammar_uses_unicode_word_characters_and_keeps_invalid_forms_literal() {
    let mut attributes = Attributes::new();
    for name in ["Ж", "a\u{301}", "a-b", "a_", "1"] {
        attributes.set(name, "yes");
    }
    for name in ["a.b", "-a", "a b", "²"] {
        attributes.set(name, "no");
    }
    let source = "{Ж} {a\u{301}} {a-b} {a_} {1} {a.b} {-a} {a b} {²}";
    let options = ParseOptions {
        attributes,
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert_eq!(text(inlines), "yes yes yes yes yes {a.b} {-a} {a b} {²}");
    assert_eq!(
        adoctra_core::tokens_with_options(source, &options)
            .iter()
            .filter(|t| t.kind == TokenKind::AttributeRef)
            .count(),
        5
    );
}

#[test]
fn missing_policies_preserve_syntax_spans_and_report_each_use() {
    for policy in ["skip", "unknown", "warn", "drop", "drop-line"] {
        let source = format!(":attribute-missing: {policy}\n\nBefore {{MISSING}} after");
        let result = parse(&source);
        let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
            panic!()
        };
        let expected = match policy {
            "drop" => "Before  after",
            "drop-line" => "",
            _ => "Before {MISSING} after",
        };
        assert_eq!(text(inlines), expected, "{policy}: {inlines:?}");
        let span = inlines
            .iter()
            .find_map(|n| match n {
                Inline::AttributeRef { span, .. } => Some(*span),
                _ => None,
            })
            .unwrap();
        assert_eq!(&source[span.start..span.end], "{MISSING}");
        assert!(tokens(&source)
            .iter()
            .any(|t| t.kind == TokenKind::AttributeRef && t.span == span));
        if matches!(policy, "warn" | "drop-line") {
            assert_eq!(result.diagnostics.len(), 1);
            assert_eq!(result.diagnostics[0].span, span);
            assert_eq!(
                result.diagnostics[0].severity,
                if policy == "warn" {
                    Severity::Warning
                } else {
                    Severity::Info
                }
            );
            assert!(result.diagnostics[0].message.ends_with(": missing"));
        } else {
            assert!(result.diagnostics.is_empty());
        }
    }
}

#[test]
fn drop_removes_only_lines_made_empty_by_missing_values() {
    for (body, expected) in [
        ("first\n{missing}\nlast", "first last"),
        ("first\n{missing}{empty}\nlast", "first last"),
        ("first\n{missing}{sp}\nlast", "first   last"),
        ("first\n*{missing}*\nlast", "first  last"),
        ("{missing}\n{missing}", ""),
    ] {
        let source = format!(":attribute-missing: drop\n\n{body}");
        assert_eq!(text(&paragraph(&source)), expected, "{source}");
    }
    // A defined empty value does not activate drop-empty-line behavior.
    assert_eq!(
        text(&paragraph(
            ":attribute-missing: drop\n\nfirst\n{empty}\nlast"
        )),
        "first  last"
    );
}

#[test]
fn drop_line_removes_transformed_lines_before_replacements_and_macro_matching() {
    let source = ":attribute-missing: drop-line\n\n[verse]\n____\nfirst\n*{missing}*\n// comment\nlast {cpp}\n____";
    let result = parse(source);
    let BlockKind::Verse { inlines, .. } = &result.document.blocks[0].kind else {
        panic!("{result:?}")
    };
    assert_eq!(text(inlines), "first\n// comment\nlast C&#43;&#43;");
    assert!(!inlines.iter().any(|n| matches!(n, Inline::Strong { .. })));
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        &source[result.diagnostics[0].span.start..result.diagnostics[0].span.end],
        "{missing}"
    );
}

#[test]
fn attribute_values_are_not_rescanned_and_generated_newlines_participate_in_drop_line() {
    let mut attributes = Attributes::new();
    attributes.set("attribute-missing", "drop-line");
    attributes.set("v", "first\nsecond");
    attributes.set("x", "{missing}");
    let options = ParseOptions {
        attributes,
        ..Default::default()
    };
    let result = parse_with_options("{v} {absent}\n{x}", &options);
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert_eq!(text(inlines), "first\n{missing}");
    assert_eq!(result.diagnostics.len(), 1);
    assert!(
        matches!(&inlines[0], Inline::AttributeRef { name, value: Some(v), span, .. } if name == "v" && v == "first\n" && *span == Span::new(0,3))
    );
}

#[test]
fn quotes_precede_attributes_and_late_metadata_expansion_preserves_boundaries() {
    let source = ":v: Value\n:x: pass:[{v}]\n\n[.{x}]#label# {asterisk}literal{asterisk} {_v_}";
    let ns = paragraph(source);
    assert!(ns.iter().any(|n| matches!(n, Inline::Substituted { parts, .. }
        if parts.iter().any(|p| matches!(p, P::OpenWithAttributes {role: Some(role), ..} if role == "Value")))));
    assert!(text(&ns).ends_with("*literal* {v}"));
    assert!(ns.iter().any(|n| matches!(n, Inline::Emphasis { .. })));
}

#[test]
fn missing_link_labels_keep_the_reference_node_after_macro_consumption() {
    let source = "link:https://example.org[{missing}]";
    let ns = paragraph(source);
    assert!(ns
        .iter()
        .any(|n| matches!(n, Inline::Substituted { value, parts, .. }
        if value == "{missing}" && matches!(&parts[0], P::OpenLink(_)))));
    assert!(ns.iter().any(
        |n| matches!(n, Inline::AttributeRef {value: Some(v), span, ..}
        if v.is_empty() && &source[span.start..span.end] == "{missing}")
    ));
}

#[test]
fn protected_escaped_and_verbatim_references_do_not_warn() {
    for body in [
        r"\{missing}",
        r"{missing\}",
        "+{missing}+",
        "pass:[{missing}]",
        "----\n{missing}\n----",
        "[literal]\n{missing}",
        "[pass]\n{missing}",
    ] {
        let source = format!(":attribute-missing: warn\n\n{body}");
        assert!(parse(&source).diagnostics.is_empty(), "{source}");
    }
}

#[test]
fn unicode_crlf_partial_input_and_long_unfinished_references_keep_valid_spans() {
    let source = "\u{feff}:attribute-missing: warn\r\n\r\nЖ \\{name} {имя\\} {MISSING} *{nbsp}*";
    struct Check<'a>(&'a str);
    impl adoctra_core::visit::VisitorMut for Check<'_> {
        fn visit_span(&mut self, s: &mut Span) {
            assert!(self.0.get(s.start..s.end).is_some(), "{s:?}");
        }
    }
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        let src = &source[..end];
        let mut result = parse(src);
        adoctra_core::visit::walk_document_mut(&mut Check(src), &mut result.document);
        for d in result.diagnostics {
            assert!(src.get(d.span.start..d.span.end).is_some());
        }
        for t in tokens(src) {
            assert!(src.get(t.span.start..t.span.end).is_some());
        }
    }
    let src = format!("{}{}", "{".repeat(10000), "a".repeat(10000));
    assert_eq!(text(&paragraph(&src)), src);
}

#[test]
fn diagnostics_cover_titles_lists_captions_and_repeated_uses_once() {
    for body in [
        "= {missing}\n\nBody",
        "== {missing}",
        ".{missing}\nBody",
        "* {missing}",
        "NOTE: {missing}",
        "[verse]\n{missing}",
    ] {
        let source = format!(":attribute-missing: warn\n\n{body}");
        let result = parse(&source);
        assert_eq!(result.diagnostics.len(), 1, "{source}: {result:?}");
        let s = result.diagnostics[0].span;
        assert_eq!(&source[s.start..s.end], "{missing}");
    }
    let result = parse(":attribute-missing: warn\n\n{missing} {missing}");
    assert_eq!(result.diagnostics.len(), 2);
    assert_ne!(result.diagnostics[0].span, result.diagnostics[1].span);
}

#[test]
fn unicode_declarations_and_unsets_use_the_same_word_grammar_as_references() {
    let source = ":a\u{301}: Accent\n:\u{301}: Mark\n\n{a\u{301}} {\u{301}}\n\n:a\u{301}!:";
    let result = parse(source);
    assert_eq!(result.document.attributes.get("a\u{301}"), Some("Accent"));
    assert_eq!(text(&paragraph(source)), "Accent Mark");
    let ts = tokens(source);
    assert_eq!(
        ts.iter()
            .filter(|t| t.kind == TokenKind::AttributeEntry)
            .count(),
        3
    );
    assert_eq!(
        ts.iter()
            .filter(|t| t.kind == TokenKind::AttributeRef)
            .count(),
        2
    );
    let source = format!("{source}\n\n{{a\u{301}}}");
    let result = parse(&source);
    let BlockKind::Paragraph { inlines } = &result.document.blocks[1].kind else {
        panic!()
    };
    assert!(matches!(
        &inlines[0],
        Inline::AttributeRef { value: None, .. }
    ));
}

#[test]
fn shared_attribute_tables_preserve_header_block_values_and_reference_spans() {
    for newline in ["\n", "\r\n"] {
        let source =
            ":v: pass:q[*Ж🙂*]\n\n{v}\n\n:v: plain\n\n{v}\n\n:v!:\n\n{v}".replace('\n', newline);
        let result = parse(&source);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let header = &result.document.attributes;
        assert_eq!(header.get("v"), Some("Ж🙂"));
        let formatted = header.raw_parts("v").unwrap();
        assert!(matches!(formatted[0], P::Open(_)));
        let blocks = result.document.flat_blocks();
        assert_eq!(blocks.len(), 3);
        for (index, (at, _)) in source.match_indices("{v}").enumerate() {
            let block = blocks[index];
            let BlockKind::Paragraph { inlines } = &block.kind else {
                panic!("{block:?}")
            };
            let [Inline::AttributeRef {
                name,
                value,
                parts,
                span,
            }] = inlines.as_slice()
            else {
                panic!("{inlines:?}")
            };
            assert_eq!(name, "v");
            assert_eq!(*span, Span::new(at, at + 3));
            assert_eq!(value.as_deref(), [Some("Ж🙂"), Some("plain"), None][index]);
            assert_eq!(block.attribute_context.get("v"), value.as_deref());
            if index == 0 {
                assert_eq!(parts, formatted);
                assert_eq!(block.attribute_context.raw_parts("v"), Some(formatted));
            } else {
                assert_eq!(block.attribute_context.raw_parts("v"), None);
            }
        }
    }
}
