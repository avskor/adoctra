use adoctra_core::{
    parse, parse_with_options, tokens, tokens_with_options, AttributePart as P, BlockKind, Inline,
    ParseOptions, SourceId, Span, TokenKind,
};

fn inlines(result: &adoctra_core::ParseResult, index: usize) -> &[Inline] {
    match &result.document.blocks[index].kind {
        BlockKind::Paragraph { inlines } | BlockKind::Verse { inlines, .. } => inlines,
        other => panic!("{other:?}"),
    }
}

fn breaks(nodes: &[Inline]) -> usize {
    nodes
        .iter()
        .map(|n| match n {
            Inline::Substituted { parts, .. } | Inline::AttributeRef { parts, .. } => {
                parts.iter().filter(|p| **p == P::LineBreak).count()
            }
            Inline::Strong { children, .. } | Inline::CrossReference { children, .. } => {
                breaks(children)
            }
            _ => 0,
        })
        .sum()
}

#[test]
fn explicit_breaks_keep_unicode_byte_spans_and_token_positions() {
    let source = "Ж(C) +\nБ +";
    let result = parse(source);
    let nodes = inlines(&result, 0);
    assert_eq!(breaks(nodes), 2);
    let spans: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::Substituted { parts, span, .. } if parts == &[P::LineBreak] => Some(*span),
            _ => None,
        })
        .collect();
    assert_eq!(spans, vec![Span::new(5, 7), Span::new(10, 12)]);
    let highlighted: Vec<_> = tokens(source)
        .into_iter()
        .filter(|t| t.kind == TokenKind::LineBreak)
        .map(|t| t.span)
        .collect();
    assert_eq!(highlighted, spans);
}

#[test]
fn attribute_owned_breaks_preserve_reference_and_snapshot() {
    let source = ":v: one + \\\n two\n\n{v}";
    let result = parse(source);
    assert_eq!(result.document.attributes.get("v"), Some("one +\ntwo"));
    let Inline::AttributeRef {
        value, parts, span, ..
    } = &inlines(&result, 0)[0]
    else {
        panic!()
    };
    assert_eq!(value.as_deref(), Some("one two"));
    assert!(parts.contains(&P::LineBreak));
    assert_eq!(&source[span.start..span.end], "{v}");
    assert!(tokens(source)
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == *span));
    assert!(!tokens(source)
        .iter()
        .any(|t| t.kind == TokenKind::LineBreak));
}

#[test]
fn marker_crosses_source_and_attribute_owners_without_losing_empty_refs() {
    let source = ":p: +\n\nЖ {p}\nlast";
    let result = parse(source);
    let nodes = inlines(&result, 0);
    let marker = nodes
        .iter()
        .find(|n| matches!(n, Inline::Substituted {parts, ..} if parts.contains(&P::LineBreak)))
        .unwrap();
    assert_eq!(&source[marker.span().start..marker.span().end], " {p}");
    assert!(nodes.iter().any(|n| matches!(n, Inline::AttributeRef { value:Some(v),span,.. } if v.is_empty() && &source[span.start..span.end]=="{p}")));
    assert_eq!(breaks(nodes), 1);
    let result = parse(":s: pass:[ ]\n:p: +\n\nЖ{s}{p}\nlast");
    assert_eq!(breaks(inlines(&result, 0)), 1);
}

#[test]
fn hardbreaks_alias_unsets_and_block_snapshots_are_local() {
    let source = ":hardbreaks:\n\none\ntwo +\n\n:hardbreaks!:\n\nthree\nfour +";
    let result = parse(source);
    assert!(result
        .document
        .attributes
        .get("hardbreaks-option")
        .is_some());
    assert!(result.document.attributes.get("hardbreaks").is_none());
    assert_eq!(breaks(inlines(&result, 0)), 1);
    assert!(inlines(&result, 0)
        .iter()
        .any(|n| matches!(n,Inline::Text {value,..} if value.ends_with("two +"))));
    assert_eq!(breaks(inlines(&result, 1)), 1);
    assert!(result.document.blocks[1]
        .attribute_context
        .get("hardbreaks-option")
        .is_none());
    assert_eq!(
        breaks(inlines(
            &parse(":hardbreaks:\n:!hardbreaks:\n\none\ntwo"),
            0
        )),
        0
    );
}

#[test]
fn host_alias_is_locked_and_generated_newlines_keep_reference_spans() {
    let mut options = ParseOptions {
        source_id: SourceId(7),
        ..Default::default()
    };
    options.attributes.set("hardbreaks", "");
    options.attributes.set("v", "one\ntwo +");
    let source = ":hardbreaks!:\n\n{v}";
    let result = parse_with_options(source, &options);
    assert_eq!(breaks(inlines(&result, 0)), 1);
    assert_eq!(
        inlines(&result, 0)[0].span(),
        Span::in_source(SourceId(7), 15, 18)
    );
    assert!(result.document.blocks[0]
        .attribute_context
        .get("hardbreaks-option")
        .is_some());
}

#[test]
fn local_options_do_not_leak_into_containers_lists_or_following_blocks() {
    for option in ["%hardbreaks", "options=hardbreaks", "hardbreaks-option="] {
        let result = parse(&format!("[{option}]\none\ntwo\n\nthree\nfour"));
        assert_eq!(breaks(inlines(&result, 0)), 1);
        assert_eq!(breaks(inlines(&result, 1)), 0);
        assert!(result.document.blocks[0]
            .attribute_context
            .get("hardbreaks-option")
            .is_none());
    }
    let result = parse("[%hardbreaks]\n====\none\ntwo\n====\n\n[%hardbreaks]\n* one\n  two");
    let BlockKind::Example { blocks } = &result.document.blocks[0].kind else {
        panic!()
    };
    let BlockKind::Paragraph { inlines } = &blocks[0].kind else {
        panic!()
    };
    assert_eq!(breaks(inlines), 0);
    let BlockKind::List { items, .. } = &result.document.blocks[1].kind else {
        panic!()
    };
    assert_eq!(breaks(&items[0].inlines), 0);
}

