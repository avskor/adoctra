use adoctra_core::{
    parse, parse_with_options, tokens, tokens_with_options, AttributePart, Attributes, Block,
    BlockKind, Inline, ParseOptions, Severity, SourceId, Span, TokenKind,
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
            Inline::Strong { children, .. } | Inline::CrossReference { children, .. } => {
                text(children)
            }
            _ => panic!("{node:?}"),
        })
        .collect()
}

fn title(block: &Block) -> String {
    text(block.title.as_ref().unwrap())
}

fn body(block: &Block) -> String {
    match &block.kind {
        BlockKind::Paragraph { inlines }
        | BlockKind::Verse { inlines, .. }
        | BlockKind::Heading { inlines, .. } => text(inlines),
        BlockKind::Example { blocks }
        | BlockKind::Quote { blocks, .. }
        | BlockKind::Sidebar { blocks }
        | BlockKind::Open { blocks }
        | BlockKind::Admonition { blocks, .. } => {
            blocks.iter().map(body).collect::<Vec<_>>().join("|")
        }
        BlockKind::List { items, .. } => items
            .iter()
            .map(|i| text(&i.inlines))
            .collect::<Vec<_>>()
            .join("|"),
        _ => panic!("{block:?}"),
    }
}

#[test]
fn unanchored_titles_share_conversion_state_and_preserve_reading_snapshots() {
    let source =
        "Before {counter:n}\n\n.T {counter:n} {set:x:yes}{x}\nBody {counter:n} {x}\n\n== S {n} {x}";
    let result = parse(source);
    let blocks = &result.document.blocks;
    assert_eq!(body(&blocks[0]), "Before 1");
    assert_eq!(title(&blocks[1]), "T 2 yes");
    assert_eq!(body(&blocks[1]), "Body 3 yes");
    assert_eq!(body(&blocks[2]), "S {n} {x}");
    assert_eq!(blocks[1].attribute_context.get("n"), None);
    assert_eq!(result.document.attributes.get("x"), None);
    let node = blocks[1]
        .title
        .as_ref()
        .unwrap()
        .iter()
        .find(|n| matches!(n, Inline::AttributeRef { name, .. } if name == "counter:n"))
        .unwrap();
    assert_eq!(&source[node.span().start..node.span().end], "{counter:n}");
    assert!(
        matches!(node, Inline::AttributeRef { parts, .. } if parts == &[AttributePart::Raw("2".into())])
    );
}

#[test]
fn anchored_titles_execute_on_completion_and_are_not_replayed() {
    let source = "[[here]]\n.T {counter:n} {v} {set:x:yes}\n====\n:v: inside\n\nBody {counter:n}\n====\n\nifdef::x[]\nPresent {x} {counter:n}\nendif::[]";
    let result = parse(source);
    assert_eq!(title(&result.document.blocks[0]), "T 1 inside ");
    assert_eq!(body(&result.document.blocks[0]), "Body 1");
    assert_eq!(body(&result.document.blocks[1]), "Present {x} 2");
    assert_eq!(
        result.document.blocks[0].attribute_context.get("x"),
        Some("yes")
    );
    let BlockKind::Example { blocks } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert_eq!(blocks[0].attribute_context.get("x"), None);
}

#[test]
fn unanchored_container_titles_run_before_child_declarations() {
    let doc = parse(":v: header\n\n.T {v} {counter:n}\n====\n:v: inside\n\nBody {v} {counter:n}\n====\n\n{v} {counter:n}").document;
    assert_eq!(title(&doc.blocks[0]), "T header 1");
    assert_eq!(body(&doc.blocks[0]), "Body inside 2");
    assert_eq!(body(&doc.blocks[1]), "inside 3");
    assert_eq!(doc.blocks[0].attribute_context.get("v"), Some("inside"));
}

#[test]
fn caption_counter_is_assigned_before_cached_title_and_replayed_before_body() {
    let doc = parse("[[here]]\n.T {counter:example-number}\n====\nBody {counter:example-number}\n====\n\n{example-number}").document;
    assert_eq!(doc.blocks[0].caption.as_deref(), Some("Example 1. "));
    assert_eq!(title(&doc.blocks[0]), "T 2");
    assert_eq!(body(&doc.blocks[0]), "Body 2");
    assert_eq!(body(&doc.blocks[1]), "2");
    assert_eq!(
        doc.blocks[0].attribute_context.get("example-number"),
        Some("2")
    );
    assert_eq!(doc.attributes.get("example-number"), None);
}

