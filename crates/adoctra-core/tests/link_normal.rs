use adoctra_core::{
    parse, parse_with_options, tokens, AttributeFormat as F, AttributePart as P, Attributes,
    BlockKind, Inline, ParseOptions, Severity, SourceId, Span, TokenKind,
};

fn parts(result: &adoctra_core::ParseResult) -> Vec<P> {
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!("{result:?}")
    };
    inlines
        .iter()
        .flat_map(|n| match n {
            Inline::Substituted { parts, .. } | Inline::AttributeRef { parts, .. } => parts.clone(),
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn repeated_normal_extracts_before_quotes_and_restores_after_post() {
    for (value, expected) in [
        ("+*bold*+", P::Text("*bold*".into())),
        ("+++<b>raw</b>+++", P::Raw("<b>raw</b>".into())),
        (r"pass:[{missing}\]", P::Raw("{missing}".into())),
        ("+end ++", P::Text("end +".into())),
    ] {
        let source = format!(
            ":attribute-missing: warn\n:v: pass:[{value}]\n\nlink:https://e['{{v}}',role=x]"
        );
        let result = parse_with_options(
            &source,
            &ParseOptions {
                source_id: SourceId(7),
                ..Default::default()
            },
        );
        assert!(result.diagnostics.is_empty(), "{source}: {result:?}");
        let ps = parts(&result);
        assert!(ps.contains(&expected), "{source}: {ps:?}");
        assert!(!ps.contains(&P::Open(F::Strong)));
        assert!(!ps.contains(&P::LineBreak));
        let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
            unreachable!()
        };
        assert!(
            inlines
                .iter()
                .any(|n| matches!(n, Inline::Substituted { span, parts, .. }
            if parts.iter().any(|p| matches!(p, P::OpenLink(_)))
            && *span == Span::in_source(SourceId(7), source.find("link:").unwrap(), source.len())))
        );
        let ts = tokens(&source);
        assert!(ts.iter().any(|t| t.kind == TokenKind::Link));
        assert!(ts.iter().any(
            |t| t.kind == TokenKind::AttributeRef && &source[t.span.start..t.span.end] == "{v}"
        ));
        assert!(!ts.iter().any(|t| t.kind == TokenKind::PassthroughText));
    }
}

#[test]
fn repeated_post_emits_neutral_break_and_respects_hardbreaks_final_line() {
    for (header, breaks) in [("", 1), (":hardbreaks:\n\n", 0)] {
        let result = parse(&format!("{header}link:https://e['end +',role=x]"));
        assert_eq!(
            parts(&result)
                .iter()
                .filter(|p| **p == P::LineBreak)
                .count(),
            breaks
        );
    }
    let result = parse("link:https://e[Label,role='end +',title='end +']");
    assert!(parts(&result).iter().any(|p| matches!(p, P::OpenLink(l)
        if l.role.as_deref() == Some("end<br>") && l.title.as_deref() == Some("end +"))));
}

#[test]
fn host_newlines_reach_post_after_attribute_list_folding() {
    let mut attributes = Attributes::new();
    attributes.set("v", "one +\ntwo");
    let source = "link:https://e['\\{v}',role=x]";
    let result = parse_with_options(
        source,
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    assert!(parts(&result).contains(&P::LineBreak), "{result:?}");
    assert!(!tokens(source)
        .iter()
        .any(|t| t.kind == TokenKind::LineBreak));
}

#[test]
fn nested_parameter_pass_diagnostics_use_the_enclosing_link_span() {
    let body = r"link:https://e['\pass:bad[Ж\]',role=x]";
    let source = format!("Ж(C) {body}");
    let result = parse_with_options(
        &source,
        &ParseOptions {
            source_id: SourceId(9),
            ..Default::default()
        },
    );
    assert_eq!(result.diagnostics.len(), 1, "{result:?}");
    let d = &result.diagnostics[0];
    assert_eq!(d.severity, Severity::Warning);
    assert!(d.message.contains("invalid substitution type"));
    assert_eq!(
        d.span,
        Span::in_source(SourceId(9), source.find("link:").unwrap(), source.len())
    );
}

#[test]
fn selected_and_definition_normal_passes_share_the_parameter_pipeline() {
    for source in [
        "pass:m[link:https://e['end +',role=x\\]]",
        ":v: pass:m[link:https://e['end +',role=x]]\n\n{v}",
    ] {
        let result = parse(source);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert!(parts(&result).contains(&P::LineBreak));
    }
}

#[test]
fn shared_slots_restore_source_passthroughs_inside_repeated_fields() {
    let source = "++before++ link:https://e['pass:q[*bold*]',role=x] ++after++";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    let ps = parts(&result);
    assert!(ps.contains(&P::Open(F::Strong)), "{ps:?}");
    assert!(!format!("{result:?}").contains('\u{96}'));
    assert_eq!(
        tokens(source)
            .iter()
            .filter(|t| t.kind == TokenKind::PassthroughText)
            .count(),
        3
    );
}

#[test]
fn mixed_pass_and_link_recursion_shares_one_limit() {
    for value in [
        "link:page['{cycle}',title=T]",
        "link:page['pass:n[{cycle}\\]',title=T]",
    ] {
        let mut attributes = Attributes::new();
        attributes.set("cycle", value);
        let source = r"link:page['\{cycle}',title=T]";
        let result = parse_with_options(
            source,
            &ParseOptions {
                attributes,
                ..Default::default()
            },
        );
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert!(result.diagnostics[0].message.contains("nesting limit (32)"));
        assert_eq!(result.diagnostics[0].span, Span::new(0, source.len()));
        let ps = parts(&result);
        let links = ps.iter().filter(|p| matches!(p, P::OpenLink(_))).count();
        assert!(links <= 33, "{links}");
        assert!(ps.iter().any(|p| p.text().contains("{cycle}")));
        assert!(!format!("{result:?}").contains('\u{96}'));
    }
}

#[test]
fn incomplete_unicode_parameters_keep_all_spans_in_bounds() {
    let source = "Ж(C) link:https://e['\\pass:bad[Ж\\] +',role=x]\r\nlast";
    struct Bounds<'a>(&'a str);
    impl adoctra_core::visit::VisitorMut for Bounds<'_> {
        fn visit_span(&mut self, span: &mut Span) {
            assert!(self.0.get(span.start..span.end).is_some(), "{span:?}");
        }
    }
    for end in (0..=source.len()).filter(|&end| source.is_char_boundary(end)) {
        let src = &source[..end];
        let mut result = parse(src);
        adoctra_core::visit::walk_document_mut(&mut Bounds(src), &mut result.document);
        for d in result.diagnostics {
            assert!(src.get(d.span.start..d.span.end).is_some());
        }
        for t in tokens(src) {
            assert!(src.get(t.span.start..t.span.end).is_some());
        }
    }
}

#[test]
fn nested_pass_closers_are_unescaped_once_before_attribute_list_parsing() {
    let result = parse(":v: pass:[pass:m[pass:q[*bold*\\\\]\\]]\n\nlink:https://e['{v}',role=x]");
    assert!(result.diagnostics.is_empty(), "{result:?}");
    assert!(parts(&result).contains(&P::Open(F::Strong)), "{result:?}");
    assert!(parts(&result).contains(&P::Raw("bold".into())));
}
