use adoctra_core::{
    parse, parse_with_options, tokens, AttributeFormat as F, AttributePart as P, Attributes,
    BlockKind, Inline, ParseOptions, Severity, SourceId, Span, TokenKind,
};

fn text(block: &adoctra_core::Block) -> String {
    let nodes = match &block.kind {
        BlockKind::Paragraph { inlines } | BlockKind::Heading { inlines, .. } => inlines,
        _ => panic!("{block:?}"),
    };
    nodes
        .iter()
        .map(|n| match n {
            Inline::Text { value, .. }
            | Inline::Raw { value, .. }
            | Inline::Substituted { value, .. } => value.clone(),
            Inline::AttributeRef { value, .. } => value.clone().unwrap_or_default(),
            _ => panic!("{n:?}"),
        })
        .collect()
}

#[test]
fn definitions_execute_left_to_right_and_keep_header_snapshot() {
    let source=":first: {counter:n} {counter:n} {n}\n:second: {counter2:n}{counter:n}\n\n{first}|{second}|{n}\n\n:third: {counter:n}\n\n{third}|{n}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.document.attributes.get("n"), Some("4"));
    assert_eq!(text(result.document.flat_blocks()[0]), "1 2 2|4|4");
    assert_eq!(text(result.document.flat_blocks()[1]), "5|4");
    assert_eq!(
        result.document.flat_blocks()[0].attribute_context.get("n"),
        Some("4")
    );
    assert_eq!(
        result.document.flat_blocks()[1].attribute_context.get("n"),
        Some("5")
    );
}

#[test]
fn body_side_effects_affect_reading_and_headings_but_only_declared_results_replay() {
    let result=parse(":first: {counter:n}\n\n{n}\n\n:second: {counter:n}\n\n== Head {n}\n\n{n} {second}\n\n:n: 8\n:third: {counter:n}\n\n{n} {third}");
    assert_eq!(text(result.document.flat_blocks()[1]), "Head 2");
    assert_eq!(text(result.document.flat_blocks()[2]), "1 2");
    assert_eq!(text(result.document.flat_blocks()[3]), "8 9");
    assert_eq!(result.document.attributes.get("n"), Some("1"));
}

