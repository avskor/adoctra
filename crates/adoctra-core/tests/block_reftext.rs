use adoctra_core::{
    parse, parse_with_options, tokens_with_options, Attributes, Block, BlockKind, Inline,
    ParseOptions, Severity, SourceId, TokenKind,
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
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::CrossReference { children, .. } => text(children),
            _ => panic!("{node:?}"),
        })
        .collect()
}

fn body(block: &Block) -> String {
    match &block.kind {
        BlockKind::Paragraph { inlines } | BlockKind::Heading { inlines, .. } => text(inlines),
        _ => panic!("{block:?}"),
    }
}

#[test]
fn initial_reftext_effects_enter_header_snapshot_once() {
    let result =
        parse("[[here,R {counter:n}{set:x:yes}]]\nBody {counter:n} {x}\n\n<<here>> / <<here>>");
    assert!(result.diagnostics.is_empty());
    let doc = result.document;
    assert_eq!(doc.attributes.get("n"), Some("1"));
    assert_eq!(doc.attributes.get("x"), Some("yes"));
    assert_eq!(text(doc.blocks[0].reftext.as_ref().unwrap()), "R 1");
    assert_eq!(body(&doc.blocks[0]), "Body 2 yes");
    assert_eq!(body(&doc.blocks[1]), "R 1 / R 1");
    assert_eq!(doc.blocks[1].attribute_context.get("n"), Some("1"));
}

#[test]
fn body_reftext_effects_change_reading_but_are_not_replayed() {
    let doc = parse("Before {counter:n}\n\n[[here,R {counter:n}{set:x:yes}]]\nBody {counter:n} {x}\n\n== S {n} {x}\n\n<<here>> {counter:n}").document;
    assert_eq!(doc.attributes.get("n"), None);
    assert_eq!(body(&doc.blocks[0]), "Before 1");
    assert_eq!(body(&doc.blocks[1]), "Body 2 {x}");
    assert_eq!(doc.blocks[1].attribute_context.get("x"), Some("yes"));
    assert_eq!(body(&doc.blocks[2]), "S 1 yes");
    assert_eq!(body(&doc.blocks[3]), "R 1 3");
}

#[test]
fn replaced_and_orphan_reftexts_execute_and_anchor_without_reftext_keeps_value() {
    let source = "[[old,R {counter:n}]]\n[[here,S {counter:n}]]\n[[last]]\nBody {counter:n}\n\n<<last>>\n\n[[orphan,{counter:n}{set:x:yes}]]";
    let doc = parse(source).document;
    assert_eq!(doc.attributes.get("n"), Some("2"));
    assert_eq!(doc.blocks[0].id.as_deref(), Some("last"));
    assert_eq!(body(&doc.blocks[0]), "Body 3");
    assert_eq!(body(&doc.blocks[1]), "S 2");
    assert_eq!(doc.attributes.get("x"), None);
    assert_eq!(doc.blocks.len(), 2);
    let orphan = parse("[[orphan,{counter:n}{set:x:yes}]]");
    assert_eq!(orphan.document.attributes.get("n"), Some("1"));
    assert_eq!(orphan.document.attributes.get("x"), Some("yes"));
    assert!(orphan.document.blocks.is_empty());
    let ts = tokens_with_options(source, &ParseOptions::default());
    for (start, _) in source.match_indices("{counter:n}") {
        assert!(ts.iter().any(|t| t.kind == TokenKind::AttributeRef
            && t.span.start == start
            && t.span.end == start + "{counter:n}".len()));
    }
}

#[test]
fn reftext_uses_cqr_without_repeating_attributes_or_running_macros() {
    let doc = parse(":x: value\n\n[[here,*{counter:n}* \\{x} link:p[{counter:n}] pass:a[{counter:n}] ++{counter:n}++]]\nBody\n\n<<here>>").document;
    let nodes = doc.blocks[0].reftext.as_ref().unwrap();
    assert_eq!(text(nodes), "1 {x} link:p[2] pass:a[3] ++4++");
    assert!(nodes.iter().any(|n| matches!(n, Inline::Strong { .. })));
    assert_eq!(doc.attributes.get("n"), Some("4"));
    assert!(!nodes.iter().any(|n| matches!(n, Inline::Link { .. })));
}

#[test]
fn reftext_protects_generated_markup_and_uses_completed_header_compat_mode() {
    let doc = parse(":v: pass:q[*bold*]\n\n[[here,{v}]]\nBody\n\n<<here>>").document;
    let nodes = doc.blocks[0].reftext.as_ref().unwrap();
    assert_eq!(text(nodes), "<strong>bold</strong>");
    assert!(matches!(&nodes[0], Inline::AttributeRef { parts, .. } if parts.is_empty()));
    let doc = parse("[[here,R +mono+ {counter:n}]]\nHeading\n=======\n\n<<here>>").document;
    assert_eq!(doc.attributes.get("n"), Some("1"));
    assert!(doc.blocks[0]
        .reftext
        .as_ref()
        .unwrap()
        .iter()
        .any(|n| matches!(n, Inline::Monospace { .. })));
}