#[test]
fn nested_caption_replay_follows_entry_order_after_child_first_assignment() {
    let doc = parse(".Outer {example-number}\n====\n.Inner {example-number}\n[example]\nx {counter:example-number}\n====\n\n{example-number}").document;
    let outer = &doc.blocks[0];
    assert_eq!(outer.caption.as_deref(), Some("Example 2. "));
    assert_eq!(title(outer), "Outer 2");
    let BlockKind::Example { blocks } = &outer.kind else {
        panic!()
    };
    assert_eq!(blocks[0].caption.as_deref(), Some("Example 1. "));
    assert_eq!(title(&blocks[0]), "Inner 1");
    assert_eq!(body(&blocks[0]), "x 2");
    assert_eq!(body(&doc.blocks[1]), "2");
}

#[test]
fn only_reading_title_effects_change_later_caption_prefixes() {
    for (anchor, second) in [("", "Example 2. "), ("[[here]]\n", "Changed 2. ")] {
        let doc = parse(&format!(
            "{anchor}.T {{set:example-caption:Changed}}\n====\nx\n====\n\n.Second\n====\nx\n===="
        ))
        .document;
        assert_eq!(doc.blocks[0].caption.as_deref(), Some("Example 1. "));
        assert_eq!(doc.blocks[1].caption.as_deref(), Some(second));
    }
}

#[test]
fn caption_replay_overrides_body_counter_changes_and_respects_declarations() {
    let doc = parse("Before {counter:example-number}\n\n.T {counter:example-number}\n====\nBody {counter:example-number}\n====\n\n:example-number: 8\n\n.U {example-number}\n====\nx\n====\n\n{counter:example-number}").document;
    assert_eq!(body(&doc.blocks[0]), "Before 1");
    assert_eq!(title(&doc.blocks[1]), "T 2");
    assert_eq!(body(&doc.blocks[1]), "Body 3");
    assert_eq!(doc.blocks[2].caption.as_deref(), Some("Example 9. "));
    assert_eq!(title(&doc.blocks[2]), "U 9");
    assert_eq!(body(&doc.blocks[3]), "10");
}

#[test]
fn locked_caption_counters_keep_reading_registry_across_conversion() {
    let mut attributes = Attributes::new();
    attributes.set("example-number", "8");
    let source = ".A {counter:example-number}\n====\nBody {counter:example-number}\n====\n\n.B {counter:example-number}\n====\nBody {counter:example-number}\n====";
    let options = ParseOptions {
        attributes,
        ..Default::default()
    };
    let doc = parse_with_options(source, &options).document;
    assert_eq!(doc.attributes.get("example-number"), Some("8"));
    for (block, caption, heading, content) in [
        (&doc.blocks[0], "Example 9. ", "A 11", "Body 12"),
        (&doc.blocks[1], "Example 10. ", "B 13", "Body 14"),
    ] {
        assert_eq!(block.caption.as_deref(), Some(caption));
        assert_eq!(title(block), heading);
        assert_eq!(body(block), content);
        assert_eq!(block.attribute_context.get("example-number"), Some("8"));
    }
    assert_eq!(
        tokens_with_options(source, &options),
        tokens_with_options(source, &options)
    );
}

#[test]
fn caption_counters_support_unicode_and_arbitrary_precision_seeds() {
    for (seed, next) in [
        (
            "9999999999999999999999999999",
            "10000000000000000000000000000",
        ),
        ("Я", "а"),
    ] {
        let doc = parse(&format!(
            ":example-number: {seed}\n\n.T {{example-number}}\n====\nx\n===="
        ))
        .document;
        assert_eq!(
            doc.blocks[0].caption.as_deref(),
            Some(format!("Example {next}. ").as_str())
        );
        assert_eq!(title(&doc.blocks[0]), format!("T {next}"));
    }
}

#[test]
fn explicit_and_empty_captions_suppress_automatic_counter_assignment() {
    let doc = parse(":caption: Custom\n\n.A\n====\nx\n====\n\n:caption!:\n\n[caption=\"\"]\n.B\n====\nx\n====\n\n.C\n====\nx\n====").document;
    assert_eq!(doc.blocks[0].caption.as_deref(), Some("Custom"));
    assert_eq!(doc.blocks[1].caption.as_deref(), Some(""));
    assert_eq!(doc.blocks[2].caption.as_deref(), Some("Example 1. "));
    assert_eq!(doc.blocks[1].attribute_context.get("example-number"), None);
}