#[test]
fn locked_counters_have_an_independent_registry_and_locked_definitions_do_not_execute() {
    let mut attributes = Attributes::new();
    attributes.set("n", "8");
    attributes.set("ignored", "host");
    let source=":first: {counter:n}\n:second: {counter:n}\n:ignored: {counter:n}{set:other:bad}\n:third: {counter:n}\n\n{first} {second} {third} {n}";
    let result = parse_with_options(
        source,
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    assert_eq!(text(result.document.flat_blocks()[0]), "9 10 11 8");
    assert_eq!(result.document.attributes.get("other"), None);
}

#[test]
fn counter_names_remain_case_sensitive_and_large_integers_do_not_overflow() {
    let result=parse(":v: {counter:N:9}{counter:n:3}{counter:N}\n:big: {counter:huge:999999999999999999999999999999}\n:next: {counter:huge}\n:negative: {counter:neg:-100000000000000000000000000000}\n:negative-next: {counter:neg}\n\n{v}");
    let a = &result.document.attributes;
    assert_eq!(a.get("v"), Some("9310"));
    assert_eq!(a.get("n"), Some("3"));
    assert!(a.iter().any(|(k, v)| k == "N" && v == "10"));
    assert_eq!(a.get("next"), Some("1000000000000000000000000000000"));
    assert_eq!(
        a.get("negative-next"),
        Some("-99999999999999999999999999999")
    );
}

#[test]
fn string_seeds_follow_numeric_and_alphabetic_carry_rules() {
    for (seed, expected) in [
        ("Z", "AA"),
        ("z9", "aa0"),
        ("9-z", "9-aa"),
        ("Z-z", "AA-a"),
        ("01", "02"),
        ("-01", "-02"),
        ("-1", "0"),
        ("Ж", "З"),
        ("９", "１０"),
        ("٩", "١٠"),
        ("۹", "۱۰"),
        ("Ö", "Ø"),
        ("𝟡", "𝟢"),
        ("𝟿", "𝟏𝟎"),
        ("\u{10ffff}", "\u{1}\u{10000}"),
        ("Я", "а"),
        ("Ω", "Ϊ"),
        ("!", "\""),
        ("", "1"),
    ] {
        let result = parse(&format!(
            ":first: {{counter:n:{seed}}}\n:second: {{counter:n}}\n\n{{second}}"
        ));
        assert_eq!(
            result.document.attributes.get("second"),
            Some(expected),
            "{seed}"
        );
    }
}

#[test]
fn set_sanitizes_names_aliases_and_resets_or_unsets_counters() {
    let result=parse(":attribute-undefined: skip\n:v: {set:A b.c:one}{abc}|{set:hardbreaks}{hardbreaks-option}|{counter:n}{set:n:9}{counter:n}{set:n!}{counter:n}\n\n{v}");
    let a = &result.document.attributes;
    assert_eq!(a.get("abc"), Some("one"));
    assert_eq!(a.get("hardbreaks-option"), Some(""));
    assert_eq!(a.get("v"), Some("one||1101"));
    assert_eq!(a.get("n"), Some("1"));
}

#[test]
fn unset_filters_transformed_lines_but_preserves_all_executed_side_effects() {
    for (policy, value) in [
        ("drop-line", ""),
        ("skip", "beforeafter2"),
        ("drop", "beforeafter2"),
    ] {
        let result=parse(&format!(":attribute-undefined: {policy}\n:x: old\n:v: before{{set:x!}}after{{counter2:n}}{{counter:n}}\n\n{{v}}"));
        assert_eq!(result.document.attributes.get("v"), Some(value));
        assert_eq!(result.document.attributes.get("n"), Some("2"));
        assert_eq!(result.document.attributes.get("x"), None);
    }
}

#[test]
fn missing_policy_is_read_lazily_then_cached_within_each_attribute_stage() {
    let result = parse(
        ":v: {set:attribute-missing:warn}{missing}{set:attribute-missing:drop}{other}\n\nBody",
    );
    assert_eq!(result.diagnostics.len(), 2, "{result:?}");
    assert!(result
        .diagnostics
        .iter()
        .all(|d| d.severity == Severity::Warning));
    assert_eq!(
        result.document.attributes.get("v"),
        Some("{missing}{other}")
    );
    assert_eq!(
        result.document.attributes.get("attribute-missing"),
        Some("drop")
    );
}

#[test]
fn escaped_expressions_and_protected_passes_do_not_execute() {
    let result =
        parse(":v: \\{counter:n} {counter:n\\}\n:p: pass:n[+{counter:n}+ {counter:n}]\n\n{v} {p}");
    assert_eq!(
        result.document.attributes.get("v"),
        Some("{counter:n} {counter:n}")
    );
    assert_eq!(result.document.attributes.get("p"), Some("{counter:n} 1"));
    assert_eq!(result.document.attributes.get("n"), Some("1"));
}

#[test]
fn selected_passes_execute_at_restoration_after_outer_stages() {
    let result = parse(":v: pass:n[pass:a[{counter:n}] {counter:n} pass:a[{counter:n}]]\n\n{v}");
    assert_eq!(result.document.attributes.get("v"), Some("2 1 3"));
    let result =
        parse(":attribute-missing: drop-line\n:v: pass:n[pass:a[{counter:n}] {missing}]\n\nBody");
    assert_eq!(result.document.attributes.get("n"), None);
    assert_eq!(result.document.attributes.get("v"), Some(""));
}

#[test]
fn repeated_slots_execute_for_each_target_and_label_occurrence() {
    let result = parse(":v: pass:n[link:pass:a[{counter:n}][]]\n\n{v}");
    let ps = result.document.attributes.raw_parts("v").unwrap();
    assert!(ps
        .iter()
        .any(|p| matches!(p,P::OpenLink(l) if l.target=="1")));
    assert!(ps.contains(&P::Raw("2".into())));
    assert_eq!(result.document.attributes.get("n"), Some("2"));
}

#[test]
fn metadata_and_repeated_parameters_share_definition_state_and_neutral_parts() {
    let late = parse(":v: pass:q,a[[.\\{counter:n}]*Text*]\n\n{v}");
    assert!(
        late.document
            .attributes
            .raw_parts("v")
            .unwrap()
            .contains(&P::OpenWithAttributes {
                format: F::Strong,
                id: None,
                role: Some("1".into()),
            }),
        "{late:?}"
    );

    let result=parse(":v: pass:q,a[[.{counter:n}]*{counter:n}*]\n:w: pass:m[link:page['{counter:n}',role=x] link:page['{counter:n}',role=x]]\n\n{v} {w}");
    let ps = result.document.attributes.raw_parts("v").unwrap();
    assert!(ps.contains(&P::OpenWithAttributes {
        format: F::Strong,
        id: None,
        role: Some("1".into())
    }));
    assert_eq!(result.document.attributes.get("v"), Some("2"));
    assert_eq!(result.document.attributes.get("w"), Some("3 4"));
    assert_eq!(result.document.attributes.get("n"), Some("4"));
}

#[test]
fn diagnostics_and_tokens_keep_the_whole_physical_definition_origin() {
    let source = ":v: {set:attribute-missing:warn}{counter:n}{missing}\r\n\r\n{v}";
    let result = parse_with_options(
        source,
        &ParseOptions {
            source_id: SourceId(7),
            ..Default::default()
        },
    );
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        result.diagnostics[0].span,
        Span::in_source(SourceId(7), 0, source.find('\r').unwrap())
    );
    let ts = tokens(source);
    assert_eq!(
        ts.iter()
            .filter(|t| t.kind == TokenKind::AttributeEntry)
            .count(),
        1
    );
    assert_eq!(
        ts.iter()
            .filter(|t| t.kind == TokenKind::AttributeRef)
            .count(),
        1
    );
    assert_eq!(parse(source).document.attributes.get("n"), Some("1"));
}

