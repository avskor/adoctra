use adoctra_core::{
    parse, parse_with_options, tokens, AttributeFormat as F, AttributePart as P, Attributes,
    BlockKind, Inline, ParseOptions, Severity, Span, TokenKind,
};

fn paragraph(result: &adoctra_core::ParseResult) -> &[Inline] {
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!("{result:?}")
    };
    inlines
}

fn definition(source: &str) -> (String, Vec<P>) {
    let result = parse(source);
    let attrs = &result.document.attributes;
    (
        attrs.get("v").unwrap().into(),
        attrs.raw_parts("v").unwrap_or_default().to_vec(),
    )
}

#[test]
fn definitions_share_intrinsics_protection_and_both_escape_positions() {
    let source = ":x: Ж\n:v: <b>&</b> {CPP} \\{x} {x\\} {x}\n\n{v}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    assert_eq!(
        result.document.attributes.get("v"),
        Some("<b>&</b> C&#43;&#43; {x} {x} Ж")
    );
    let parts = result.document.attributes.raw_parts("v").unwrap();
    assert!(parts.contains(&P::Text("<b>&</b> ".into())));
    assert!(parts.contains(&P::Raw("C&#43;&#43;".into())));
    let Inline::AttributeRef {
        value,
        parts: rendered,
        span,
        ..
    } = &paragraph(&result)[0]
    else {
        panic!()
    };
    assert_eq!(value.as_deref(), result.document.attributes.get("v"));
    assert_eq!(rendered, parts);
    assert_eq!(&source[span.start..span.end], "{v}");
    assert!(tokens(source)
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == *span));
}

#[test]
fn selected_stages_preserve_order_and_neutral_formatting() {
    let (value, parts) = definition(":v: pass:a,c[{cpp} {amp}]");
    assert_eq!(value, "C&#43;&#43; &");
    assert!(parts.is_empty());
    let (value, parts) = definition(":v: pass:c,a[{cpp} {amp}]");
    assert_eq!(value, "C&#43;&#43; &");
    assert_eq!(
        parts,
        vec![
            P::Raw("C&#43;&#43;".into()),
            P::Text(" ".into()),
            P::Raw("&".into())
        ]
    );
    for (stages, formatted) in [("a,q", true), ("q,a", false)] {
        let (_, parts) = definition(&format!(":v: pass:{stages}[{{asterisk}}Bold{{asterisk}}]"));
        assert_eq!(parts.contains(&P::Open(F::Strong)), formatted);
    }
    let (_, parts) = definition(":v: pass:normal[*{cpp}*]");
    assert!(parts.contains(&P::Open(F::Strong)));
    assert!(parts.contains(&P::Raw("C&#43;&#43;".into())));
}

#[test]
fn pass_without_a_keeps_references_and_generated_values_are_not_rescanned() {
    for selector in ["none", "c", "q", "r", "p", "m"] {
        let source = format!(":attribute-missing: warn\n:v: pass:{selector}[{{missing}}]\n\n{{v}}");
        let result = parse(&source);
        assert!(result.diagnostics.is_empty(), "{source}");
        assert_eq!(result.document.attributes.get("v"), Some("{missing}"));
    }
    for selector in ["a", "a,a", "a,normal"] {
        let source = format!(":x: pass:[{{cpp}}]\n:v: pass:{selector}[{{x}}]\n\n{{v}}");
        assert_eq!(definition(&source).0, "{cpp}");
    }
}

#[test]
fn definition_and_pass_policies_preserve_unknowns_or_assign_empty_values() {
    for policy in ["skip", "unknown", "warn", "drop", "drop-line"] {
        for pass in [false, true] {
            let value = if pass {
                "pass:a[before {MISSING} after]"
            } else {
                "before {MISSING} after"
            };
            let source = format!(":attribute-missing: {policy}\n:v: {value}\n\n{{v}} / {{v}}");
            let result = parse(&source);
            let expected = match policy {
                "drop" => "before  after",
                "drop-line" => "",
                _ => "before {MISSING} after",
            };
            assert_eq!(
                result.document.attributes.get("v"),
                Some(expected),
                "{source}"
            );
            if matches!(policy, "warn" | "drop-line") {
                assert_eq!(result.diagnostics.len(), 1, "{result:?}");
                let d = &result.diagnostics[0];
                assert_eq!(&source[d.span.start..d.span.end], format!(":v: {value}"));
                assert_eq!(
                    d.severity,
                    if policy == "warn" {
                        Severity::Warning
                    } else {
                        Severity::Info
                    }
                );
            } else {
                assert!(result.diagnostics.is_empty());
            }
        }
    }
}

#[test]
fn escaped_invalid_and_unicode_references_use_one_grammar() {
    let source = ":attribute-missing: warn\n:á: Accent\n:x: Value\n:v: {á} \\{missing} {missing\\} {a.b} {-x} {{x}}\n\n{v}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    assert_eq!(
        result.document.attributes.get("v"),
        Some("Accent {missing} {missing} {a.b} {-x} {Value}")
    );
}

