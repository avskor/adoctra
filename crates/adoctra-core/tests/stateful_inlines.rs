use adoctra_core::{
    parse, parse_with_options, tokens, tokens_with_options, AttributePart as P, Attributes, Block,
    BlockKind, Inline, ParseOptions, Severity, SourceId, Span, TokenKind,
};

fn inlines(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines }
        | BlockKind::Heading { inlines, .. }
        | BlockKind::Verse { inlines, .. } => inlines,
        _ => panic!("{block:?}"),
    }
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
            Inline::Strong { children, .. } | Inline::Link { children, .. } => text(children),
            _ => panic!("{n:?}"),
        })
        .collect()
}

#[test]
fn body_executes_once_without_mutating_reading_snapshots() {
    let source = ":n: 8\n\n{counter:n} {set:x:yes}{x}\n\n{counter:n} {x}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    assert_eq!(text(inlines(result.document.flat_blocks()[0])), "9 yes");
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "10 yes");
    assert_eq!(result.document.attributes.get("n"), Some("8"));
    assert_eq!(result.document.attributes.get("x"), None);
    for block in &result.document.flat_blocks() {
        assert_eq!(block.attribute_context.get("n"), Some("8"));
        assert_eq!(block.attribute_context.get("x"), None);
    }
    let before = format!("{result:?}");
    assert_eq!(tokens(source), tokens(source));
    assert_eq!(before, format!("{:?}", parse(source)));
}

#[test]
fn declarations_replay_even_when_equal_to_the_previous_reading_value() {
    let result = parse(":n: 1\n\n{counter:n}\n\n:n: 1\n\n{counter:n}\n\n:n!:\n\n{counter:n}");
    let values: Vec<_> = result
        .document
        .flat_blocks()
        .iter()
        .map(|b| text(inlines(b)))
        .collect();
    assert_eq!(values, ["2", "2", "1"]);
}

#[test]
fn definitions_and_headings_finish_reading_before_body_execution() {
    let result = parse(":first: {counter:n}\n\n{counter:n} {set:x:yes}\n\n:second: {counter:n}\n\n== Head {n} {x}\n\n{second} {counter:n} {x}\n\nifdef::x[]\nWrong\nendif::[]");
    let values: Vec<_> = result
        .document
        .flat_blocks()
        .iter()
        .map(|b| text(inlines(b)))
        .collect();
    assert_eq!(values, ["2 ", "Head 2 {x}", "2 3 yes"]);
}

#[test]
fn locked_counter_registry_survives_reading_and_body_set_respects_locks() {
    let mut attributes = Attributes::new();
    attributes.set("n", "8");
    attributes.set("x", "host");
    let source = ":first: {counter:n}\n\n{counter:n} {set:x:{counter:n}}{x}\n\n:second: {counter:n}\n\n{counter:n} {n} {second}";
    let result = parse_with_options(
        source,
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    // Both definitions execute before the first body counter; a locked set
    // does not evaluate its value, and the first brace closes its expression.
    assert_eq!(text(inlines(result.document.flat_blocks()[0])), "11 }host");
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "12 8 10");
}

#[test]
fn late_passes_share_state_and_dropped_slots_never_execute() {
    let result = parse("pass:a[{counter:n}] {counter:n} pass:a[{counter:n}]\n\n{n}");
    assert_eq!(text(inlines(result.document.flat_blocks()[0])), "2 1 3");
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "3");
    let result = parse(
        ":attribute-missing: drop-line\n\npass:a[{counter:n}] {missing}\nkept {counter:n}\n\n{n}",
    );
    assert_eq!(text(inlines(result.document.flat_blocks()[0])), "kept 1");
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "1");
}

#[test]
fn repeated_link_parameters_and_quote_metadata_execute_in_the_shared_phase() {
    let source = "Before [.{counter:n}]*{counter:n}* link:page['\\{counter:n}',role=x]\n\n{n}";
    let result = parse(source);
    let debug = format!("{:?}", inlines(result.document.flat_blocks()[0]));
    assert!(debug.contains("role: Some(\"1\")"), "{debug}");
    assert_eq!(
        text(inlines(result.document.flat_blocks()[0])),
        "Before 2 3"
    );
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "3");
    let highlighted = tokens(source);
    for (at, _) in source.match_indices("{counter:n}").take(2) {
        assert!(highlighted.iter().any(|t| t.kind == TokenKind::AttributeRef
            && t.span == Span::new(at, at + "{counter:n}".len())));
    }
    let result = parse("link:pass:a[{counter:n}][]\n\n{n}");
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "2");
    assert!(format!("{:?}", inlines(result.document.flat_blocks()[0])).contains("target: \"1\""));
}