#[test]
fn metadata_before_doctitle_is_consumed_once_and_transferred_to_header() {
    let doc =
        parse("[[here,R {counter:n}]]\n[.lead]\n= H {counter:n}\n\nBody {counter:n}\n\n<<here>>")
            .document;
    assert!(doc.header.is_some());
    assert_eq!(doc.attributes.get("n"), Some("2"));
    assert_eq!(doc.attributes.get("reftext"), Some("R 1"));
    assert_eq!(doc.attributes.get("role"), Some("lead"));
    assert_eq!(doc.blocks[0].id.as_deref(), Some("here"));
    assert!(doc.blocks[0].roles.is_empty());
    assert_eq!(body(&doc.blocks[0]), "H 3");
    assert_eq!(body(&doc.blocks[1]), "Body 4");
    assert_eq!(body(&doc.blocks[2]), "R 1");
    assert!(doc.blocks[1].reftext.is_none());
    let doc = parse("[[here,Ref]]\n= {reftext}\n\n{doctitle}\n\n<<here>>").document;
    assert_eq!(doc.attributes.get("doctitle"), Some("{reftext}"));
    assert_eq!(body(&doc.blocks[0]), "Ref");
    assert_eq!(body(&doc.blocks[1]), "{reftext}");
}

#[test]
fn initial_metadata_keeps_following_definitions_in_header_and_respects_comments() {
    let doc = parse("[[here,R {counter:n}]]\n\n// comment\n:n: 8\n\nBody {counter:n}\n\n<<here>>")
        .document;
    assert_eq!(doc.attributes.get("n"), Some("8"));
    assert_eq!(body(&doc.blocks[0]), "Body 9");
    assert_eq!(body(&doc.blocks[1]), "R 1");
    for source in [
        "[comment]\n--\nendif::[]\nifdef::absent[]\n--\n\nshown",
        "[comment]\nhidden\nendif::[]\nifdef::absent[]\n\nshown",
    ] {
        let result = parse(source);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(body(&result.document.blocks[0]), "shown");
    }
}

#[test]
fn empty_expanded_reftext_falls_back_without_losing_executed_reference_nodes() {
    let doc =
        parse(":attribute-missing: drop-line\n\n[[here,{absent}{counter:n}]]\nBody\n\n<<here>>")
            .document;
    assert_eq!(doc.attributes.get("n"), Some("1"));
    assert_eq!(body(&doc.blocks[1]), "[here]");
    let refs = doc.blocks[0].reftext.as_ref().unwrap();
    assert!(refs.iter().any(|n| matches!(n, Inline::AttributeRef { name, value: Some(v), .. } if name == "counter:n" && v.is_empty())));
    let doc =
        parse(":attribute-missing: drop-line\n\n[[here,{absent}]]\n== Title\n\n<<here>>").document;
    assert_eq!(body(&doc.blocks[1]), "Title");
}

#[test]
fn locked_counters_share_registry_while_snapshots_keep_host_values() {
    let mut attributes = Attributes::new();
    attributes.set("n", "8");
    attributes.set("x", "host");
    let doc = parse_with_options(
        "[[here,R {counter:n}{set:x:local}]]\nBody {counter:n} {x}\n\n<<here>>",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    assert_eq!(doc.attributes.get("n"), Some("8"));
    assert_eq!(doc.blocks[0].attribute_context.get("n"), Some("8"));
    assert_eq!(body(&doc.blocks[0]), "Body 10 host");
    assert_eq!(body(&doc.blocks[1]), "R 9");
}

#[test]
fn unicode_crlf_diagnostics_and_tokens_keep_original_reference_spans() {
    let source = ":attribute-missing: warn\r\n\r\n[[here,Ж {counter:n:Я} {missing}]]\r\nBody {counter:n}\r\n\r\n<<here>>";
    let options = ParseOptions {
        source_id: SourceId(7),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert_eq!(result.diagnostics.len(), 1);
    let warning = &result.diagnostics[0];
    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(warning.span.source_id, SourceId(7));
    assert_eq!(&source[warning.span.start..warning.span.end], "{missing}");
    let refs = result.document.blocks[0].reftext.as_ref().unwrap();
    let counter = refs
        .iter()
        .find(|n| matches!(n, Inline::AttributeRef { name, .. } if name == "counter:n:Я"))
        .unwrap();
    assert_eq!(counter.span().source_id, SourceId(7));
    assert_eq!(
        &source[counter.span().start..counter.span().end],
        "{counter:n:Я}"
    );
    let ts = tokens_with_options(source, &options);
    assert_eq!(ts, tokens_with_options(source, &options));
    assert!(ts
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == counter.span()));
    assert!(ts
        .iter()
        .any(|t| t.kind == TokenKind::Anchor
            && source[t.span.start..t.span.end].starts_with("[[here,")));
    assert_eq!(body(&result.document.blocks[1]), "Ж Я {missing}");
}

#[test]
fn header_metadata_and_incomplete_reftext_do_not_panic_or_invent_body_nodes() {
    for source in [
        "[[here,{counter:n}",
        "[[here,{counter:n}]]",
        "[[here,*x]]",
        "[[here,Ж \\{n}]]",
        ".T\n[[here,R]]",
    ] {
        let result = parse(source);
        let ts = tokens_with_options(source, &ParseOptions::default());
        for token in ts {
            assert!(source.get(token.span.start..token.span.end).is_some());
        }
        assert!(result
            .diagnostics
            .iter()
            .all(|d| source.get(d.span.start..d.span.end).is_some()));
    }
}