#[test]
fn policy_assignments_use_the_previous_context_and_snapshots_remain_immutable() {
    let source = ":attribute-missing: drop\n:v: {missing}\n\n{v}\n\n:attribute-missing: warn\n:v: {missing}\n\n{v}\n\n:attribute-missing: {absent}\n:v: {missing}\n\n{v}";
    let result = parse(source);
    assert_eq!(result.document.attributes.get("v"), Some(""));
    assert_eq!(
        result.document.blocks[0].attribute_context.get("v"),
        Some("")
    );
    assert_eq!(
        result.document.blocks[1].attribute_context.get("v"),
        Some("{missing}")
    );
    assert_eq!(
        result.document.blocks[2]
            .attribute_context
            .get("attribute-missing"),
        Some("{absent}")
    );
    assert_eq!(result.diagnostics.len(), 2);
    assert!(result.diagnostics[1].message.ends_with(": absent"));
}

#[test]
fn locked_definitions_are_not_evaluated_and_host_values_override_intrinsics() {
    let source =
        ":attribute-missing: warn\n:v: pass:a[{missing}]\n:cpp: ignored\n:w: {CPP}\n\n{v} {w}";
    let mut attributes = Attributes::new();
    attributes.set("v", "locked");
    attributes.set("cpp", "host");
    let result = parse_with_options(
        source,
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.document.attributes.get("v"), Some("locked"));
    assert_eq!(result.document.attributes.get("w"), Some("host"));
}

#[test]
fn multiline_definitions_report_the_complete_physical_declaration() {
    let source = "\u{feff}:attribute-missing: drop-line\r\n:v: first + \\\r\n  {missing} + \\\r\n  last {cpp}\r\n\r\n{v}";
    let result = parse(source);
    assert_eq!(
        result.document.attributes.get("v"),
        Some("first +\nlast C&#43;&#43;")
    );
    assert_eq!(result.diagnostics.len(), 1);
    let d = &result.diagnostics[0];
    assert_eq!(
        &source[d.span.start..d.span.end],
        ":v: first + \\\r\n  {missing} + \\\r\n  last {cpp}"
    );
    let ts = tokens(source);
    assert_eq!(
        ts.iter()
            .filter(|t| t.kind == TokenKind::AttributeEntry)
            .count(),
        4
    );
}

#[test]
fn selected_pass_drops_transformed_lines_and_preserves_macro_spans() {
    for policy in ["drop", "drop-line"] {
        let source = format!(
            ":attribute-missing: {policy}\n\nBefore pass:a[first\n{{missing}}\nlast {{cpp}}] after"
        );
        let result = parse(&source);
        let Inline::Substituted { value, span, .. } = &paragraph(&result)[1] else {
            panic!("{result:?}")
        };
        assert_eq!(value, "first\nlast C&#43;&#43;");
        assert_eq!(
            &source[span.start..span.end],
            "pass:a[first\n{missing}\nlast {cpp}]"
        );
        if policy == "drop-line" {
            assert_eq!(result.diagnostics[0].span, *span);
        }
    }
}

#[test]
fn nested_passes_protect_inner_references_and_report_one_enclosing_origin() {
    let source =
        ":attribute-missing: warn\n\nBefore pass:m,a[+{missing}+ pass:a[{missing}\\]] after";
    let result = parse(source);
    assert_eq!(result.diagnostics.len(), 1);
    let span = result.diagnostics[0].span;
    assert_eq!(
        &source[span.start..span.end],
        r"pass:m,a[+{missing}+ pass:a[{missing}\]]"
    );
    assert!(result.diagnostics[0].message.ends_with(": missing"));
}

#[test]
fn late_attribute_stage_preserves_quote_and_link_metadata() {
    let (value, parts) = definition(":x: Value\n:r: pass:[{x}]\n:v: pass:q,a[[.{r}]#Label#]");
    assert_eq!(value, "Label");
    assert!(parts
        .iter()
        .any(|p| matches!(p, P::OpenWithAttributes {role: Some(role), ..} if role == "Value")));
    let (_, parts) =
        definition(":x: Value\n:r: pass:[{x}]\n:v: pass:m,a[link:https://e/{r}[Label,role={r}]]");
    assert!(parts.iter().any(|p| matches!(p, P::OpenLink(link) if link.target == "https://e/{x}" && link.role.as_deref() == Some("{x}"))));
}

#[test]
fn every_unicode_prefix_keeps_ast_diagnostic_and_token_positions_valid() {
    let source = "\u{feff}:attribute-missing: warn\r\n:v: pass:normal[*{missing}* {cpp} \\\r\n{missing\\}]\r\n\r\nЖ pass:a[{missing}] {v}";
    struct Check<'a>(&'a str);
    impl adoctra_core::visit::VisitorMut for Check<'_> {
        fn visit_span(&mut self, s: &mut Span) {
            assert!(self.0.get(s.start..s.end).is_some(), "{s:?}");
        }
    }
    for end in (0..=source.len()).filter(|&n| source.is_char_boundary(n)) {
        let src = &source[..end];
        let mut result = parse(src);
        adoctra_core::visit::walk_document_mut(&mut Check(src), &mut result.document);
        for d in result.diagnostics {
            assert!(src.get(d.span.start..d.span.end).is_some(), "{d:?}");
        }
        for t in tokens(src) {
            assert!(src.get(t.span.start..t.span.end).is_some());
        }
    }
}