#[test]
fn filtering_keeps_executed_effects_and_caches_missing_policy_per_stage() {
    let result = parse("{set:x}before{set:x!}{counter:n}\nkept {counter:n}\n\n{n}");
    assert_eq!(text(inlines(result.document.flat_blocks()[0])), "kept 2");
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "2");
    let result = parse("{absent}{set:attribute-missing:drop}{missing}\n\n{missing}end");
    assert_eq!(
        text(inlines(result.document.flat_blocks()[0])),
        "{absent}{missing}"
    );
    assert_eq!(text(inlines(result.document.flat_blocks()[1])), "end");
}

#[test]
fn verse_and_list_continuations_share_state_without_reexecuting_discarded_scans() {
    let empty = parse("[verse]\n____\n____\n\n[verse]\n____\n____\n\n{counter:n}");
    assert!(inlines(empty.document.flat_blocks()[0]).is_empty());
    assert!(inlines(empty.document.flat_blocks()[1]).is_empty());
    assert_eq!(text(inlines(empty.document.flat_blocks()[2])), "1");
    let result = parse(
        "[verse]\n{counter:n}\n{counter:n}\n\n* {counter:n}\n+\n{counter:n}\n* {counter:n}\n\n{n}",
    );
    assert_eq!(text(inlines(result.document.flat_blocks()[0])), "1\n2");
    let BlockKind::List { items, .. } = &result.document.flat_blocks()[1].kind else {
        panic!()
    };
    assert_eq!(text(&items[0].inlines), "3");
    assert_eq!(text(inlines(&items[0].blocks[0])), "4");
    assert_eq!(text(&items[1].inlines), "5");
    assert_eq!(text(inlines(result.document.flat_blocks()[2])), "5");
}

#[test]
fn raw_protection_escapes_and_verbatim_blocks_do_not_execute() {
    let source = "\\{counter:n} {counter:n\\} +{counter:n}+ pass:[{counter:n}]\n\n[literal]\n{counter:n}\n\n[pass]\n{counter:n}\n\n[comment]\n{counter:n}\n\n{counter:n}";
    let result = parse(source);
    assert_eq!(
        text(inlines(result.document.flat_blocks().last().unwrap())),
        "1"
    );
    assert!(result.diagnostics.is_empty());
}

#[test]
fn references_diagnostics_and_tokens_keep_physical_unicode_crlf_spans() {
    let source =
        "Ж {set:attribute-missing:warn}{counter:n}\r\n// skipped\r\nБ {missing} {counter:n}";
    let options = ParseOptions {
        source_id: SourceId(7),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.severity, Severity::Warning);
    assert_eq!(diagnostic.span.source_id, SourceId(7));
    assert_eq!(
        &source[diagnostic.span.start..diagnostic.span.end],
        "{missing}"
    );
    let tokens = tokens_with_options(source, &options);
    for (at, _) in source.match_indices("{counter:n}") {
        let span = Span::in_source(SourceId(7), at, at + "{counter:n}".len());
        assert!(inlines(result.document.flat_blocks()[0])
            .iter()
            .any(|n| matches!(n,
            Inline::AttributeRef { name, span: s, parts, .. }
            if name == "counter:n" && *s == span && parts.iter().any(|p| matches!(p, P::Raw(_))))));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::AttributeRef && t.span == span));
    }
}

#[test]
fn stateful_values_keep_neutral_formatting_and_unbounded_counters() {
    let result =
        parse("{set:x:*Bold*}{x} {counter:n:999999999999999999999999999999}\n\n{counter:n}");
    assert_eq!(
        text(inlines(result.document.flat_blocks()[1])),
        "1000000000000000000000000000000"
    );
    assert_eq!(
        text(inlines(result.document.flat_blocks()[0])),
        "*Bold* 999999999999999999999999999999"
    );
    assert!(inlines(result.document.flat_blocks()[1]).iter().any(|n| matches!(n, Inline::AttributeRef { parts, .. } if parts.iter().any(|p| matches!(p, P::Raw(_))))));
}

#[test]
fn incomplete_expressions_and_deep_passes_remain_bounded() {
    let source = "Ж {counter:n} {set:x:yes} pass:n[pass:a[{counter:n}]]";
    for (end, _) in source.char_indices() {
        let input = &source[..end];
        let result = parse(input);
        for diagnostic in &result.diagnostics {
            assert!(diagnostic.span.end <= input.len());
        }
        tokens(input);
    }
    let mut nested = "{counter:n}".to_owned();
    for _ in 0..40 {
        nested = format!("pass:m[{}]", nested.replace(']', r"\]"));
    }
    let result = parse(&nested);
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("nesting limit")));
    assert!(!format!("{result:?}").contains('\u{96}'));
}
