use adoctra_core::{
    parse, parse_with_options, AttributeFormat as F, AttributePart as P, Attributes, BlockKind,
    Inline, ParseOptions, Severity, Span,
};

fn paragraph(source: &str) -> Vec<Inline> {
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let BlockKind::Paragraph { inlines } = result.document.blocks[0].kind.clone() else {
        panic!()
    };
    inlines
}
fn parts(nodes: &[Inline]) -> Vec<P> {
    nodes
        .iter()
        .flat_map(|node| match node {
            Inline::Text { value, .. } => vec![P::Text(value.clone())],
            Inline::Raw { value, .. } => vec![P::Raw(value.clone())],
            Inline::Substituted { parts, .. } | Inline::AttributeRef { parts, .. } => parts.clone(),
            _ => panic!("{node:?}"),
        })
        .collect()
}

#[test]
fn normal_and_verbatim_groups_preserve_neutral_parts_and_source_ranges() {
    for selector in ["normal", "n"] {
        let source = format!(":v: Ж\n\nBefore pass:{selector}[*{{v}}* & (C)] after");
        let nodes = paragraph(&source);
        let Inline::Substituted { value, parts, span } = &nodes[1] else {
            panic!("{nodes:?}")
        };
        assert!(value.contains('Ж'));
        assert!(parts.contains(&P::Open(F::Strong)));
        assert!(parts.contains(&P::Raw("&#169;".into())));
        assert_eq!(
            &source[span.start..span.end],
            format!("pass:{selector}[*{{v}}* & (C)]")
        );
    }
    for selector in ["verbatim", "v", "specialchars"] {
        let source = format!("Before pass:{selector}[*Bold* {{v}} & <1>] after");
        assert!(parts(&paragraph(&source)).contains(&P::Text("*Bold* {v} & <1>".into())));
    }
}

#[test]
fn ordered_groups_removals_and_duplicates_follow_the_resolver() {
    let ps = parts(&paragraph(
        ":v: Ж\n\nBefore pass:normal,-quotes[*{v}* (C)] after",
    ));
    assert!(!ps.contains(&P::Open(F::Strong)));
    assert!(ps.contains(&P::Text("*Ж* ".into())));
    let ps = parts(&paragraph("Before pass:q,n[*Bold* &] after"));
    assert!(!ps.contains(&P::Open(F::Strong)));
    assert!(ps
        .iter()
        .any(|p| matches!(p, P::Text(s) if s.contains("<strong>Bold</strong>"))));
    let ps = parts(&paragraph("Before pass:c,c,q,q[*Bold* &] after"));
    assert_eq!(ps.iter().filter(|p| **p == P::Open(F::Strong)).count(), 1);
    assert!(parse("Before pass:bad,-bad,q[*Bold*] after")
        .diagnostics
        .is_empty());
}

#[test]
fn invalid_stages_warn_at_the_original_selector_and_keep_valid_stages() {
    let source = "\u{feff}Before pass:basic,q,basic[*Ж*] after";
    let result = parse(source);
    assert_eq!(result.diagnostics.len(), 1);
    let d = &result.diagnostics[0];
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(&source[d.span.start..d.span.end], "basic,q,basic");
    assert_eq!(
        d.message,
        "invalid substitution types for passthrough macro: basic, basic"
    );
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert!(parts(inlines).contains(&P::Open(F::Strong)));
}

#[test]
fn escaped_unrecognized_and_verbatim_text_do_not_emit_substitution_warnings() {
    for source in [
        r"Before \pass:bad[*Bold*] after",
        "Before pass:q,,bad[*Bold*] after",
        "Before pass:-bad[*Bold*] after",
        "----\npass:bad[x]\n----",
        "[literal]\npass:bad[x]",
        "[pass]\npass:bad[x]",
        "[source]\npass:bad[x]",
        "// pass:bad[x]",
    ] {
        assert!(parse(source).diagnostics.is_empty(), "{source}");
    }
}

#[test]
fn nested_extraction_precedes_all_selected_stages_when_macros_is_enabled() {
    let source = r"Before pass:q,m[+*Literal* {v}+ *Bold* pass:c[<b>x</b>\]] after";
    let ps = parts(&paragraph(source));
    assert!(ps.contains(&P::Text("*Literal* {v}".into())));
    assert!(ps.contains(&P::Text("<b>x</b>".into())));
    assert_eq!(ps.iter().filter(|p| **p == P::Open(F::Strong)).count(), 1);
    let ps = parts(&paragraph("Before pass:n,-m[+*Bold*+] after"));
    assert!(ps.contains(&P::Open(F::Strong)));
}

