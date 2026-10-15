use adoctra_core::{
    parse, parse_with_options, tokens, AttributePart as P, BlockKind, Inline, ParseOptions,
    Severity, SourceId, Span, TokenKind,
};

fn inlines(result: &adoctra_core::ParseResult) -> &[Inline] {
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!("{result:?}")
    };
    inlines
}
fn parts(result: &adoctra_core::ParseResult) -> Vec<P> {
    inlines(result)
        .iter()
        .flat_map(|n| match n {
            Inline::Substituted { parts, .. } | Inline::AttributeRef { parts, .. } => parts.clone(),
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn eager_quote_warning_retains_reference_span_and_late_stage_warning() {
    let source = ":attribute-missing: warn\n\nЖ(C) [.{missing}]*Text*";
    let result = parse_with_options(
        source,
        &ParseOptions {
            source_id: SourceId(7),
            ..Default::default()
        },
    );
    assert_eq!(result.diagnostics.len(), 2, "{result:?}");
    let start = source.find("{missing}").unwrap();
    assert_eq!(
        result.diagnostics[0].span,
        Span::in_source(SourceId(7), start, start + 9)
    );
    assert_eq!(
        &source[result.diagnostics[1].span.start..result.diagnostics[1].span.end],
        "[.{missing}]*"
    );
    assert!(result
        .diagnostics
        .iter()
        .all(|d| d.severity == Severity::Warning));
    assert!(parts(&result)
        .iter()
        .any(|p| matches!(p,P::OpenWithAttributes{role:Some(role),..} if role=="{missing}")));
    assert!(tokens(source)
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef
            && &source[t.span.start..t.span.end] == "{missing}"));
}

#[test]
fn drop_line_in_quote_metadata_drops_metadata_not_surrounding_text() {
    for (policy, decorated) in [("skip", true), ("drop", true), ("drop-line", false)] {
        let source =
            format!(":attribute-missing: {policy}\n\nBefore [.red{{missing}}]*Text* after");
        let result = parse(&source);
        assert_eq!(
            parts(&result)
                .iter()
                .any(|p| matches!(p, P::OpenWithAttributes { .. })),
            decorated
        );
        assert_eq!(result.diagnostics.len(), usize::from(policy == "drop-line"));
        if policy == "drop-line" {
            assert_eq!(result.diagnostics[0].severity, Severity::Info);
            assert_eq!(
                &source[result.diagnostics[0].span.start..result.diagnostics[0].span.end],
                "{missing}"
            );
            assert!(inlines(&result)
                .iter()
                .any(|n| matches!(n, Inline::Strong { .. })));
        }
        assert!(inlines(&result)
            .iter()
            .any(|n| matches!(n,Inline::Text{value,..} if value.contains("Before"))));
    }
}

#[test]
fn passthrough_metadata_reports_once_at_its_enclosing_origin() {
    for body in [
        "[.{missing}]+Text+",
        "[.{missing}]++Text++",
        "[.{missing} x-]+Text+",
    ] {
        let source = format!(":attribute-missing: warn\n\n{body}");
        let result = parse(&source);
        assert_eq!(result.diagnostics.len(), 1, "{source}: {result:?}");
        let span = result.diagnostics[0].span;
        assert_eq!(&source[span.start..span.end], body);
        assert!(tokens(&source).iter().any(|t| matches!(
            t.kind,
            TokenKind::PassthroughText | TokenKind::Monospace
        ) && t.span == span));
    }
}

#[test]
fn selected_quote_stages_and_definitions_keep_enclosing_physical_origins() {
    for (selector, count) in [("q", 1), ("q,a", 2), ("normal", 2)] {
        let body = format!("pass:{selector}[[.{{missing}}\\]*Text*]");
        let source = format!(":attribute-missing: warn\n\n{body}");
        let result = parse(&source);
        assert_eq!(result.diagnostics.len(), count, "{source}: {result:?}");
        for d in result.diagnostics {
            assert_eq!(&source[d.span.start..d.span.end], body);
        }
        let entry = format!(":v: pass:{selector}[[.{{missing}}]*Text*]");
        let source = format!(":attribute-missing: warn\n{entry}\n\n{{v}}");
        let result = parse(&source);
        assert_eq!(result.diagnostics.len(), count, "{source}: {result:?}");
        for d in result.diagnostics {
            assert_eq!(&source[d.span.start..d.span.end], entry);
        }
    }
}

#[test]
fn quote_metadata_uses_shared_unicode_intrinsics_and_escape_grammar() {
    let result = parse(":á: accent\n\n[.{á}.{cpp}]+Text+");
    assert!(result.diagnostics.is_empty());
    assert!(parts(&result)
        .iter()
        .any(|p| matches!(p,P::OpenWithAttributes{role:Some(role),..} if role=="accent C&")));
    for body in [r"[.\{missing}]+Text+", r"[.{missing\}]+Text+"] {
        let result = parse(&format!(":attribute-missing: warn\n\n{body}"));
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert!(parts(&result)
            .iter()
            .any(|p| matches!(p,P::OpenWithAttributes{role:Some(role),..} if role=="{missing}")));
    }
}

#[test]
fn unused_comma_fields_warn_but_escaped_or_unmatched_metadata_does_not() {
    let result = parse(":attribute-missing: warn\n\n[.red,ignored={missing}]*Text*");
    assert_eq!(result.diagnostics.len(), 1);
    for body in [
        r"\[.{missing}]*Text*",
        "[.{missing}]*Text",
        "pass:[[{missing}]*Text*]",
    ] {
        let result = parse(&format!(":attribute-missing: warn\n\n{body}"));
        // Unmatched/escaped source metadata still reaches the ordinary a stage;
        // protected passthrough contents do not.
        let expected = usize::from(!body.starts_with("pass:"));
        assert_eq!(result.diagnostics.len(), expected, "{body}: {result:?}");
    }
}

#[test]
fn repeated_link_attributes_report_each_executed_stage() {
    for (label, count) in [(r"\{missing}", 1), ("{missing}", 2)] {
        let body = format!("link:https://e['{label}',role=x]");
        let source = format!(":attribute-missing: warn\n\n{body}");
        let result = parse(&source);
        assert_eq!(result.diagnostics.len(), count, "{source}: {result:?}");
        let last = result.diagnostics.last().unwrap().span;
        assert_eq!(&source[last.start..last.end], body);
        assert!(parts(&result).contains(&P::Text("{missing}".into())));
        assert!(tokens(&source)
            .iter()
            .any(|t| t.kind == TokenKind::Link && t.span == last));
    }
}

#[test]
fn missing_policy_in_single_quoted_label_keeps_the_link() {
    for policy in ["drop", "drop-line"] {
        let body = r"link:https://e['\{missing}',role=x]";
        let source = format!(":attribute-missing: {policy}\n\nBefore {body} after");
        let result = parse(&source);
        let ps = parts(&result);
        assert!(ps.iter().any(|p|matches!(p,P::OpenLink(link) if link.target=="https://e" && link.role.as_deref()==Some("bare x"))));
        assert!(!ps.iter().any(|p| p.text().contains("{missing}")));
        assert_eq!(result.diagnostics.len(), usize::from(policy == "drop-line"));
        if let Some(d) = result.diagnostics.first() {
            assert_eq!(d.severity, Severity::Info);
            assert_eq!(&source[d.span.start..d.span.end], body);
        }
    }
}

#[test]
fn named_title_reftext_and_options_skip_repeated_substitution() {
    for name in ["title", "reftext", "opts", "options"] {
        let source =
            format!(":attribute-missing: warn\n\nlink:https://e[Label,{name}='\\{{missing}}']");
        let result = parse(&source);
        assert!(result.diagnostics.is_empty(), "{source}: {result:?}");
        if name == "title" {
            assert!(parts(&result).iter().any(
                |p| matches!(p,P::OpenLink(link) if link.title.as_deref()==Some("{missing}"))
            ));
        }
    }
    let result = parse(":attribute-missing: warn\n\nlink:https://e[Label,role='\\{missing}']");
    assert_eq!(result.diagnostics.len(), 1);
}

#[test]
fn transformed_link_inputs_use_enclosing_source_or_pass_span() {
    for (body, origin) in [
        (
            "link:https://e['{v}',role=x]",
            "link:https://e['{v}',role=x]",
        ),
        (
            "pass:m[link:https://e['{missing}',role=x\\]]",
            "pass:m[link:https://e['{missing}',role=x\\]]",
        ),
    ] {
        let source = format!(":attribute-missing: warn\n:v: pass:[{{missing}}]\n\n{body}");
        let result = parse(&source);
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        let span = result.diagnostics[0].span;
        assert_eq!(&source[span.start..span.end], origin);
    }
}

#[test]
fn body_snapshots_change_missing_policy_without_mutating_earlier_blocks() {
    let source=":attribute-missing: warn\n\n[.{missing}]+First+\n\n:attribute-missing: drop-line\n\n[.{missing}]+Second+";
    let result = parse(source);
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|d| d.severity)
            .collect::<Vec<_>>(),
        vec![Severity::Warning, Severity::Info]
    );
    assert_eq!(
        result.document.blocks[0]
            .attribute_context
            .get("attribute-missing"),
        Some("warn")
    );
    assert_eq!(
        result.document.blocks[1]
            .attribute_context
            .get("attribute-missing"),
        Some("drop-line")
    );
}

#[test]
fn incomplete_unicode_metadata_and_link_parameters_keep_valid_spans() {
    let source=":attribute-missing: warn\r\n\r\nЖ(C) [.{missing}]*Текст* [.роль{other}]+Ж+ link:https://e['\\{unknown}',role=x]";
    struct Bounds<'a>(&'a str);
    impl adoctra_core::visit::VisitorMut for Bounds<'_> {
        fn visit_span(&mut self, span: &mut Span) {
            assert!(self.0.get(span.start..span.end).is_some(), "{span:?}");
        }
    }
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        let src = &source[..end];
        let mut result = parse(src);
        adoctra_core::visit::walk_document_mut(&mut Bounds(src), &mut result.document);
        for d in result.diagnostics {
            assert!(src.get(d.span.start..d.span.end).is_some(), "{d:?}");
        }
        for token in tokens(src) {
            assert!(
                src.get(token.span.start..token.span.end).is_some(),
                "{token:?}"
            );
        }
    }
}