#[test]
fn incomplete_unicode_and_long_unclosed_expressions_remain_bounded() {
    let source = ":v: pass:n[[.{counter:число}]pass:a[{set:имя:Ж}] {counter:число}]\r\n\r\n{v}";
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        let src = &source[..end];
        let result = parse(src);
        for d in result.diagnostics {
            assert!(src.get(d.span.start..d.span.end).is_some());
        }
        for t in tokens(src) {
            assert!(src.get(t.span.start..t.span.end).is_some());
        }
    }
    let source = format!(":v: {}\n\nBody", "{counter:".repeat(10_000));
    assert_eq!(
        parse(&source).document.attributes.get("v").unwrap().len(),
        90_000
    );
}

#[test]
fn locked_sets_skip_value_evaluation_but_unset_still_filters_the_line() {
    let mut attributes = Attributes::new();
    attributes.set("n", "8");
    let source = ":v: {set:n:pass:bad[ignored]}{counter:n}\n:u: before{set:n!}after\n:w: {counter:n}\n\n{v} {u} {w} {n}";
    let result = parse_with_options(
        source,
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    );
    assert!(result.diagnostics.is_empty(), "{result:?}");
    assert_eq!(result.document.attributes.get("v"), Some("9"));
    assert_eq!(result.document.attributes.get("u"), Some(""));
    assert_eq!(result.document.attributes.get("w"), Some("10"));
    assert_eq!(result.document.attributes.get("n"), Some("8"));
}

#[test]
fn lazy_definition_passthroughs_retain_the_shared_recursion_limit_and_origin() {
    let mut body = "{counter:n}".to_owned();
    for _ in 0..40 {
        body = format!("pass:m[{}]", body.replace(']', r"\]"));
    }
    let entry = format!(":v: pass:m[{body}]");
    let source = format!("{entry}\n\n{{v}}");
    let result = parse(&source);
    assert_eq!(result.diagnostics.len(), 1, "{result:?}");
    assert!(result.diagnostics[0].message.contains("nesting limit (32)"));
    assert_eq!(result.diagnostics[0].span, Span::new(0, entry.len()));
    assert_eq!(result.document.attributes.get("n"), None);
    assert!(!format!("{result:?}").contains('\u{96}'));
}