#[test]
fn post_replacements_generate_line_breaks_and_allow_later_literal_protection() {
    let ps = parts(&paragraph("Before pass:n[One +\nTwo +] after"));
    assert_eq!(ps.iter().filter(|p| **p == P::LineBreak).count(), 2);
    assert!(!ps
        .iter()
        .any(|p| matches!(p, P::Raw(s) if s.contains("<br>"))));
    let ps = parts(&paragraph("Before pass:p,c[One +\nTwo] after"));
    assert!(!ps.contains(&P::LineBreak));
    assert!(ps.contains(&P::Text("One<br>\nTwo".into())));
    let ps = parts(&paragraph(
        ":hardbreaks-option:\n\nBefore pass:n[One\nTwo +] after",
    ));
    assert_eq!(ps.iter().filter(|p| **p == P::LineBreak).count(), 1);
    assert!(ps
        .iter()
        .any(|p| matches!(p, P::Text(s) if s.ends_with("Two +"))));
}

#[test]
fn whole_value_groups_warn_once_at_definition_and_respect_host_locks() {
    let source = ":v: pass:bad,n[*Ж*]\n\n{v} {v}";
    let result = parse(source);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        &source[result.diagnostics[0].span.start..result.diagnostics[0].span.end],
        ":v: pass:bad,n[*Ж*]"
    );
    assert!(result
        .document
        .attributes
        .raw_parts("v")
        .unwrap()
        .contains(&P::Open(F::Strong)));
    let mut attributes = Attributes::new();
    attributes.set("v", "locked");
    assert!(parse_with_options(
        source,
        &ParseOptions {
            attributes,
            ..Default::default()
        }
    )
    .diagnostics
    .is_empty());
}

#[test]
fn nested_diagnostics_and_depth_limit_keep_real_enclosing_spans() {
    let source = r"Before pass:m[pass:bad[Ж\]] after";
    let result = parse(source);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        &source[result.diagnostics[0].span.start..result.diagnostics[0].span.end],
        r"pass:m[pass:bad[Ж\]]"
    );
    let mut nested = "Ж".to_owned();
    for _ in 0..40 {
        nested = format!("pass:m[{}]", nested.replace(']', r"\]"));
    }
    let result = parse(&format!("Before {nested} after"));
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("nesting limit")));
    assert!(!format!("{:?}", result.document).contains('\u{96}'));
}

#[test]
fn diagnostics_in_titles_lists_and_verse_are_not_lost_or_duplicated() {
    for source in [
        "= pass:bad[Title]\n\nBody",
        "== pass:bad[Title]",
        ".pass:bad[Caption]\nBody",
        "* pass:bad[Item]",
        "[verse]\npass:bad[Line]",
        "[verse]\n____\npass:bad[Line]\n____",
        "NOTE: pass:bad[Body]",
    ] {
        let result = parse(source);
        assert_eq!(
            result.diagnostics.len(),
            1,
            "{source}: {:?}",
            result.diagnostics
        );
        let span = result.diagnostics[0].span;
        assert_eq!(&source[span.start..span.end], "bad");
    }
}

#[test]
fn every_unicode_prefix_keeps_diagnostic_and_ast_spans_in_bounds() {
    let source =
        "\u{feff}:v: Ж\r\n\r\nBefore pass:bad,n[*{v}* +\r\n// removed\r\npass:bad[Ж\\]] after";
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
            assert!(src.get(d.span.start..d.span.end).is_some(), "{d:?}");
        }
    }
}

#[test]
fn post_replacements_recognize_physical_crlf_without_shifting_spans() {
    let source = "\u{feff}Before pass:normal[Ж +\r\nNext] after";
    let nodes = paragraph(source);
    let Inline::Substituted { parts, span, .. } = &nodes[1] else {
        panic!("{nodes:?}")
    };
    assert!(parts.contains(&P::LineBreak));
    assert_eq!(&source[span.start..span.end], "pass:normal[Ж +\r\nNext]");
}