#[test]
fn overwritten_or_orphan_titles_do_not_execute_or_diagnose() {
    let source = ":attribute-missing: warn\n\n.Unused {counter:n} {missing}\n.Used {counter:n}\nBody {counter:n}\n\n.Orphan {counter:n} {missing}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{result:?}");
    assert_eq!(title(&result.document.blocks[0]), "Used 1");
    assert_eq!(body(&result.document.blocks[0]), "Body 2");
}

#[test]
fn passthrough_titles_only_execute_when_cached_for_an_explicit_id() {
    for (anchor, expected) in [("", "S {x}"), ("[[here]]\n", "S yes")] {
        let source = format!("{anchor}.T {{set:x:yes}}\n[pass]\nRaw\n\n== S {{x}}\n\n{{x}}");
        let doc = parse(&source).document;
        assert_eq!(body(&doc.blocks[1]), expected);
        assert_eq!(body(&doc.blocks[2]), "{x}");
        if anchor.is_empty() {
            assert_eq!(title(&doc.blocks[0]), "T {set:x:yes}");
        } else {
            assert_eq!(title(&doc.blocks[0]), "T ");
        }
    }
}

#[test]
fn title_effects_reach_body_hardbreaks_and_share_late_pass_state() {
    let doc = parse(
        ".T pass:a[{counter:n}] {counter:n} {set:hardbreaks:}\nOne {counter:n}\nTwo\n\n{counter:n}",
    )
    .document;
    assert_eq!(title(&doc.blocks[0]), "T 2 1 ");
    let debug = format!("{:?}", doc.blocks[0].kind);
    assert!(debug.contains("LineBreak"), "{debug}");
    assert_eq!(body(&doc.blocks[1]), "4");
}

#[test]
fn title_evaluation_order_holds_for_lists_verse_and_container_styles() {
    for (block, next) in [
        ("* {counter:n}\n* {counter:n}", "4"),
        ("[verse]\n{counter:n}\n{counter:n}", "4"),
        ("****\n{counter:n}\n\n{counter:n}\n****", "4"),
        ("[NOTE]\n{counter:n}", "3"),
        ("[quote]\n{counter:n}", "3"),
    ] {
        let doc = parse(&format!(".T {{counter:n}}\n{block}\n\n{{counter:n}}")).document;
        assert_eq!(title(&doc.blocks[0]), "T 1");
        let debug = format!("{:?}", doc.blocks[0].kind);
        assert!(debug.contains("Raw(\"2\")"), "{debug}");
        assert_eq!(body(&doc.blocks[1]), next);
    }
}

#[test]
fn title_diagnostics_and_tokens_preserve_unicode_crlf_positions() {
    let source = ".Ж {set:attribute-missing:warn}{counter:n} {missing}\r\nBody {counter:n}";
    let options = ParseOptions {
        source_id: SourceId(7),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert_eq!(result.diagnostics.len(), 1);
    let d = &result.diagnostics[0];
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(d.span.source_id, SourceId(7));
    assert_eq!(&source[d.span.start..d.span.end], "{missing}");
    let ts = tokens_with_options(source, &options);
    for (at, _) in source.match_indices("{counter:n}") {
        assert!(ts.iter().any(|t| t.kind == TokenKind::AttributeRef
            && t.span == Span::in_source(SourceId(7), at, at + "{counter:n}".len())));
    }
    assert_eq!(ts, tokens_with_options(source, &options));
    assert_eq!(
        format!("{result:?}"),
        format!("{:?}", parse_with_options(source, &options))
    );
}

#[test]
fn incomplete_titles_and_nested_passes_keep_bounded_diagnostics() {
    let source = "[[here]]\n.Ж pass:n[{counter:n}] link:page['\\{counter:n}',role=x]\nBody";
    for end in source.char_indices().map(|(i, _)| i).chain([source.len()]) {
        let result = parse(&source[..end]);
        assert!(result.diagnostics.iter().all(|d| d.span.end <= end));
        assert!(tokens(&source[..end]).iter().all(|t| t.span.end <= end));
    }
    let mut nested = "{counter:n}".to_owned();
    for _ in 0..40 {
        nested = format!("pass:m[{}]", nested.replace(']', r"\]"));
    }
    let result = parse(&format!(".T {nested}\nBody"));
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("nesting limit")));
    assert!(!format!("{result:?}").contains('\u{96}'));
}