#[test]
fn formatting_links_and_xrefs_observe_rendered_line_ends() {
    for (source, count) in [
        ("*one +\ntwo* +\nend", 2),
        ("*one +*\ntwo", 0),
        ("link:https://e[one +\ntwo] +\nend", 2),
        ("link:https://e[one +]\nend", 0),
        ("<<foo,one +\ntwo>> +\nend", 2),
        ("one +[[foo]]\ntwo", 0),
    ] {
        assert_eq!(breaks(inlines(&parse(source), 0)), count, "{source}");
    }
}

#[test]
fn passthrough_restoration_follows_outer_post_stage() {
    for source in ["pass:[one +\ntwo]", "+one +\ntwo+", "pass:c[one +\ntwo]"] {
        let result = parse(&format!(":hardbreaks:\n\n{source}"));
        assert_eq!(breaks(inlines(&result, 0)), 0, "{source}");
    }
    assert_eq!(breaks(inlines(&parse("pass:p[one +\ntwo]"), 0)), 1);
    assert!(!tokens("pass:p[ +]")
        .iter()
        .any(|t| t.kind == TokenKind::LineBreak));
    let result = parse(":v: pass:[one +]\n\n{v}\ntwo");
    assert_eq!(breaks(inlines(&result, 0)), 1);
}

#[test]
fn verse_preserves_newlines_and_local_options_in_all_forms() {
    for source in [
        "[verse%hardbreaks]\nЖ\nБ +",
        "[verse%hardbreaks]\n____\nЖ\nБ +\n____",
        "[verse%hardbreaks]\n--\nЖ\nБ +\n--",
    ] {
        let result = parse(source);
        let nodes = inlines(&result, 0);
        assert_eq!(breaks(nodes), 1, "{source}");
        assert!(nodes.iter().any(
            |n| matches!(n,Inline::Substituted {parts,..} if parts.contains(&P::Text("\n".into())))
        ));
        assert!(nodes
            .iter()
            .any(|n| matches!(n,Inline::Text {value,..} if value.ends_with("Б +"))));
    }
}

#[test]
fn physical_crlf_comments_and_source_ids_survive() {
    let source = "Ж +\r\n// removed\r\nБ +";
    let options = ParseOptions {
        source_id: SourceId(8),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert_eq!(breaks(inlines(&result, 0)), 2);
    let tokens = tokens_with_options(source, &options);
    let spans: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::LineBreak)
        .map(|t| t.span)
        .collect();
    assert_eq!(spans.len(), 2);
    for span in spans {
        assert_eq!(span.source_id, SourceId(8));
        assert_eq!(&source[span.start..span.end], " +");
    }
}

#[test]
fn highlighting_uses_full_block_option_and_final_line_semantics() {
    let source = "[%hardbreaks]\none\ntwo +\n\nplain\nlast +";
    let spans: Vec<_> = tokens(source)
        .into_iter()
        .filter(|t| t.kind == TokenKind::LineBreak)
        .map(|t| t.span)
        .collect();
    assert_eq!(spans.len(), 2);
    assert_eq!(&source[spans[0].start..spans[0].end], "\n");
    assert_eq!(&source[spans[1].start..spans[1].end], " +");
}

#[test]
fn incomplete_unicode_input_keeps_every_node_and_token_within_source() {
    let source = ":hardbreaks:\n:v: Ж + \\\n Б\n\n[%hardbreaks]\n*Ж +\n{v}* link:https://e[Б +\nВ] pass:p[Ж +\nБ] +";
    struct Bounds(usize);
    impl adoctra_core::visit::VisitorMut for Bounds {
        fn visit_span(&mut self, span: &mut Span) {
            assert!(span.start <= span.end && span.end <= self.0, "{span:?}");
        }
    }
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        let prefix = &source[..end];
        let mut result = parse(prefix);
        adoctra_core::visit::walk_document_mut(&mut Bounds(end), &mut result.document);
        for token in tokens(prefix) {
            assert!(token.span.start <= token.span.end && token.span.end <= end);
            assert!(prefix.is_char_boundary(token.span.start));
            assert!(prefix.is_char_boundary(token.span.end));
        }
    }
}

#[test]
fn local_option_does_not_define_an_attribute_even_inside_selected_passes() {
    let source = ":attribute-missing: warn\n\n[%hardbreaks]\none {hardbreaks-option}\ntwo pass:a[{hardbreaks-option}] pass:normal[three\nfour +]";
    let result = parse(source);
    assert_eq!(result.diagnostics.len(), 2);
    assert!(inlines(&result, 0).iter().any(
        |n| matches!(n, Inline::AttributeRef {name,value:None,..} if name=="hardbreaks-option")
    ));
    assert_eq!(breaks(inlines(&result, 0)), 2);
    assert_eq!(
        result.document.blocks[0].attribute_context.as_ref(),
        &result.document.attributes
    );
    for diagnostic in result.diagnostics {
        assert!(source[diagnostic.span.start..diagnostic.span.end].contains("{hardbreaks-option}"));
    }
}
