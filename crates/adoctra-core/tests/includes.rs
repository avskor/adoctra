use adoctra_core::{
    parse_resource, tokens_resource, BlockKind, IncludeLimits, Inline, MemoryResources,
    ParseOptions, ParseResult, Resource, ResourceError, ResourceErrorKind, ResourceProvider,
    ResourceRequest, ResourceResult, SourceId, SourceMap, Span, TokenKind,
};
use std::cell::RefCell;

fn setup(root: &str, files: &[(&str, &str)]) -> (SourceMap, MemoryResources, ParseOptions) {
    let mut resources = MemoryResources::new();
    resources.insert("/docs/main.adoc", root).unwrap();
    for (name, text) in files {
        resources.insert(name, *text).unwrap();
    }
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    (
        sources,
        resources,
        ParseOptions {
            source_id,
            ..Default::default()
        },
    )
}

#[test]
fn diagnostic_positions_keep_nested_selected_unicode_crlf_byte_ranges() {
    // Ruby has no column/byte-range oracle. Check these against the original
    // resource, including removed indentation and excluded physical lines.
    for selection in [
        "tag=body,indent=0",
        "lines=3..5,indent=0",
        "lines=\"3;5\",indent=0",
    ] {
        let root = ":attribute-missing: warn\r\n\r\ninclude::sub/outer.adoc[]\r\n";
        let outer = format!("// outer\r\n\r\ninclude::inner.adoc[{selection}]\r\n");
        let inner =
            "ignored\r\n// tag::body[]\r\n  Ж🙂 {first}\r\n\r\n  Я {second}\r\n// end::body[]\r\n";
        let (mut sources, resources, options) = setup(
            root,
            &[
                ("/docs/sub/outer.adoc", &outer),
                ("/docs/sub/inner.adoc", inner),
            ],
        );
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
        for (diagnostic, (text, line, column)) in result
            .diagnostics
            .iter()
            .zip([("{first}", 3, 6), ("{second}", 5, 5)])
        {
            let span = diagnostic.span;
            let source = sources.get(span.source_id).unwrap();
            assert_eq!(source.name(), "/docs/sub/inner.adoc");
            assert_eq!(span.start, inner.find(text).unwrap());
            assert_eq!(span.end, span.start + text.len());
            assert_eq!(sources.slice(span), Some(text));
            let position = source.position(span.start).unwrap();
            assert_eq!((position.line, position.column), (line, column));
            assert_eq!(diagnostic.severity, adoctra_core::Severity::Warning);
            assert_eq!(
                diagnostic.report.location,
                adoctra_core::DiagnosticLocation::Unlocated
            );
            assert_eq!(
                diagnostic.message,
                format!(
                    "skipping reference to missing attribute: {}",
                    &text[1..text.len() - 1]
                )
            );
        }
        check_spans(&result, &sources);
    }
}

#[test]
fn diagnostic_positions_keep_nested_heading_at_eof_in_physical_source() {
    let root = "= Title\n\ninclude::sub/outer.adoc[]\n";
    let outer = "// outer\n\ninclude::inner.adoc[]\n";
    let inner = "// inner\r\n\r\n=== Пропуск\r\n";
    let (mut sources, resources, options) = setup(
        root,
        &[
            ("/docs/sub/outer.adoc", outer),
            ("/docs/sub/inner.adoc", inner),
        ],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    let source = sources.get(diagnostic.span.source_id).unwrap();
    assert_eq!(source.name(), "/docs/sub/inner.adoc");
    assert_eq!(diagnostic.span.start, inner.find("===").unwrap());
    let position = source.position(diagnostic.span.start).unwrap();
    assert_eq!((position.line, position.column), (3, 1));
    assert_eq!(
        diagnostic.report.location,
        adoctra_core::DiagnosticLocation::Cursor(adoctra_core::DiagnosticCursor {
            source_id: SourceId(0),
            line: 3
        })
    );
    assert_eq!(
        diagnostic.message,
        "section title out of sequence: expected level 1, got level 2"
    );
    check_spans(&result, &sources);
}

#[test]
fn diagnostic_report_cursor_counts_selected_lines_without_rewriting_physical_spans() {
    for (selection, inner, physical_line) in [
        ("lines=\"3;6..9\"", "ignored\nignored\n=== First\nignored\nignored\n\n===== Second\n\nText.\n", 7),
        ("lines=\"3;6..9\",leveloffset=+1", "ignored\nignored\n=== First\nignored\nignored\n\n===== Second\n\nText.\n", 7),
        ("tags=a;b", "// tag::a[]\n=== First\n// end::a[]\nignored\n// tag::b[]\n\n===== Second\n\nText.\n// end::b[]\n", 7),
    ] {
        let root = format!("= T\n\ninclude::part.adoc[{selection}]\n");
        let (mut sources, resources, options) = setup(&root, &[("/docs/part.adoc", inner)]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 2);
        let first_line = if selection.starts_with("lines") { 3 } else { 2 };
        for (diagnostic, reported_line) in result.diagnostics.iter().zip([first_line, first_line + 2]) {
            assert_eq!(diagnostic.report.location, adoctra_core::DiagnosticLocation::Cursor(
                adoctra_core::DiagnosticCursor { source_id: SourceId(1), line: reported_line }
            ));
        }
        let span = result.diagnostics[1].span;
        assert_eq!(span.start, inner.find("===== Second").unwrap());
        assert_eq!(sources.get(span.source_id).unwrap().position(span.start).unwrap().line, physical_line);
        check_spans(&result, &sources);
    }
}

#[test]
fn diagnostic_report_for_unclosed_tag_points_to_include_but_editor_to_tag() {
    let root = "= T\n\ninclude::part.adoc[tag=body]\n";
    let inner = "ignored\r\n// tag::body[]\r\nЖ text\r\n";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", inner)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.span.source_id, SourceId(1));
    assert_eq!(sources.slice(diagnostic.span), Some("// tag::body[]"));
    assert_eq!(diagnostic.message, "unclosed tag 'body'");
    assert_eq!(
        diagnostic.report.message.as_deref(),
        Some("detected unclosed tag 'body' starting at line 2 of include file: /docs/part.adoc")
    );
    assert_eq!(
        diagnostic.report.location,
        adoctra_core::DiagnosticLocation::Cursor(adoctra_core::DiagnosticCursor {
            source_id: SourceId(0),
            line: 3
        })
    );
    check_spans(&result, &sources);
}

#[test]
fn diagnostic_report_restores_nearest_parent_cursor_after_include_eof() {
    for (outer, expected_source, expected_line) in [
        ("// outer\ninclude::inner.adoc[]\n\nTail\n", SourceId(1), 2),
        ("// outer\ninclude::inner.adoc[]\n", SourceId(0), 3),
    ] {
        let root = "= T\n\ninclude::sub/outer.adoc[]\n\nTail\n";
        let inner = "=== Last\n";
        let (mut sources, resources, options) = setup(
            root,
            &[
                ("/docs/sub/outer.adoc", outer),
                ("/docs/sub/inner.adoc", inner),
            ],
        );
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 1);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.span.source_id, SourceId(2));
        assert_eq!(sources.slice(diagnostic.span), Some("=== Last"));
        assert_eq!(
            diagnostic.report.location,
            adoctra_core::DiagnosticLocation::Cursor(adoctra_core::DiagnosticCursor {
                source_id: expected_source,
                line: expected_line
            })
        );
        check_spans(&result, &sources);
    }
}

#[test]
fn block_reftext_early_effects_keep_selected_include_origins() {
    let root =
        ":attribute-missing: warn\n\ninclude::part.adoc[tag=body,indent=0]\n\n<<here>> {counter:n}";
    let part = "// tag::body[]\r\n  [[here,Ж {counter:n:Я} {missing}]]\r\n  Body {counter:n}\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.document.attributes.get("n"), Some("Я"));
    let nodes = result.document.flat_blocks()[0].reftext.as_ref().unwrap();
    let counter = nodes
        .iter()
        .find(|n| matches!(n, Inline::AttributeRef { name, .. } if name == "counter:n:Я"))
        .unwrap();
    assert_eq!(counter.span().source_id, SourceId(1));
    assert_eq!(sources.slice(counter.span()), Some("{counter:n:Я}"));
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(sources.slice(result.diagnostics[0].span), Some("{missing}"));
    assert_eq!(result.diagnostics[0].span.source_id, SourceId(1));
    let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(ts
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == counter.span()));
    check_spans(&result, &sources);
}

#[test]
fn stateful_block_titles_keep_selected_include_origins_in_both_phases() {
    for anchor in ["", "  [[here]]\r\n"] {
        let root = ":n: 8\n:attribute-missing: warn\n\ninclude::part.adoc[tag=body,indent=0]";
        let part = format!("// tag::body[]\r\n{anchor}  .Ж {{counter:n}} {{missing}}\r\n  ====\r\n  Body {{counter:n}}\r\n  ====\r\n// end::body[]");
        let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", &part)]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        let block = &result.document.flat_blocks()[0];
        let counter = block
            .title
            .as_ref()
            .unwrap()
            .iter()
            .find(|n| matches!(n, Inline::AttributeRef { name, .. } if name == "counter:n"))
            .unwrap();
        assert_eq!(counter.span().source_id, SourceId(1));
        assert_eq!(sources.slice(counter.span()), Some("{counter:n}"));
        assert!(matches!(counter, Inline::AttributeRef { value: Some(v), .. } if v == "9"));
        assert_eq!(
            block.attribute_context.get("n"),
            Some(if anchor.is_empty() { "8" } else { "9" })
        );
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].span.source_id, SourceId(1));
        assert_eq!(sources.slice(result.diagnostics[0].span), Some("{missing}"));
        let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
        assert!(ts
            .iter()
            .any(|t| t.kind == TokenKind::AttributeRef && t.span == counter.span()));
        assert!(ts.iter().any(|t| t.kind == TokenKind::BlockTitle
            && sources.slice(t.span) == Some(".Ж {counter:n} {missing}")));
        check_spans(&result, &sources);
    }
}

#[test]
fn nested_caption_replay_keeps_composite_block_and_physical_title_spans() {
    let root = ".Outer {example-number}\n====\ninclude::part.adoc[]\n====\n\n{example-number}";
    let part = ".Inner {example-number}\n[example]\n--\n{counter:example-number}\n--";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let outer = result.document.flat_blocks()[0];
    assert_eq!(outer.caption.as_deref(), Some("Example 2. "));
    assert!(outer.span.provenance.is_some());
    let BlockKind::Example { blocks } = &outer.kind else {
        panic!()
    };
    assert_eq!(blocks[0].caption.as_deref(), Some("Example 1. "));
    for (block, id, value) in [
        (outer, options.source_id, "2"),
        (&blocks[0], SourceId(1), "1"),
    ] {
        let node = block
            .title
            .as_ref()
            .unwrap()
            .iter()
            .find(|n| matches!(n, Inline::AttributeRef { .. }))
            .unwrap();
        assert_eq!(node.span().source_id, id);
        assert!(node.span().provenance.is_none());
        assert_eq!(sources.slice(node.span()), Some("{example-number}"));
        assert!(matches!(node, Inline::AttributeRef { value: Some(v), .. } if v == value));
    }
    check_spans(&result, &sources);
}

#[test]
fn stateful_headings_keep_selected_include_diagnostics_and_tokens() {
    let root = "include::part.adoc[tags=body,indent=0]\n\n{counter:n}";
    let part = "// tag::body[]\r\n  = Ж {counter:n}\r\n\r\n  == Б {set:attribute-missing:warn}{counter:n} {missing}\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.document.attributes.get("n"), Some("1"));
    assert_eq!(
        result.document.flat_blocks()[1].attribute_context.get("n"),
        Some("2")
    );
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].span.source_id, SourceId(1));
    assert_eq!(sources.slice(result.diagnostics[0].span), Some("{missing}"));
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    for block in &result.document.flat_blocks()[..2] {
        let BlockKind::Heading { inlines, .. } = &block.kind else {
            panic!()
        };
        let node = inlines
            .iter()
            .find(|n| matches!(n, Inline::AttributeRef { name, .. } if name == "counter:n"))
            .unwrap();
        assert_eq!(sources.slice(node.span()), Some("{counter:n}"));
        assert_eq!(node.span().source_id, SourceId(1));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::AttributeRef && t.span == node.span()));
    }
    check_spans(&result, &sources);
}

#[test]
fn stateful_doctitle_override_keeps_composite_declaration_provenance() {
    let root = "= Original\n:doctitle: pass:none[Ж {counter:n} \\\ninclude::part.adoc[]\n\n== S {counter:n}\n\n{counter:n}";
    let (mut sources, resources, options) = setup(
        root,
        &[("/docs/part.adoc", "{set:attribute-missing:warn}{missing}]")],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1, "{result:?}");
    let origin = result.diagnostics[0].span;
    let fragments = &result.provenance[origin.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().source_id, options.source_id);
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    let BlockKind::Heading { inlines, .. } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    for node in inlines {
        assert_eq!(node.span().source_id, origin.source_id);
        assert_eq!(
            result.provenance[node.span().provenance.unwrap()],
            *fragments
        );
    }
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(!tokens.iter().any(|t| t.kind == TokenKind::AttributeRef
        && (t.span.source_id == SourceId(1) || t.span.start < root.find("== S").unwrap())));
    check_spans(&result, &sources);
}

#[test]
fn stateful_setext_title_spans_sources_without_replaying_the_counter() {
    let root = "Ж {counter:n}\ninclude::underline.adoc[]\n\n{counter:n}";
    let (mut sources, resources, options) =
        setup(root, &[("/docs/underline.adoc", "=============")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.document.attributes.get("n"), Some("1"));
    let title = &result.document.flat_blocks()[0];
    let fragments = &result.provenance[title.span.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().source_id, options.source_id);
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    let BlockKind::Heading { inlines, .. } = &title.kind else {
        panic!()
    };
    assert!(inlines
        .iter()
        .any(|n| matches!(n, Inline::AttributeRef { value: Some(value), .. } if value == "2")));
    check_spans(&result, &sources);
}

#[test]
fn stateful_body_keeps_selected_include_origins_and_replays_declarations() {
    let root = "{counter:n}\n\ninclude::part.adoc[tags=body,indent=0]\n\n{counter:n}";
    let part = "// tag::body[]\r\n  :n: 8\r\n\r\n  Ж {counter:n}\r\n  // skipped\r\n  Б {counter:n}\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[1].kind else {
        panic!()
    };
    let references: Vec<_> = inlines
        .iter()
        .filter_map(|n| match n {
            Inline::AttributeRef {
                name, value, span, ..
            } if name == "counter:n" => Some((value.as_deref(), *span)),
            _ => None,
        })
        .collect();
    assert_eq!(
        references.iter().map(|r| r.0).collect::<Vec<_>>(),
        [Some("9"), Some("10")]
    );
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    for (_, span) in references {
        assert_eq!(span.source_id, SourceId(1));
        assert_eq!(sources.slice(span), Some("{counter:n}"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::AttributeRef && t.span == span));
    }
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[2].kind else {
        panic!()
    };
    assert!(
        matches!(&inlines[0], Inline::AttributeRef { value: Some(value), .. } if value == "11")
    );
    check_spans(&result, &sources);
}

#[test]
fn stateful_expression_across_files_retains_composite_provenance() {
    let root = "{set:x:first\ninclude::part.adoc[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", "second}{x}")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let span = inlines
        .iter()
        .find_map(|n| match n {
            Inline::AttributeRef { name, span, .. } if name.starts_with("set:") => Some(*span),
            _ => None,
        })
        .unwrap();
    let fragments = &result.provenance[span.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().source_id, options.source_id);
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    assert_eq!(sources.slice(*fragments.last().unwrap()), Some("second}"));
    check_spans(&result, &sources);
}

#[test]
fn metadata_diagnostics_keep_selected_include_reference_and_macro_origins() {
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "// tag::body[]\r\n  :attribute-missing: warn\r\n\r\n  Ж [.{missing}]*Text* link:https://e['\\{other}',role=x]\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 3, "{result:?}");
    let origins: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| {
            assert_eq!(d.span.source_id, SourceId(1));
            sources.slice(d.span).unwrap()
        })
        .collect();
    assert_eq!(
        origins,
        [
            "{missing}",
            "[.{missing}]*",
            "link:https://e['\\{other}',role=x]"
        ]
    );
    check_spans(&result, &sources);
}

#[test]
fn repeated_link_warning_across_files_keeps_composite_provenance() {
    let root = ":attribute-missing: warn\n\nlink:https://e['\\{missing}',\ninclude::part.adoc[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", "role=x]")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1, "{result:?}");
    let span = result.diagnostics[0].span;
    let fragments = &result.provenance[span.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().source_id, options.source_id);
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    assert_eq!(sources.slice(*fragments.last().unwrap()), Some("role=x]"));
    check_spans(&result, &sources);
}

#[test]
fn post_replacements_keep_selected_include_break_origins_and_tokens() {
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  Ж(C) +\r\n  // removed\r\n  Б +\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let spans: Vec<_> = inlines
        .iter()
        .filter_map(|n| match n {
            Inline::Substituted { parts, span, .. }
                if parts.contains(&adoctra_core::AttributePart::LineBreak) =>
            {
                Some(*span)
            }
            _ => None,
        })
        .collect();
    assert_eq!(spans.len(), 2);
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    for span in spans {
        assert_eq!(span.source_id, SourceId(1));
        assert_eq!(sources.slice(span), Some(" +"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::LineBreak && t.span == span));
    }
    check_spans(&result, &sources);
}

#[test]
fn source_quotes_keep_metadata_reference_tokens_in_selected_include_text() {
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  :role: красный\r\n\r\n  Ж(C) [{role}]*Жёлтый*\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    assert!(inlines.iter().any(|n| matches!(n, Inline::Substituted { parts, span, .. }
        if matches!(&parts[0], adoctra_core::AttributePart::OpenWithAttributes { role: Some(role), .. } if role == "красный")
        && sources.slice(*span) == Some("[{role}]*"))));
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    for (kind, syntax) in [
        (TokenKind::Strong, "[{role}]*Жёлтый*"),
        (TokenKind::AttributeRef, "{role}"),
    ] {
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == kind && sources.slice(t.span) == Some(syntax)),
            "{tokens:?}"
        );
    }
    check_spans(&result, &sources);
}

#[test]
fn ordered_source_quotes_retain_provenance_across_an_include_and_comment() {
    let root = "*First\n// hidden\ninclude::part.adoc[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", "**Second** Tail*")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let Inline::Strong { children, span } = &inlines[0] else {
        panic!("{inlines:?}")
    };
    let fragments = &result.provenance[span.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().source_id, options.source_id);
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    assert!(children.iter().any(
        |n| matches!(n, Inline::Strong { span, .. } if sources.slice(*span) == Some("**Second**"))
    ));
    check_spans(&result, &sources);
}

#[test]
fn direct_link_labels_cross_files_and_keep_formatting_provenance() {
    let root = ":target: page\n\nlink:{target}[*First\ninclude::part.adoc[]";
    let (mut sources, resources, options) = setup(root, &[("docs/part.adoc", "Second*]")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let Inline::Substituted { value, span, parts } = &inlines[0] else {
        panic!("{inlines:?}")
    };
    assert_eq!(value, "First Second");
    assert!(matches!(&parts[0], adoctra_core::AttributePart::OpenLink(l) if l.target == "page"));
    let fragments = &result.provenance[span.provenance.unwrap()];
    assert_eq!(
        sources.slice(fragments[0]),
        Some("link:{target}[*First\ninclude::part.adoc[]")
    );
    assert_eq!(sources.slice(*fragments.last().unwrap()), Some("Second*]"));
    check_spans(&result, &sources);
}

#[test]
fn direct_link_tokens_survive_include_selection_and_indentation() {
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  :target: страница\r\n\r\n  Ж(C) link:{target}[*Жёлтый*]\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let link = inlines
        .iter()
        .find(|n| matches!(n, Inline::Substituted { .. }))
        .unwrap();
    assert_eq!(sources.slice(link.span()), Some("link:{target}[*Жёлтый*]"));
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    for (kind, syntax) in [
        (TokenKind::Link, "link:{target}[*Жёлтый*]"),
        (TokenKind::AttributeRef, "{target}"),
        (TokenKind::Strong, "*Жёлтый*"),
    ] {
        assert!(
            tokens
                .iter()
                .any(|t| t.kind == kind && sources.slice(t.span) == Some(syntax)),
            "{tokens:?}"
        );
    }
    check_spans(&result, &sources);
}

#[test]
fn post_attribute_macro_across_files_retains_all_source_fragments() {
    let root = ":k: nk:\n\nli{k}page[First\ninclude::part.adoc[]";
    let (mut sources, resources, options) = setup(root, &[("docs/part.adoc", "Second]")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let Inline::Substituted { value, span, .. } = &inlines[0] else {
        panic!("{inlines:?}")
    };
    assert_eq!(value, "First Second");
    let fragments = &result.provenance[span.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().source_id, options.source_id);
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    assert_eq!(sources.slice(*fragments.last().unwrap()), Some("Second]"));
    let reference = inlines
        .iter()
        .find(|n| matches!(n, Inline::AttributeRef { .. }))
        .unwrap();
    assert_eq!(sources.slice(reference.span()), Some("{k}"));
    check_spans(&result, &sources);
}

#[test]
fn post_attribute_macros_keep_selected_physical_spans_and_snapshot_values() {
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  :s: ttps\r\n\r\n  h{s}://example.org\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let block = &result.document.flat_blocks()[0];
    assert_eq!(block.attribute_context.get("s"), Some("ttps"));
    let BlockKind::Paragraph { inlines } = &block.kind else {
        panic!()
    };
    let Inline::Substituted { span, parts, .. } = &inlines[0] else {
        panic!("{inlines:?}")
    };
    assert_eq!(sources.slice(*span), Some("h{s}://example.org"));
    assert_eq!(
        sources.get(span.source_id).unwrap().name(),
        "/docs/part.adoc"
    );
    assert!(
        matches!(&parts[0], adoctra_core::AttributePart::OpenLink(l) if l.target == "https://example.org")
    );
    let reference = inlines
        .iter()
        .find(|n| matches!(n, Inline::AttributeRef { .. }))
        .unwrap();
    assert_eq!(sources.slice(reference.span()), Some("{s}"));
    assert!(tokens_resource(&mut sources, &options, &resources)
        .unwrap()
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == reference.span()));
}

#[test]
fn sanitized_section_ids_keep_selected_include_title_and_reference_provenance() {
    let root =
        ":v: pass:[<b>Жёлтый</b> &copy;]\n\n<<_жёлтый>>\n\ninclude::part.adoc[lines=2..3,indent=0]";
    let child = "ignored\r\n  == {v}\r\n  \r\nignored";
    let (mut sources, resources, options) = setup(root, &[("docs/part.adoc", child)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let heading = &result.document.flat_blocks()[1];
    assert_eq!(heading.id.as_deref(), Some("_жёлтый"));
    let BlockKind::Heading { inlines, .. } = &heading.kind else {
        panic!()
    };
    let span = inlines[0].span();
    assert_eq!(sources.slice(span), Some("{v}"));
    assert_eq!(sources.get(span.source_id).unwrap().text(), child);
    assert_ne!(span.source_id, options.source_id);
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let Inline::CrossReference { xref, children, .. } = &inlines[0] else {
        panic!()
    };
    assert_eq!(xref.target, "#_жёлтый");
    assert_eq!(children[0].span(), span);
    assert!(tokens_resource(&mut sources, &options, &resources)
        .unwrap()
        .iter()
        .any(|t| t.kind == TokenKind::Heading
            && t.span.source_id == span.source_id
            && t.span.start <= span.start
            && t.span.end >= span.end));
    check_spans(&result, &sources);
}

fn paragraphs(result: &ParseResult) -> Vec<String> {
    result
        .document
        .flat_blocks()
        .iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => Some(
                inlines
                    .iter()
                    .map(|inline| match inline {
                        Inline::Text { value, .. }
                        | Inline::AttributeRef {
                            value: Some(value), ..
                        } => value.as_str(),
                        _ => panic!("unexpected inline {inline:?}"),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn normalization_depends_on_resource_type_and_keeps_include_boundaries() {
    // `tabsize` expands the tabs that normalization keeps (Ruby's build_block).
    let tab = |n| " ".repeat(n);
    let expanded = format!("{}Я{}X", tab(8), tab(7));
    for (name, expected) in [
        ("part.adoc", format!("{expanded}\n\nZ\u{a0}")),
        ("part.txt", format!("{expanded}\n\nZ\u{a0}")),
        (
            "part.rs",
            format!("{expanded}{}\0\n{}\nZ\u{a0}{}", tab(7), tab(8), tab(6)),
        ),
    ] {
        let child = "\u{feff}\tЯ\tX \t\0\r\n \t\r\nZ\u{a0} \t\r\n";
        let (mut sources, resources, options) = setup(
            &format!(":tabsize: 8\n\n---- \t\ninclude::{name}[] \t\n---- \t"),
            &[(&format!("docs/{name}"), child)],
        );
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty(), "{result:?}");
        let BlockKind::Listing { text, .. } = &result.document.flat_blocks()[0].kind else {
            panic!("{result:?}")
        };
        assert_eq!(text, &expected, "{name}");
        assert_eq!(sources.get(SourceId(1)).unwrap().text(), child);
        check_spans(&result, &sources);
    }
    let (mut sources, resources, options) = setup(
        "before\ninclude::part.adoc[]\nafter",
        &[("docs/part.adoc", "middle \t\n \t\n")],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&result), ["before middle", "after"]);
    check_spans(&result, &sources);
}

#[test]
fn selected_normalized_lines_preserve_physical_markup_and_snapshot_versions() {
    let child = "\u{feff}ignored\r\n*Я* \t\r\nignored\r\n_🙂_ \t\0\r\n";
    let (mut sources, mut resources, options) = setup(
        "include::part.adoc[lines=2;4]",
        &[("docs/part.adoc", child)],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!("{result:?}")
    };
    let highlighted = tokens_resource(&mut sources, &options, &resources).unwrap();
    for text in ["*Я*", "_🙂_"] {
        let start = child.find(text).unwrap();
        let span = Span::in_source(SourceId(1), start, start + text.len());
        assert!(inlines.iter().any(|inline| inline.span() == span));
        assert!(highlighted.iter().any(|token| token.span == span));
    }
    assert!(result.document.flat_blocks()[0].span.provenance.is_some());
    for token in highlighted
        .iter()
        .filter(|t| t.span.source_id == SourceId(1))
    {
        let text = sources.slice(token.span).unwrap();
        assert!(!text.contains('\t') && !text.contains('\0'));
    }
    resources.insert("docs/part.adoc", "skip\nnew \t").unwrap();
    let edited = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&edited), ["new"]);
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), child);
    check_spans(&result, &sources);
    check_spans(&edited, &sources);
}

fn check_spans(result: &ParseResult, sources: &SourceMap) {
    struct Check<'a>(&'a SourceMap, &'a [Vec<Span>]);
    impl adoctra_core::visit::VisitorMut for Check<'_> {
        fn visit_span(&mut self, span: &mut Span) {
            assert!(self.0.slice(*span).is_some(), "invalid span {span:?}");
            if let Some(index) = span.provenance {
                assert!(self.1[index].len() > 1);
                for fragment in &self.1[index] {
                    assert!(fragment.provenance.is_none());
                    assert!(
                        self.0.slice(*fragment).is_some(),
                        "invalid fragment {fragment:?}"
                    );
                }
            }
        }
    }
    adoctra_core::visit::walk_document_mut(
        &mut Check(sources, &result.provenance),
        &mut result.document.clone(),
    );
    for span in result
        .comments
        .iter()
        .chain(&result.conditional_directives)
        .chain(&result.include_directives)
        .chain(&result.inactive)
        .chain(result.diagnostics.iter().map(|d| &d.span))
    {
        assert!(sources.slice(*span).is_some(), "invalid trivia {span:?}");
    }
}

#[test]
fn nested_paths_attributes_and_forward_references_share_one_document() {
    let (mut sources, resources, options) = setup(
        ":part: sub/one.adoc\n\ninclude::{part}[]\n\n== Target\n\n{from-child}",
        &[
            (
                "docs/sub/one.adoc",
                ":from-child: yes\n\ninclude::two.adoc[]",
            ),
            ("docs/sub/two.adoc", "See <<_target>>."),
        ],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.include_directives.len(), 2);
    assert_eq!(
        paragraphs(&ParseResult {
            document: adoctra_core::Document {
                blocks: vec![result.document.flat_blocks()[2].clone()],
                ..Default::default()
            },
            ..result.clone()
        }),
        ["yes"]
    );
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    assert!(matches!(&inlines[1], Inline::CrossReference { .. }));
    assert_eq!(
        sources
            .get(result.document.flat_blocks()[0].span.source_id)
            .unwrap()
            .name(),
        "/docs/sub/two.adoc"
    );
    check_spans(&result, &sources);
}

#[test]
fn one_paragraph_and_nested_markup_can_cross_multiple_sources() {
    let (mut sources, resources, options) = setup(
        "*Я\ninclude::part.adoc[]\nконец*",
        &[("docs/part.adoc", "середина")],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.document.flat_blocks().len(), 1);
    let block = &result.document.flat_blocks()[0];
    let fragments = &result.provenance[block.span.provenance.unwrap()];
    assert_eq!(
        fragments.iter().map(|s| s.source_id).collect::<Vec<_>>(),
        [SourceId(0), SourceId(1), SourceId(0)]
    );
    let BlockKind::Paragraph { inlines } = &block.kind else {
        panic!()
    };
    assert!(matches!(&inlines[0], Inline::Strong { .. }));
    assert!(inlines[0].span().provenance.is_some());
    check_spans(&result, &sources);
}

#[test]
fn unicode_crlf_bom_and_diagnostics_retain_physical_offsets() {
    let text = "\u{feff}Я *текст* <<missing>>\r\n";
    let (mut sources, resources, options) =
        setup("include::часть.adoc[]\r\n", &[("docs/часть.adoc", text)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        sources.slice(result.diagnostics[0].span),
        Some("<<missing>>")
    );
    assert_eq!(result.diagnostics[0].span.source_id, SourceId(1));
    assert_eq!(result.document.flat_blocks()[0].span.start, 3);
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Strong && sources.slice(t.span) == Some("*текст*")));
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::IncludeDirective && t.span.source_id == options.source_id));
    assert!(tokens
        .iter()
        .all(|t| t.span.provenance.is_none() && sources.slice(t.span).is_some()));
    check_spans(&result, &sources);
}

#[test]
fn includes_can_supply_header_list_items_and_container_fences() {
    for (root, child, expected) in [
        (
            "include::part.adoc[]\n\n{author}",
            "= Title\nИван Иванов",
            "header",
        ),
        ("* first\ninclude::part.adoc[]\n* last", "* middle", "list"),
        (
            "====\ninclude::part.adoc[]\n\nafter",
            "inside\n====",
            "example",
        ),
    ] {
        let (mut sources, resources, options) = setup(root, &[("docs/part.adoc", child)]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        match expected {
            "header" => assert_eq!(
                result.document.header.as_ref().unwrap().authors[0].name,
                "Иван Иванов"
            ),
            "list" => assert!(
                matches!(&result.document.flat_blocks()[0].kind, BlockKind::List { items, .. } if items.len() == 3)
            ),
            _ => assert!(
                matches!(&result.document.flat_blocks()[0].kind, BlockKind::Example { blocks } if blocks.len() == 1)
            ),
        }
        check_spans(&result, &sources);
    }
}

#[derive(Default)]
struct Host {
    calls: RefCell<Vec<String>>,
}
impl ResourceProvider for Host {
    fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
        self.calls.borrow_mut().push(request.target.into());
        Err(ResourceError::new(
            ResourceErrorKind::AccessDenied,
            request.target,
            "denied by host",
        ))
    }
}

#[test]
fn skipped_comments_conditions_and_escaped_includes_do_not_load() {
    let root = "ifdef::absent[]\ninclude::hidden.adoc[]\nendif::[]\n////\ninclude::comment.adoc[]\n////\n\\include::escaped.adoc[]\n\ninclude::denied.adoc[]";
    let mut sources = SourceMap::new();
    let source_id = sources.add("/main.adoc", root);
    let host = Host::default();
    let result = parse_resource(
        &mut sources,
        &ParseOptions {
            source_id,
            ..Default::default()
        },
        &host,
    )
    .unwrap();
    assert_eq!(*host.calls.borrow(), ["denied.adoc"]);
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].message.contains("AccessDenied"));
    assert_eq!(
        sources.slice(result.diagnostics[0].span),
        Some("include::denied.adoc[]")
    );
    check_spans(&result, &sources);
}

#[test]
fn included_attributes_follow_lazy_reads_and_container_capture() {
    for (root, expected) in [
        ("include::part.adoc[]\nifdef::flag[after]", vec!["after"]),
        (
            "====\ninclude::part.adoc[]\nifdef::flag[hidden]\n====\n\nifdef::flag[after]",
            vec!["after"],
        ),
    ] {
        let (mut sources, resources, options) = setup(root, &[("docs/part.adoc", ":flag:\n\n")]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty());
        assert_eq!(paragraphs(&result), expected);
        if let BlockKind::Example { blocks } = &result.document.flat_blocks()[0].kind {
            assert!(blocks.is_empty());
        }
        check_spans(&result, &sources);
    }
}

#[test]
fn repeated_include_reuses_snapshot_and_preserves_occurrences() {
    let (mut sources, resources, options) = setup(
        "include::part.adoc[]\ninclude::part.adoc[]",
        &[("docs/part.adoc", "text")],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&result), ["text text"]);
    assert!(sources.get(SourceId(2)).is_none());
    let fragments = &result.provenance[result.document.flat_blocks()[0].span.provenance.unwrap()];
    assert_eq!(
        fragments
            .iter()
            .filter(|s| s.source_id == SourceId(1))
            .count(),
        2
    );
    check_spans(&result, &sources);
}

#[test]
fn cycles_limits_missing_files_and_invalid_encoding_leave_partial_output() {
    for (root, limits, error) in [
        (
            "include::main.adoc[]\n\nafter",
            IncludeLimits::default(),
            "cyclic",
        ),
        (
            "include::missing.adoc[]\n\nafter",
            IncludeLimits::default(),
            "NotFound",
        ),
        (
            "include::part.adoc[encoding=UTF-16]\n\nafter",
            IncludeLimits::default(),
            "InvalidEncoding",
        ),
        (
            "include::part.adoc[]\n\nafter",
            IncludeLimits {
                max_depth: 0,
                ..IncludeLimits::default()
            },
            "depth",
        ),
        (
            "include::part.adoc[]\n\nafter",
            IncludeLimits {
                max_bytes: 2,
                ..IncludeLimits::default()
            },
            "byte count",
        ),
    ] {
        let (mut sources, resources, mut options) = setup(root, &[("docs/part.adoc", "content")]);
        options.includes = limits;
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert!(result.diagnostics[0].message.contains(error), "{result:?}");
        assert_eq!(paragraphs(&result).last().unwrap(), "after");
        check_spans(&result, &sources);
    }
}

#[test]
fn non_asciidoc_includes_are_raw_but_adoc_includes_preprocess_inside_listings() {
    for (name, expected) in [
        ("part.rs", "ifdef::missing[]\nhidden\nendif::[]\nshown"),
        ("part.adoc", "shown"),
    ] {
        let root = format!("----\ninclude::{name}[]\n----");
        let file = format!("docs/{name}");
        let (mut sources, resources, options) = setup(
            &root,
            &[(&file, "ifdef::missing[]\nhidden\nendif::[]\nshown")],
        );
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty());
        assert!(
            matches!(&result.document.flat_blocks()[0].kind, BlockKind::Listing { text, .. } if text == expected)
        );
        check_spans(&result, &sources);
    }
}

#[test]
fn included_conditionals_and_comments_keep_their_source_ids() {
    let (mut sources, resources, options) = setup(
        "include::part.adoc[]",
        &[(
            "docs/part.adoc",
            "// comment\r\nifdef::missing[]\r\nhidden\r\nendif::[]\r\nshown",
        )],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&result), ["shown"]);
    assert_eq!(result.comments[0].source_id, SourceId(1));
    assert_eq!(result.inactive[0].source_id, SourceId(1));
    assert_eq!(result.conditional_directives.len(), 2);
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Comment && sources.slice(t.span) == Some("// comment")));
    check_spans(&result, &sources);
}

#[test]
fn invalid_root_is_an_api_error_and_string_parsing_performs_no_loads() {
    let mut sources = SourceMap::new();
    let resources = MemoryResources::new();
    assert_eq!(
        parse_resource(&mut sources, &ParseOptions::default(), &resources)
            .unwrap_err()
            .kind,
        ResourceErrorKind::InvalidTarget
    );
    let plain = adoctra_core::parse("include::part.adoc[]");
    assert!(plain.diagnostics.is_empty());
    assert_eq!(paragraphs(&plain), ["include::part.adoc[]"]);
}

#[test]
fn depth_and_total_bytes_bound_repeated_or_deep_expansion() {
    let (mut sources, mut resources, mut options) = setup("include::part0.adoc[]", &[]);
    for index in 0..80 {
        resources
            .insert(
                &format!("docs/part{index}.adoc"),
                format!("include::part{}.adoc[]", index + 1),
            )
            .unwrap();
    }
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].message.contains("depth"));
    check_spans(&result, &sources);
    resources
        .insert(
            "docs/repeat.adoc",
            "include::same.adoc[]\ninclude::same.adoc[]\ninclude::same.adoc[]",
        )
        .unwrap();
    resources.insert("docs/same.adoc", "abcd").unwrap();
    options.source_id = sources.load("docs/repeat.adoc", None, &resources).unwrap();
    options.includes.max_bytes = 8;
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].message.contains("byte count"));
    check_spans(&result, &sources);
}

#[test]
fn partial_unicode_input_never_panics_or_produces_invalid_ranges() {
    let child = "\u{feff}Я *bold*\r\n// comment\r\nifdef::missing[]\r\nhidden\r\nendif::[]\r\n<<missing>>\r\n";
    let root = "====\ninclude::part.adoc[]\n====\n\nafter";
    for end in child.char_indices().map(|(i, _)| i).chain([child.len()]) {
        let (mut sources, resources, options) = setup(root, &[("docs/part.adoc", &child[..end])]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        check_spans(&result, &sources);
        let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
        assert!(
            tokens.iter().all(|t| sources.slice(t.span).is_some()),
            "{end}: {tokens:?}"
        );
    }
}

#[test]
fn line_filters_preserve_order_precedence_and_reference_numeric_rules() {
    for (filter, expected) in [
        ("lines=2", "two"),
        ("lines=4;2;2;3..4", "two three four"),
        ("lines=\"1,3..4\"", "one three four"),
        ("lines='2..3'", "two three"),
        ("lines=3..-1", "three four five"),
        ("lines=3..", "three four five"),
        ("lines=3..-2", "three four five"),
        ("lines=2..;4", "two four five"),
        ("lines=20..30", ""),
        ("lines=0;2", ""),
        ("lines=nonsense", ""),
        ("lines=2tail", "two"),
        ("lines=4..2", "one two three four five"),
        ("lines=,tag=absent", "one two three four five"),
        ("lines=None,lines=2", "two"),
        ("lines=2,lines=None", "two"),
        ("lines=2,tag=absent", "two"),
        ("lines=2,lines=3", "three"),
        (
            "lines=2..99999999999999999999999999999999999999999999999",
            "two three four five",
        ),
    ] {
        let root = format!("include::part.adoc[{filter}]");
        let (mut sources, resources, options) =
            setup(&root, &[("docs/part.adoc", "one\ntwo\nthree\nfour\nfive")]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{filter}: {:?}",
            result.diagnostics
        );
        assert_eq!(paragraphs(&result).join(" "), expected, "{filter}");
        check_spans(&result, &sources);
    }
}

#[test]
fn tag_filters_preserve_nested_order_wildcards_and_exclusions() {
    let child = "outside\n// tag::a[]\na\n// tag::b[]\nb\n// end::b[]\nafter-b\n// end::a[]\nend";
    for (filter, expected) in [
        ("tag=a", "a b after-b"),
        ("tags=b;a", "a b after-b"),
        ("tags=\"b,a\"", "a b after-b"),
        ("tag=!b", "outside a after-b end"),
        ("tags=a;!b", "a after-b"),
        ("tag=*", "a b after-b"),
        ("tag=**", "outside a b after-b end"),
        ("tag=!*", "outside end"),
        ("tags=**;!b", "outside a after-b end"),
        ("tags=*;!b", "a after-b"),
        ("tags=a;!*", "a after-b"),
        ("tags=!*;a", "outside a after-b end"),
        ("tags=!**;!b", "a after-b"),
        ("tags=!**;b", "b"),
        ("tags=**;!*;b", "outside b end"),
        ("tags=a;!a", "outside end"),
        ("tags=!a;a", "a b after-b"),
        ("tag=b,tags=a", "b"),
        ("tag=,tags=absent", "outside a b after-b end"),
        ("tag=!,tags=absent", "outside a b after-b end"),
    ] {
        let root = format!("include::part.adoc[{filter}]");
        let (mut sources, resources, options) = setup(&root, &[("docs/part.adoc", child)]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{filter}: {:?}",
            result.diagnostics
        );
        assert_eq!(paragraphs(&result).join(" "), expected, "{filter}");
        check_spans(&result, &sources);
    }
}

#[test]
fn selected_unicode_crlf_lines_have_exact_fragments_and_tokens() {
    let child = "\u{feff}excluded\r\n*Я*\r\nexcluded\r\n_ты_\r\n";
    let (mut sources, resources, options) = setup(
        "include::part.adoc[lines=2;4]",
        &[("docs/part.adoc", child)],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let block = &result.document.flat_blocks()[0];
    let fragments = &result.provenance[block.span.provenance.unwrap()];
    assert_eq!(
        fragments
            .iter()
            .map(|s| sources.slice(*s).unwrap())
            .collect::<Vec<_>>(),
        ["*Я*\r\n", "_ты_"]
    );
    let BlockKind::Paragraph { inlines } = &block.kind else {
        panic!()
    };
    assert!(
        matches!(&inlines[0], Inline::Strong { span, .. } if sources.slice(*span) == Some("*Я*"))
    );
    assert!(inlines.iter().any(
        |i| matches!(i, Inline::Emphasis { span, .. } if sources.slice(*span) == Some("_ты_"))
    ));
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Strong && sources.slice(t.span) == Some("*Я*")));
    assert!(tokens.iter().all(|t| t.span.source_id == options.source_id
        || !sources.slice(t.span).unwrap().contains("excluded")));
    check_spans(&result, &sources);
}

#[test]
fn tag_warnings_point_to_original_markers_and_missing_tags_to_include() {
    let child =
        "\u{feff}ignored\r\n// end::a[]\r\n// tag::a[]\r\nЯ\n// tag::b[]\r\n// end::a[]\r\nlast";
    let root = "include::part.adoc[tags=a;b;missing]";
    let (mut sources, resources, options) = setup(root, &[("docs/part.adoc", child)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&result), ["Я last"]);
    assert_eq!(result.diagnostics.len(), 4, "{:?}", result.diagnostics);
    for (message, text) in [
        ("unexpected", "// end::a[]"),
        ("mismatched", "// end::a[]"),
        ("unclosed", "// tag::b[]"),
        ("not found", root),
    ] {
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains(message))
            .unwrap();
        assert_eq!(diagnostic.severity, adoctra_core::Severity::Warning);
        assert_eq!(sources.slice(diagnostic.span), Some(text));
        assert_eq!(
            diagnostic.span.source_id,
            if message == "not found" {
                SourceId(0)
            } else {
                SourceId(1)
            }
        );
    }
    check_spans(&result, &sources);
}

#[test]
fn selection_precedes_preprocessing_and_does_not_load_excluded_resources() {
    let child = "include::missing.adoc[]\n:flag: bad\n// tag::chosen[]\n:flag: yes\n\nifdef::flag[shown]\ninclude::nested.adoc[lines=2]\n// end::chosen[]";
    let (mut sources, resources, options) = setup(
        ":filter: chosen\n\ninclude::part.adoc[tag={filter}]\n\n{flag}",
        &[
            ("docs/part.adoc", child),
            ("docs/nested.adoc", "include::missing.adoc[]\ninside"),
        ],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(paragraphs(&result), ["shown inside", "yes"]);
    assert_eq!(result.include_directives.len(), 2);
    check_spans(&result, &sources);
}

#[test]
fn raw_tag_markers_obey_boundaries_and_are_not_required_to_be_comments() {
    let child = "before\n<!-- tag::a[] -->\nkept\nnotag::b[]\n// tag::bad name[]\n// tag::bad[]\t\n// tag::other[]\ninside\n// end::other[]\n<!-- end::a[] -->\nafter";
    let (mut sources, resources, options) = setup(
        "----\ninclude::part.rs[tag=a]\n----",
        &[("docs/part.rs", child)],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert!(
        matches!(&result.document.flat_blocks()[0].kind, BlockKind::Listing { text, .. } if text == "kept\nnotag::b[]\n// tag::bad name[]\n// tag::bad[]\t\ninside")
    );
    check_spans(&result, &sources);
}

#[test]
fn tag_selection_and_partial_directives_remain_tolerant() {
    let child = "\u{feff}// tag::я[]\r\n*текст*\r\n// tag::nested[]\r\nmore\r\n// end::nested[]\r\n// end::я[]";
    for end in child.char_indices().map(|(i, _)| i).chain([child.len()]) {
        let (mut sources, resources, options) = setup(
            "include::part.adoc[tags=*;я]",
            &[("docs/part.adoc", &child[..end])],
        );
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        check_spans(&result, &sources);
        assert!(tokens_resource(&mut sources, &options, &resources)
            .unwrap()
            .iter()
            .all(|t| sources.slice(t.span).is_some()));
    }
    let root = "include::part.adoc[tags=\"я;nested\"]";
    for end in root.char_indices().map(|(i, _)| i).chain([root.len()]) {
        let (mut sources, resources, options) = setup(&root[..end], &[("docs/part.adoc", child)]);
        check_spans(
            &parse_resource(&mut sources, &options, &resources).unwrap(),
            &sources,
        );
    }
}

#[test]
fn optional_includes_suppress_missing_resources_only() {
    struct Failure(ResourceErrorKind);
    impl ResourceProvider for Failure {
        fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
            Err(ResourceError::new(self.0, request.target, "test failure"))
        }
    }
    for option in ["opts=optional", "options=\"optional\"", "optional-option="] {
        let root = format!("before\ninclude::missing.adoc[{option}]\nafter");
        for kind in [
            ResourceErrorKind::NotFound,
            ResourceErrorKind::AccessDenied,
            ResourceErrorKind::InvalidEncoding,
            ResourceErrorKind::InvalidTarget,
            ResourceErrorKind::Io,
        ] {
            let (mut sources, _, options) = setup(&root, &[]);
            let result = parse_resource(&mut sources, &options, &Failure(kind)).unwrap();
            assert_eq!(
                result.diagnostics.len(),
                usize::from(kind != ResourceErrorKind::NotFound)
            );
            if kind == ResourceErrorKind::NotFound {
                assert_eq!(paragraphs(&result), ["before after"]);
            } else {
                assert_eq!(
                    sources.slice(result.diagnostics[0].span),
                    Some(root.lines().nth(1).unwrap())
                );
            }
            assert_eq!(result.include_directives.len(), 1);
            check_spans(&result, &sources);
        }
    }
}

#[test]
fn optional_does_not_disable_cycles_limits_or_option_validation() {
    for (root, limits, message) in [
        (
            "include::main.adoc[opts=optional]",
            IncludeLimits::default(),
            "cyclic",
        ),
        (
            "include::part.adoc[opts=optional]",
            IncludeLimits {
                max_depth: 0,
                ..Default::default()
            },
            "depth",
        ),
        (
            "include::part.adoc[opts=optional]",
            IncludeLimits {
                max_bytes: 0,
                ..Default::default()
            },
            "byte count",
        ),
        (
            "include::part.adoc[opts=optional,encoding=UTF-16]",
            IncludeLimits::default(),
            "InvalidEncoding",
        ),
    ] {
        let (mut sources, resources, mut options) = setup(root, &[("docs/part.adoc", "text")]);
        options.includes = limits;
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 1);
        assert!(
            result.diagnostics[0].message.contains(message),
            "{result:?}"
        );
        check_spans(&result, &sources);
    }
}

fn levels(result: &ParseResult) -> Vec<u8> {
    result
        .document
        .flat_blocks()
        .iter()
        .filter_map(|block| match block.kind {
            BlockKind::Heading { level, .. } => Some(level),
            _ => None,
        })
        .collect()
}

#[test]
fn nested_offsets_restore_attributes_and_keep_unicode_heading_positions() {
    let root = "= Main\n\ninclude::part.adoc[leveloffset=+1]\n\n== After";
    let child = "\u{feff}= Глава\r\n\r\ninclude::nested.adoc[leveloffset=+1]\r\n\r\n== Назад";
    let nested = "= Вложенный\n\n== Раздел";
    let (mut sources, resources, options) = setup(
        root,
        &[("docs/part.adoc", child), ("docs/nested.adoc", nested)],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(levels(&result), [0, 1, 2, 3, 2, 1]);
    assert_eq!(
        result.document.flat_blocks()[1]
            .attribute_context
            .get("leveloffset"),
        Some("1")
    );
    assert_eq!(
        result.document.flat_blocks()[2]
            .attribute_context
            .get("leveloffset"),
        Some("2")
    );
    assert_eq!(
        result.document.flat_blocks()[4]
            .attribute_context
            .get("leveloffset"),
        Some("1")
    );
    assert_eq!(
        result.document.flat_blocks()[5]
            .attribute_context
            .get("leveloffset"),
        None
    );
    assert_eq!(
        sources.slice(result.document.flat_blocks()[1].span),
        Some("= Глава")
    );
    assert_eq!(
        result.document.flat_blocks()[1].span,
        Span::in_source(SourceId(1), 3, 3 + "= Глава".len())
    );
    let BlockKind::Heading { inlines, .. } = &result.document.flat_blocks()[1].kind else {
        panic!()
    };
    assert_eq!(sources.slice(inlines[0].span()), Some("Глава"));
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Heading)
            .count(),
        6
    );
    assert!(!tokens.iter().any(|t| t.kind == TokenKind::AttributeEntry));
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::IncludeDirective)
            .count(),
        2
    );
    for token in tokens {
        assert!(sources.slice(token.span).is_some(), "{token:?}");
        assert!(!token.span.is_empty(), "{token:?}");
    }
    check_spans(&result, &sources);
}

#[test]
fn offset_header_and_selected_lines_keep_physical_metadata() {
    let child =
        "omitted\r\n== Заголовок\r\nJane Doe\r\nv2, 2026-01-01\r\n\r\n=== Раздел\r\nomitted";
    let (mut sources, resources, options) = setup(
        "include::part.adoc[lines=2..6,leveloffset=-1]",
        &[("docs/part.adoc", child)],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(levels(&result), [0, 1]);
    let header = result.document.header.as_ref().unwrap();
    assert_eq!(sources.slice(header.title_span), Some("== Заголовок"));
    assert_eq!(sources.slice(header.authors[0].span), Some("Jane Doe"));
    assert_eq!(header.span.source_id, SourceId(1));
    assert_eq!(header.span.start, child.find("==").unwrap());
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Author && sources.slice(t.span) == Some("Jane Doe")));
    assert!(!tokens.iter().any(|t| t.kind == TokenKind::AttributeEntry));
    check_spans(&result, &sources);
}

#[test]
fn offsets_in_verbatim_remain_literal_without_synthetic_highlighting() {
    let root = "----\ninclude::part.rs[leveloffset=+1]\n----\n\n== After";
    let (mut sources, resources, options) = setup(root, &[("docs/part.rs", "== Literal")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Listing { text, .. } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    assert_eq!(text, ":leveloffset: +1\n\n== Literal\n\n:leveloffset!:");
    assert_eq!(levels(&result), [1]);
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    let child_tokens: Vec<_> = tokens
        .iter()
        .filter(|t| t.span.source_id == SourceId(1))
        .collect();
    assert_eq!(child_tokens.len(), 1);
    assert_eq!(child_tokens[0].kind, TokenKind::ListingText);
    assert_eq!(sources.slice(child_tokens[0].span), Some("== Literal"));
    assert!(!tokens.iter().any(|t| t.kind == TokenKind::AttributeEntry));
    check_spans(&result, &sources);
}

#[test]
fn offsets_empty_selections_locked_values_and_partial_input_are_tolerant() {
    let source = "include::part.adoc[lines=2,leveloffset=+1]\n\n== After";
    let (mut sources, resources, mut options) = setup(source, &[("docs/part.adoc", "= Omitted")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(levels(&result), [1]);
    assert_eq!(result.document.attributes.get("leveloffset"), None);
    options.attributes.set("leveloffset", "2");
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(levels(&result), [3]);
    let (mut sources, resources, mut options) = setup(
        "include::part.adoc[leveloffset=+1]\n\n== After",
        &[("docs/part.adoc", "= Child")],
    );
    options.attributes.set("leveloffset", "2");
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    // Without a header, Ruby does not check the level of a leading section.
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(levels(&result), [2, 3]);
    assert!(result
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.attribute_context.get("leveloffset") == Some("2")));
    check_spans(&result, &sources);
    for root in [
        "include::part.adoc[leveloffset=+1]",
        "include::missing.adoc[opts=optional]",
    ] {
        for (end, _) in root
            .char_indices()
            .chain(std::iter::once((root.len(), '\0')))
        {
            let (mut sources, resources, options) =
                setup(&root[..end], &[("docs/part.adoc", "= Я\n\ntext")]);
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            check_spans(&result, &sources);
            for token in tokens_resource(&mut sources, &options, &resources).unwrap() {
                assert!(sources.slice(token.span).is_some());
            }
        }
    }
}

#[test]
fn indentation_preserves_unicode_markup_tokens_and_old_snapshots() {
    let root = "include::part.adoc[lines=2;4,indent=0]";
    let child = "\u{feff}ignored\r\n  *Я*\r\nignored\r\n  _🙂_";
    let (mut sources, mut resources, options) = setup(root, &[("docs/part.adoc", child)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!("{result:?}")
    };
    assert!(matches!(inlines[0], Inline::Strong { .. }));
    assert!(result.document.flat_blocks()[0].span.provenance.is_some());
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    for text in ["*Я*", "_🙂_"] {
        let off = child.find(text).unwrap();
        let expected = Span::in_source(SourceId(1), off, off + text.len());
        assert!(inlines.iter().any(|inline| inline.span() == expected));
        assert!(
            tokens.iter().any(|token| token.span == expected),
            "{tokens:?}"
        );
    }
    check_spans(&result, &sources);
    resources
        .insert("docs/part.adoc", "ignored\n  edited")
        .unwrap();
    let edited = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&edited), ["edited"]);
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), child);
    check_spans(&result, &sources);
}

#[test]
fn indentation_and_tab_expansion_map_to_physical_characters() {
    let child = "\u{feff}\tЯ🙂\tX\r\n\tZ";
    let (mut sources, resources, options) = setup(
        ":tabsize: 4\n\n----\ninclude::part.rs[indent=2]\n----",
        &[("docs/part.rs", child)],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Listing { text, .. } = &result.document.flat_blocks()[0].kind else {
        panic!("{result:?}")
    };
    assert_eq!(text, "  Я🙂  X\n  Z");
    check_spans(&result, &sources);
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    let retained: String = tokens
        .iter()
        .filter(|t| t.span.source_id == SourceId(1))
        .map(|t| sources.slice(t.span).unwrap())
        .collect();
    assert!(retained.contains("Я🙂\tX"), "{tokens:?}");
    assert!(!tokens
        .iter()
        .any(|t| t.span.source_id == SourceId(1) && t.span.start < 4));
}

#[test]
fn dedented_directives_comments_and_diagnostics_have_exact_physical_spans() {
    let child = "  // omitted\r\n  include::nested.adoc[depth=0]\r\n  ifdef::missing[]\r\n  hidden\r\n  endif::[]";
    let nested = "include::leaf.adoc[]";
    let (mut sources, resources, options) = setup(
        "include::part.adoc[indent=0]",
        &[("docs/part.adoc", child), ("docs/nested.adoc", nested)],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].message.contains("depth"));
    assert_eq!(sources.slice(result.diagnostics[0].span), Some(nested));
    assert_eq!(sources.slice(result.comments[0]), Some("// omitted"));
    assert_eq!(
        sources.slice(result.include_directives[1]),
        Some("include::nested.adoc[depth=0]")
    );
    assert_eq!(sources.slice(result.inactive[0]), Some("hidden"));
    check_spans(&result, &sources);
}

#[test]
fn depth_is_scoped_overridable_and_bounded_by_the_host() {
    for (root_depth, nested_depth, limit, expected, failures) in [
        ("0", "99", 64, false, 1),
        ("1", "0", 64, false, 1),
        ("1", "1", 64, true, 0),
        ("1", "999999999999999999999999999", 2, false, 1),
        ("wrong", "1", 64, false, 1),
        ("-3", "1", 64, false, 1),
    ] {
        let root = format!("include::part.adoc[depth={root_depth}]\n\ninclude::leaf.adoc[]");
        let child = format!("include::nested.adoc[depth={nested_depth}]");
        let (mut sources, resources, mut options) = setup(
            &root,
            &[
                ("docs/part.adoc", &child),
                ("docs/nested.adoc", "include::leaf.adoc[]"),
                ("docs/leaf.adoc", "leaf"),
            ],
        );
        options.includes.max_depth = limit;
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), failures, "{result:?}");
        assert_eq!(
            paragraphs(&result).iter().filter(|p| *p == "leaf").count(),
            if expected { 2 } else { 1 }
        );
        assert_eq!(paragraphs(&result).last().unwrap(), "leaf");
        check_spans(&result, &sources);
    }
}

#[test]
fn rejected_depth_does_not_load_the_nested_resource() {
    struct NoLoads;
    impl ResourceProvider for NoLoads {
        fn load(&self, _: ResourceRequest<'_>) -> ResourceResult<Resource> {
            panic!("depth must be checked before loading")
        }
    }
    let mut sources = SourceMap::new();
    let id = sources.add(
        "main.adoc",
        "include::missing.adoc[depth=999,opts=optional]",
    );
    let options = ParseOptions {
        source_id: id,
        includes: IncludeLimits {
            max_depth: 0,
            ..Default::default()
        },
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &NoLoads).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].message.contains("depth"));
}

#[test]
fn indentation_growth_and_huge_values_are_bounded_without_panics() {
    for (source, child) in [
        (
            "include::part.rs[indent=9999999999999999999999999999999999999999999]",
            "X",
        ),
        (
            ":tabsize: 99999999999999999999999999999999999999999999\n\ninclude::part.rs[indent=-1]",
            "\tX",
        ),
        ("include::part.rs[indent=8]", "X\nX\nX"),
        (
            "include::part.rs[indent=8]\ninclude::part.rs[indent=8]",
            "X",
        ),
    ] {
        let (mut sources, resources, mut options) = setup(source, &[("docs/part.rs", child)]);
        options.includes.max_bytes = 16;
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert!(result.diagnostics[0].message.contains("byte count"));
        check_spans(&result, &sources);
    }
}

#[test]
fn partial_indentation_and_depth_input_keeps_all_spans_valid() {
    let root = ":tabsize: 4\n\ninclude::part.adoc[indent=0,depth=1]";
    let child = "\u{feff}\t*Я*\r\n\tinclude::missing.adoc[depth=0]\r\n\tifdef::x[]\r\n\t🙂";
    for end in child
        .char_indices()
        .map(|(off, _)| off)
        .chain(std::iter::once(child.len()))
    {
        let (mut sources, resources, options) = setup(root, &[("docs/part.adoc", &child[..end])]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        check_spans(&result, &sources);
        for token in tokens_resource(&mut sources, &options, &resources).unwrap() {
            assert!(sources.slice(token.span).is_some(), "{token:?}");
        }
    }
    for end in 0..=root.len() {
        let (mut sources, resources, options) = setup(&root[..end], &[("docs/part.adoc", child)]);
        check_spans(
            &parse_resource(&mut sources, &options, &resources).unwrap(),
            &sources,
        );
    }
}

#[test]
fn inline_tokens_after_expanded_tabs_keep_exact_unicode_offsets() {
    let child = "Я\t*🙂*";
    let (mut sources, resources, options) = setup(
        ":tabsize: 4\n\ninclude::part.adoc[indent=-1]",
        &[("docs/part.adoc", child)],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!("{result:?}")
    };
    let expected = Span::in_source(SourceId(1), 3, child.len());
    assert!(inlines
        .iter()
        .any(|inline| matches!(inline, Inline::Strong { .. }) && inline.span() == expected));
    assert!(tokens_resource(&mut sources, &options, &resources)
        .unwrap()
        .iter()
        .any(|t| t.span == expected));
    check_spans(&result, &sources);
}

#[test]
fn multiline_attributes_cross_selected_sources_and_preserve_snapshots() {
    let root = "\u{feff}:x: Я \\\r\ninclude::part.adoc[lines=2;4,indent=0]\r\n last\r\n\r\n{x} <<missing>>";
    let child = "omitted\r\n  🙂 \\ \t\r\nomitted\r\n  middle \\ \t\r\n";
    let (mut sources, mut resources, options) = setup(root, &[("docs/part.adoc", child)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(
        result.document.attributes.get("x"),
        Some("Я 🙂 middle last")
    );
    let child_id = sources.load("docs/part.adoc", None, &resources).unwrap();
    let highlighted = tokens_resource(&mut sources, &options, &resources).unwrap();
    let entries: Vec<_> = highlighted
        .iter()
        .filter(|t| t.kind == TokenKind::AttributeEntry)
        .collect();
    assert_eq!(entries.len(), 4, "{highlighted:?}");
    for text in ["🙂 \\", "middle \\"] {
        let start = child.find(text).unwrap();
        assert!(
            entries
                .iter()
                .any(|t| t.span == Span::in_source(child_id, start, start + text.len())),
            "{entries:?}"
        );
    }
    let start = root.find("{x}").unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!("{result:?}")
    };
    assert!(
        matches!(&inlines[0], Inline::AttributeRef { value: Some(value), span, .. }
        if value == "Я 🙂 middle last" && *span == Span::in_source(options.source_id, start, start + 3))
    );
    assert_eq!(
        result.diagnostics[0].span.start,
        root.find("<<missing>>").unwrap()
    );
    resources
        .insert(
            "docs/part.adoc",
            "ignored\n  changed \\\nignored\n  value \\",
        )
        .unwrap();
    let edited = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(
        edited.document.attributes.get("x"),
        Some("Я changed value last")
    );
    assert_eq!(sources.get(child_id).unwrap().text(), child);
    assert_eq!(sources.get(options.source_id).unwrap().text(), root);
    assert_eq!(
        result.document.attributes.get("x"),
        Some("Я 🙂 middle last")
    );
}

#[test]
fn multiline_attributes_can_end_in_parent_and_use_current_values_for_later_includes() {
    let (mut sources, resources, options) = setup(
        "include::part.adoc[]\n file.adoc\n\ninclude::{target}[]",
        &[
            ("docs/part.adoc", ":target: leaf \\"),
            ("docs/leaf file.adoc", "*Loaded*"),
        ],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{result:?}");
    assert_eq!(
        result.document.attributes.get("target"),
        Some("leaf file.adoc")
    );
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!("{result:?}")
    };
    assert!(matches!(&inlines[0], Inline::Strong { .. }));
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::AttributeEntry)
            .count(),
        2
    );
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::IncludeDirective)
            .count(),
        2
    );
}

#[test]
fn raw_include_suffix_does_not_open_attribute_continuation() {
    let (mut sources, resources, options) = setup(
        ":x: one \\\ninclude::part.rs[]\n\n{x}",
        &[("docs/part.rs", " two \\ \t")],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.document.attributes.get("x"), Some("one two \\ \t"));
    assert_eq!(result.document.flat_blocks().len(), 1);
}

#[test]
fn attribute_lists_accept_positional_unknown_and_recoverable_fields() {
    for (params, expected) in [
        ("ignored,lines=2", "two"),
        ("\"ignored,too\",lines=2", "two"),
        (",,lines=2,,,", "two"),
        ("unknown=anything,lines=2", "two"),
        ("LINES=1,lines=2", "two"),
        ("=lines=1,lines=2", "two"),
        ("lines=\"1\"lines=2", "two"),
        ("lines=\"2\"suffix", "two"),
        ("lines=\"2,indent=0", ""),
        ("lines='2,indent=0", ""),
        ("lines=2,lines=None", "two"),
        ("lines=2,lines=\"None\"", ""),
        ("lines=2,lines=''", "one two three"),
        ("\t lines \t=\t\"2,3\" \t", "two three"),
        ("encoding=None,lines=2", "two"),
        ("optional,opts=unknown,lines=2", "two"),
    ] {
        let root = format!("include::part.adoc[{params}]");
        let (mut sources, resources, options) =
            setup(&root, &[("docs/part.adoc", "one\ntwo\nthree")]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty(), "{params}: {result:?}");
        assert_eq!(paragraphs(&result).join(" "), expected, "{params}");
        assert_eq!(
            result.include_directives,
            [Span::in_source(options.source_id, 0, root.len())]
        );
        check_spans(&result, &sources);
    }
}

#[test]
fn include_options_follow_reference_spacing_merging_and_none_rules() {
    for (params, optional) in [
        ("opts=\"other, op tional\"", true),
        ("opts=optional,options=other", true),
        ("opts=optional,opts=None", true),
        ("optional-option='',unknown=value", true),
        ("optional-option=None", false),
        ("optional", false),
        ("opts=' optional '", false),
        ("opts=\"other,\toptional\"", false),
        ("opts=\"op tional\"", false),
        ("OPTS=optional", false),
    ] {
        let root = format!("include::missing.adoc[{params}]\n\nafter");
        let (mut sources, resources, options) = setup(&root, &[]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(
            result.diagnostics.is_empty(),
            optional,
            "{params}: {result:?}"
        );
        assert_eq!(paragraphs(&result).last().unwrap(), "after");
        if !optional {
            assert!(result.diagnostics[0].message.contains("NotFound"));
        }
        check_spans(&result, &sources);
    }
}

#[test]
fn include_parameter_substitutions_precede_parsing_without_rescanning_values() {
    for (params, tag) in [
        ("{selection}", "chosen"),
        ("tag={injection}", "chosen"),
        ("tag='{raw}'", "{name}"),
        (r"tag='\{name}'", "{name}"),
        (r"tag={name\}", "{name}"),
        ("tag={-bad}", "{-bad}"),
        ("tag='*bold*'", "*bold*"),
        (r#"tag="a\"b""#, "a\"b"),
        (r"tag='a\'b'", "a'b"),
        ("tag='{QUOT}'", "&#34;"),
        ("tag={amp}", "&"),
    ] {
        let root = format!("include::part.adoc[{params}]");
        let child = format!("omitted\n// tag::{tag}[]\nЯ\n// end::{tag}[]\nomitted");
        let (mut sources, resources, mut options) = setup(&root, &[("docs/part.adoc", &child)]);
        options.attributes.set("selection", "ignored,tag=chosen");
        options.attributes.set("injection", "ignored,tag=chosen");
        options.attributes.set("name", "replaced");
        options.attributes.set("raw", "{name}");
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty(), "{params}: {result:?}");
        assert_eq!(paragraphs(&result), ["Я"], "{params}");
        let start = child.find('Я').unwrap();
        assert_eq!(
            result.document.flat_blocks()[0].span,
            Span::in_source(SourceId(1), start, start + "Я".len())
        );
        check_spans(&result, &sources);
    }
}

#[test]
fn missing_parameter_attributes_obey_policy_and_keep_physical_warning_spans() {
    let child = "\u{feff}ignored\r\n  include::leaf.adoc[ignored,lines=2,other={MISSING}]\r\n";
    for (policy, expected, warnings) in [
        ("skip", "two", 0),
        ("drop", "two", 0),
        ("drop-line", "one two three", 0),
        ("warn", "two", 1),
        ("unknown-policy", "two", 0),
    ] {
        let root = format!(":attribute-missing: {policy}\n\ninclude::part.adoc[lines=2,indent=0]");
        let (mut sources, resources, options) = setup(
            &root,
            &[
                ("docs/part.adoc", child),
                ("docs/leaf.adoc", "one\ntwo\nthree"),
            ],
        );
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(paragraphs(&result), [expected], "{policy}");
        assert_eq!(result.diagnostics.len(), warnings, "{policy}: {result:?}");
        let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
        let start = child.find("include::").unwrap();
        let span = Span::in_source(SourceId(1), start, child.len() - 2);
        if warnings > 0 {
            assert_eq!(
                result.diagnostics[0].severity,
                adoctra_core::Severity::Warning
            );
            assert_eq!(result.diagnostics[0].span, span);
            assert!(result.diagnostics[0].message.ends_with(": missing"));
        }
        assert!(tokens
            .iter()
            .any(|token| token.kind == TokenKind::IncludeDirective && token.span == span));
        check_spans(&result, &sources);
    }
}

#[test]
fn parameter_grammar_keeps_unicode_selection_provenance_and_snapshots() {
    let root =
        "\u{feff}:selection: lines=\"2,4\"\r\n\r\ninclude::part.adoc[ignored,{selection},indent=0]";
    let child = "\u{feff}omitted\r\n  *Я*\r\nomitted\r\n  _🙂_\r\n";
    let (mut sources, mut resources, options) = setup(root, &[("docs/part.adoc", child)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{result:?}");
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!("{result:?}")
    };
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    for markup in ["*Я*", "_🙂_"] {
        let start = child.find(markup).unwrap();
        let span = Span::in_source(SourceId(1), start, start + markup.len());
        assert!(inlines.iter().any(|inline| inline.span() == span));
        assert!(tokens.iter().any(|token| token.span == span));
    }
    assert!(result.document.flat_blocks()[0].span.provenance.is_some());
    resources.insert("docs/part.adoc", "replacement").unwrap();
    parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), child);
    check_spans(&result, &sources);
}

#[test]
fn incomplete_and_long_parameter_lists_remain_tolerant() {
    let text = "include::часть.adoc[,,ignored,tag='Я\\'🙂',lines=\"2\"suffix,encoding=None]";
    for end in text.char_indices().map(|(i, _)| i).chain([text.len()]) {
        let (mut sources, resources, options) =
            setup(&text[..end], &[("docs/часть.adoc", "one\nЯ🙂")]);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        check_spans(&result, &sources);
        let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
        assert!(tokens.iter().all(|t| sources.slice(t.span).is_some()));
    }
    let root = format!(
        "include::part.adoc[{}lines=2]",
        "\"ignored\"suffix,,".repeat(5000)
    );
    let (mut sources, resources, options) = setup(&root, &[("docs/part.adoc", "one\ntwo")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(paragraphs(&result), ["two"]);
    check_spans(&result, &sources);
}

#[test]
fn target_substitutions_share_reference_grammar_and_do_not_rescan_values() {
    for (target, filename) in [
        ("{NAME}.adoc", "part.adoc"),
        (r"\{name}.adoc", "{name}.adoc"),
        (r"{name\}.adoc", "{name}.adoc"),
        ("{raw}.adoc", "{name}.adoc"),
        ("{amp}.adoc", "&.adoc"),
        ("{QUOT}.adoc", "&#34;.adoc"),
        ("{sp}part.adoc", " part.adoc"),
        ("{blank}part.adoc", "part.adoc"),
        ("{-bad}.adoc", "{-bad}.adoc"),
        ("{bad.name}.adoc", "{bad.name}.adoc"),
        ("{{name}}.adoc", "{part}.adoc"),
    ] {
        let root = format!("include::{target}[lines=2;4,indent=0]");
        let child = "\u{feff}omitted\r\n  *Я*\r\nomitted\r\n  _🙂_\r\n";
        let (mut sources, mut resources, mut options) =
            setup(&root, &[(&format!("docs/{filename}"), child)]);
        options.attributes.set("name", "part");
        options.attributes.set("raw", "{name}");
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty(), "{target}: {result:?}");
        let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
            panic!("{result:?}")
        };
        let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
        for markup in ["*Я*", "_🙂_"] {
            let start = child.find(markup).unwrap();
            let span = Span::in_source(SourceId(1), start, start + markup.len());
            assert!(inlines.iter().any(|inline| inline.span() == span));
            assert!(tokens.iter().any(|token| token.span == span));
        }
        assert!(result.document.flat_blocks()[0].span.provenance.is_some());
        resources
            .insert(&format!("docs/{filename}"), "updated")
            .unwrap();
        parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(sources.get(SourceId(1)).unwrap().text(), child);
        check_spans(&result, &sources);
    }
}

#[test]
fn target_missing_policies_control_loading_and_preserve_physical_warnings() {
    let child = "\u{feff}omitted\r\n  include::{MISSING}leaf.adoc[]\r\n";
    for (policy, expected, warnings) in [
        ("skip", "literal", 0),
        ("unknown-policy", "literal", 0),
        ("drop", "resolved", 0),
        ("drop-line", "", 0),
        ("warn", "", 2),
    ] {
        let root = format!(":attribute-missing: {policy}\n\ninclude::part.adoc[lines=2,indent=0]");
        let (mut sources, resources, options) = setup(
            &root,
            &[
                ("docs/part.adoc", child),
                ("docs/{MISSING}leaf.adoc", "literal"),
                ("docs/leaf.adoc", "resolved"),
            ],
        );
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), warnings, "{policy}: {result:?}");
        if policy != "warn" {
            assert_eq!(paragraphs(&result).join(" "), expected, "{policy}");
        }
        let start = child.find("include::").unwrap();
        let span = Span::in_source(SourceId(1), start, child.len() - 2);
        assert_eq!(result.include_directives[1], span);
        let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::IncludeDirective && t.span == span));
        if warnings > 0 {
            assert_eq!(result.diagnostics[0].span, span);
            assert_eq!(
                result.diagnostics[0].severity,
                adoctra_core::Severity::Warning
            );
            assert!(result.diagnostics[0]
                .message
                .contains("due to missing attribute"));
        }
        check_spans(&result, &sources);
    }
}

#[test]
fn blank_and_missing_targets_stop_before_provider_and_option_validation() {
    struct NoLoads;
    impl ResourceProvider for NoLoads {
        fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
            panic!("unexpected load: {}", request.target)
        }
    }
    for (policy, target, params, warning) in [
        (
            "drop-line",
            "{missing}part.adoc",
            "encoding=UTF-16,other={missing}",
            None,
        ),
        (
            "warn",
            "{missing}part.adoc",
            "opts=optional,encoding=UTF-16",
            None,
        ),
        ("drop", "{missing}", "opts=optional,encoding=UTF-16", None),
        ("skip", "{empty}", "opts=optional,encoding=UTF-16", None),
        ("drop-line", "{empty}", "", Some("resolved target is blank")),
        ("drop", "{missing}", "", Some("resolved target is blank")),
        ("skip", "{empty}", "", Some("resolved target is blank")),
        (
            "warn",
            "{missing}part.adoc",
            "encoding=UTF-16",
            Some("due to missing attribute"),
        ),
        ("warn", "{empty}", "", Some("resolved target is blank")),
    ] {
        let root =
            format!(":attribute-missing: {policy}\n\nbefore\ninclude::{target}[{params}]\n\nafter");
        let (mut sources, _, mut options) = setup(&root, &[]);
        options.includes.max_depth = 0;
        let result = parse_resource(&mut sources, &options, &NoLoads).unwrap();
        assert_eq!(
            result.diagnostics.len(),
            usize::from(warning.is_some())
                + usize::from(
                    policy == "warn" && target.contains("{missing}") && warning.is_some()
                ),
            "{root}: {result:?}"
        );
        if let Some(message) = warning {
            assert!(
                result.diagnostics[0].message.contains(message),
                "{result:?}"
            );
            assert_eq!(
                result.diagnostics[0].severity,
                adoctra_core::Severity::Warning
            );
            assert_eq!(
                sources.slice(result.diagnostics[0].span),
                Some(root.lines().nth(3).unwrap())
            );
        }
        assert!(
            matches!(&result.document.flat_blocks().last().unwrap().kind,
            BlockKind::Paragraph { inlines } if matches!(inlines.as_slice(),
                [Inline::Text { value, .. }] if value == "after"))
        );
        assert_eq!(result.include_directives.len(), 1);
        check_spans(&result, &sources);
    }
}

#[test]
fn missing_target_does_not_suppress_parameter_warnings_unless_drop_line_skips_them() {
    let root =
        ":attribute-missing: warn\n\ninclude::{missing}[opts=optional,other={absent}]\n\nafter";
    let (mut sources, resources, options) = setup(root, &[]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&result), ["after"]);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        result.diagnostics[0].message,
        "skipping reference to missing attribute: absent"
    );
    check_spans(&result, &sources);
}

#[test]
fn incomplete_and_long_target_substitutions_remain_tolerant() {
    for target in ["{", "{name", "{name\\", "\\{", "{{", "{missing}", "{name}["] {
        for close in ["", "[", "[]"] {
            let root = format!("include::{target}{close}\n\nafter");
            let (mut sources, resources, options) = setup(&root, &[]);
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            assert!(
                matches!(&result.document.flat_blocks().last().unwrap().kind,
            BlockKind::Paragraph { inlines } if matches!(inlines.as_slice(),
                [Inline::Text { value, .. }] if value == "after"))
            );
            check_spans(&result, &sources);
        }
    }
    let root = format!(
        ":attribute-missing: drop-line\n\ninclude::{}[]\n\nafter",
        "{missing}".repeat(10_000)
    );
    let (mut sources, resources, options) = setup(&root, &[]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(paragraphs(&result), ["after"]);
    check_spans(&result, &sources);
}

#[test]
fn setext_title_crosses_selected_indented_include_sources() {
    let child = "\u{feff}ignored\r\n  *Я🙂* \t\r\nignored\r\n";
    let root = "include::part.adoc[lines=2,indent=0]\n====\nJane Doe\nv2\n\nBody";
    let (mut sources, mut resources, options) = setup(root, &[("docs/part.adoc", child)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let header = result.document.header.as_ref().unwrap();
    let fragments = &result.provenance[header.title_span.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().source_id, SourceId(1));
    assert_eq!(fragments.last().unwrap().source_id, options.source_id);
    assert_eq!(
        fragments.last().unwrap().end,
        root.find("====").unwrap() + 4
    );
    assert_eq!(header.authors[0].name, "Jane Doe");
    let BlockKind::Heading { inlines, .. } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    assert_eq!(sources.slice(inlines[0].span()), Some("*Я🙂*"));
    let highlighted = tokens_resource(&mut sources, &options, &resources).unwrap();
    let titles: Vec<_> = highlighted
        .iter()
        .filter(|t| t.kind == TokenKind::Heading)
        .map(|t| sources.slice(t.span).unwrap())
        .collect();
    assert_eq!(titles, ["====", "*Я🙂*"]);
    check_spans(&result, &sources);
    resources.insert("docs/part.adoc", "Changed").unwrap();
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), child);
    assert_eq!(sources.slice(inlines[0].span()), Some("*Я🙂*"));
}

#[test]
fn doctitle_override_keeps_selected_multisource_origins_and_snapshots() {
    let main =
        "\u{feff}= Исходный\r\ninclude::override.adoc[lines=2..3,indent=0]\r\n\r\n{doctitle}";
    let included = "skip\r\n  :doctitle: *Я🙂* \\\r\n    <<missing>>\r\nskip\r\n";
    let (mut sources, mut resources, options) = setup(main, &[("/docs/override.adoc", included)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    check_spans(&result, &sources);
    let header = result.document.header.as_ref().unwrap();
    assert_eq!(header.title, "Исходный");
    assert_eq!(sources.slice(header.title_span), Some("= Исходный"));
    let BlockKind::Heading { inlines, .. } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let origin = inlines[0].span();
    assert_ne!(origin.source_id, options.source_id);
    assert_eq!(sources.get(origin.source_id).unwrap().text(), included);
    // The body reference expands the value, whose `<<missing>>` the macros
    // stage then recognizes: one warning per field, at its physical origin.
    assert_eq!(result.diagnostics.len(), 2);
    assert_eq!(result.diagnostics[0].span, origin);
    assert_eq!(
        sources.slice(result.diagnostics[1].span),
        Some("{doctitle}")
    );
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Heading)
            .count(),
        1
    );
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::AttributeEntry)
            .count(),
        2
    );
    resources
        .insert("/docs/override.adoc", ":doctitle: Changed\n")
        .unwrap();
    let _new = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(sources.get(origin.source_id).unwrap().text(), included);
    check_spans(&result, &sources);

    let (mut sources, resources, options) = setup(
        "= Source\n:doctitle: *First* \\\ninclude::rest.txt[]\n\n{doctitle}",
        &[("/docs/rest.txt", "second\n")],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Heading { inlines, .. } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let origin = inlines[0].span();
    let parts = &result.provenance[origin.provenance.expect("entry crosses files")];
    assert_eq!(
        sources.slice(parts[0]),
        Some(":doctitle: *First* \\\ninclude::rest.txt[]\n")
    );
    assert_eq!(sources.slice(*parts.last().unwrap()), Some("second"));
    assert_ne!(parts[0].source_id, parts.last().unwrap().source_id);
    check_spans(&result, &sources);
}

#[test]
fn indexed_author_overrides_across_selected_includes_keep_original_names_and_spans() {
    let root =
        "= Doc\ninclude::authors.adoc[lines=2..3,indent=0]\ninclude::override.adoc[]\n\n{authors}";
    let included = "ignored\r\n  Иван Петров; Mary_Jane Q Public\r\n  :early: {authors}\r\nignored";
    let (mut sources, resources, options) = setup(
        root,
        &[
            ("/docs/authors.adoc", included),
            ("/docs/override.adoc", ":author_2: Анна Смирнова"),
        ],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(
        result.document.attributes.get("authors"),
        Some("Иван Петров, Анна Смирнова")
    );
    assert_eq!(
        result.document.attributes.get("early"),
        Some("Иван Петров, Mary Jane Q Public")
    );
    let header = result.document.header.as_ref().unwrap();
    assert_eq!(header.authors[1].name, "Mary Jane Q Public");
    assert_eq!(
        sources.slice(header.authors[1].span),
        Some("Mary_Jane Q Public")
    );
    assert_ne!(header.authors[1].span.source_id, options.source_id);
    assert_eq!(paragraphs(&result), ["Иван Петров, Анна Смирнова"]);
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    let author = tokens.iter().find(|t| t.kind == TokenKind::Author).unwrap();
    assert_eq!(
        sources.slice(author.span),
        Some("Иван Петров; Mary_Jane Q Public")
    );
}

#[test]
fn raw_author_substitutions_across_include_preserve_fragments_and_physical_spans() {
    let root = "= Doc\nJohn Doe; Mary Roe\ninclude::author.adoc[lines=2..3,indent=0]\n\n{author_2}\n\n{firstname_2}";
    let included =
        "ignored\r\n  :name: pass:[<b>Анна</b>]\r\n  :author_2: <i>{name}</i> Смирнова\r\nignored";
    let (mut sources, resources, options) = setup(root, &[("/docs/author.adoc", included)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(
        result.document.attributes.get("firstname_2"),
        Some("<i>Анна</i>")
    );
    assert_eq!(
        result.document.attributes.get("authorinitials_2"),
        Some("&С")
    );
    assert_eq!(
        result.document.header.as_ref().unwrap().authors[1].name,
        "Mary Roe"
    );
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[1].kind else {
        panic!()
    };
    let Inline::AttributeRef { parts, span, .. } = &inlines[0] else {
        panic!()
    };
    assert_eq!(sources.slice(*span), Some("{author_2}"));
    assert_eq!(
        parts,
        &[
            adoctra_core::AttributePart::Text("<i>".to_owned()),
            adoctra_core::AttributePart::Raw("<b>Анна</b>".to_owned()),
            adoctra_core::AttributePart::Text("</i> Смирнова".to_owned()),
        ]
    );
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(tokens.iter().any(|t| t.kind == TokenKind::AttributeEntry
        && sources.slice(t.span) == Some(":author_2: <i>{name}</i> Смирнова")));
}

#[test]
fn restored_entities_from_included_attributes_keep_reference_source_and_snapshots() {
    let root = "include::attrs.adoc[lines=2..3,indent=0]\n\ninclude::body.adoc[]";
    let (mut sources, resources, options) = setup(
        root,
        &[
            (
                "/docs/attrs.adoc",
                "ignored\r\n  :prefix: &\r\n  :value: {prefix}copy;\r\nignored",
            ),
            ("/docs/body.adoc", "{value}"),
        ],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let Inline::AttributeRef { parts, span, .. } = &inlines[0] else {
        panic!()
    };
    assert_eq!(
        parts,
        &[adoctra_core::AttributePart::Raw("&copy;".to_owned())]
    );
    assert_eq!(sources.slice(*span), Some("{value}"));
    assert_eq!(
        sources.get(span.source_id).unwrap().name(),
        "/docs/body.adoc"
    );
    assert_eq!(result.document.attributes.get("value"), Some("&copy;"));
    assert!(result.document.attributes.raw_parts("value").is_none());
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    let token = tokens
        .iter()
        .find(|t| t.kind == TokenKind::AttributeRef)
        .unwrap();
    assert_eq!(token.span, *span);
}

#[test]
fn shared_replacements_keep_selected_source_spans_and_old_snapshots() {
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  :v: C\r\n  :name: copy\r\n\r\n  До ({v}) &{name};\r\n// end::body[]";
    let (mut sources, mut resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let raw = inlines
        .iter()
        .find(|n| matches!(n, Inline::Raw { .. }))
        .unwrap();
    assert_eq!(sources.slice(raw.span()), Some("&{name};"));
    assert_eq!(
        sources.get(raw.span().source_id).unwrap().name(),
        "/docs/part.adoc"
    );
    let refs: Vec<_> = inlines
        .iter()
        .filter(|n| matches!(n, Inline::AttributeRef { .. }))
        .collect();
    assert_eq!(refs.len(), 2);
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    for node in refs {
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::AttributeRef && t.span == node.span()));
    }
    resources
        .insert("/docs/part.adoc", part.replace(":v: C", ":v: R"))
        .unwrap();
    let new = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.document.attributes.get("v"), Some("C"));
    assert_eq!(new.document.attributes.get("v"), Some("R"));
    assert_eq!(sources.slice(raw.span()), Some("&{name};"));
    assert_eq!(sources.get(raw.span().source_id).unwrap().text(), part);
}

#[test]
fn pass_replacements_keep_include_selection_positions_and_snapshots() {
    let root = "include::part.adoc[tags=body,indent=0]";
    let part =
        "\u{feff}// tag::body[]\r\n  :v: pass:c,r[До (C) &copy;]\r\n\r\n  {v}\r\n// end::body[]";
    let (mut sources, mut resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(
        result.document.attributes.get("v"),
        Some("До &#169; &copy;")
    );
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let Inline::AttributeRef { span, parts, .. } = &inlines[0] else {
        panic!()
    };
    assert_eq!(sources.slice(*span), Some("{v}"));
    assert_eq!(parts, result.document.attributes.raw_parts("v").unwrap());
    assert!(tokens_resource(&mut sources, &options, &resources)
        .unwrap()
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == *span));
    resources
        .insert("/docs/part.adoc", part.replace("(C)", "(R)"))
        .unwrap();
    let new = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(new.document.attributes.get("v"), Some("До &#174; &copy;"));
    assert_eq!(
        result.document.attributes.get("v"),
        Some("До &#169; &copy;")
    );
    assert_eq!(sources.get(span.source_id).unwrap().text(), part);
}

#[test]
fn pass_quotes_keep_include_selection_positions_and_immutable_formatting() {
    use adoctra_core::{AttributeFormat, AttributePart};
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  :v: pass:c,q[*Жёлтый*]\r\n\r\n  {v}\r\n// end::body[]";
    let (mut sources, mut resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let Inline::AttributeRef {
        span, parts, value, ..
    } = &inlines[0]
    else {
        panic!()
    };
    assert_eq!(sources.slice(*span), Some("{v}"));
    assert_eq!(value.as_deref(), Some("Жёлтый"));
    assert!(parts.contains(&AttributePart::Open(AttributeFormat::Strong)));
    assert_eq!(parts, result.document.attributes.raw_parts("v").unwrap());
    assert!(tokens_resource(&mut sources, &options, &resources)
        .unwrap()
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == *span));
    resources
        .insert("/docs/part.adoc", part.replace('*', "_"))
        .unwrap();
    let new = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(new
        .document
        .attributes
        .raw_parts("v")
        .unwrap()
        .contains(&AttributePart::Open(AttributeFormat::Emphasis)));
    assert!(result
        .document
        .attributes
        .raw_parts("v")
        .unwrap()
        .contains(&AttributePart::Open(AttributeFormat::Strong)));
    assert_eq!(sources.get(span.source_id).unwrap().text(), part);
}

#[test]
fn quote_attributes_keep_selected_include_positions_and_old_snapshots() {
    use adoctra_core::{AttributeFormat, AttributePart};
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  :v: pass:c,q[[#якорь.роль]*Ж&*]\r\n\r\n  {v}\r\n// end::body[]";
    let (mut sources, mut resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let Inline::AttributeRef {
        span, parts, value, ..
    } = &inlines[0]
    else {
        panic!()
    };
    let expected = AttributePart::OpenWithAttributes {
        format: AttributeFormat::Strong,
        id: Some("якорь".into()),
        role: Some("роль".into()),
    };
    assert_eq!(parts[0], expected);
    assert_eq!(value.as_deref(), Some("Ж&"));
    assert_eq!(sources.slice(*span), Some("{v}"));
    assert!(tokens_resource(&mut sources, &options, &resources)
        .unwrap()
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == *span));
    resources
        .insert("/docs/part.adoc", part.replace("роль", "новая"))
        .unwrap();
    let new = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_ne!(new.document.attributes.raw_parts("v").unwrap()[0], expected);
    assert_eq!(
        result.document.attributes.raw_parts("v").unwrap()[0],
        expected
    );
    assert_eq!(sources.get(span.source_id).unwrap().text(), part);
}

#[test]
fn attribute_macro_links_keep_include_provenance_and_immutable_host_updates() {
    use adoctra_core::AttributePart;
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  :v: pass:c,q,m[link:страница[*Ж&*,role=роль]]\r\n\r\n  {v}\r\n// end::body[]";
    let (mut sources, mut resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let Inline::AttributeRef {
        span, parts, value, ..
    } = &inlines[0]
    else {
        panic!()
    };
    assert!(
        matches!(&parts[0],AttributePart::OpenLink(l) if l.target=="страница" && l.role.as_deref()==Some("роль"))
    );
    assert_eq!(value.as_deref(), Some("Ж&"));
    assert_eq!(sources.slice(*span), Some("{v}"));
    assert!(tokens_resource(&mut sources, &options, &resources)
        .unwrap()
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == *span));
    resources
        .insert("/docs/part.adoc", part.replace("страница", "другая"))
        .unwrap();
    let new = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(
        matches!(&new.document.attributes.raw_parts("v").unwrap()[0],AttributePart::OpenLink(l) if l.target=="другая")
    );
    assert!(
        matches!(&result.document.attributes.raw_parts("v").unwrap()[0],AttributePart::OpenLink(l) if l.target=="страница")
    );
    assert_eq!(sources.get(span.source_id).unwrap().text(), part);
}

#[test]
fn inline_passthrough_preserves_selected_include_spans_and_tokens() {
    use adoctra_core::AttributePart;
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  Before +*Ж* {v} (C)+ after\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let span = inlines
        .iter()
        .find_map(|n| match n {
            Inline::Substituted { parts, span, .. }
                if parts == &[AttributePart::Text("*Ж* {v} (C)".into())] =>
            {
                Some(*span)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(sources.slice(span), Some("+*Ж* {v} (C)+"));
    let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(ts
        .iter()
        .any(|t| t.kind == TokenKind::PassthroughText && t.span == span));
    assert!(!ts
        .iter()
        .any(|t| matches!(t.kind, TokenKind::Strong | TokenKind::AttributeRef)));
}

#[test]
fn inline_passthrough_across_files_retains_composite_provenance() {
    let root = "Before +First\n// hidden\ninclude::part.adoc[]";
    let (mut sources, resources, options) =
        setup(root, &[("/docs/part.adoc", "*Second* {v}+ after")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    let span = inlines
        .iter()
        .find_map(|n| match n {
            Inline::Substituted { value, span, .. } if value == "First\n*Second* {v}" => {
                Some(*span)
            }
            _ => None,
        })
        .expect("protected cross-source content");
    let fragments = &result.provenance[span.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().source_id, options.source_id);
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    check_spans(&result, &sources);
}

#[test]
fn pass_group_diagnostics_restore_selected_include_coordinates() {
    let root = "include::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  Before pass:bad,n[*Ж*] after\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(sources.slice(diagnostic.span), Some("bad,n"));
    assert_eq!(diagnostic.span.source_id, SourceId(1));
    check_spans(&result, &sources);
}

#[test]
fn nested_pass_warning_keeps_composite_origin_across_include_and_comments() {
    let root = "Before pass:m[First\n// hidden\ninclude::part.adoc[]";
    let (mut sources, resources, options) =
        setup(root, &[("/docs/part.adoc", r"pass:bad[Ж\]] after")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    let fragments = &result.provenance[diagnostic.span.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().source_id, options.source_id);
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    assert!(diagnostic.message.contains("invalid substitution type"));
    check_spans(&result, &sources);
}

#[test]
fn inline_attribute_policies_preserve_selected_include_positions_and_tokens() {
    let root = ":attribute-missing: warn\n\ninclude::part.adoc[tags=body,indent=0]";
    let part =
        "\u{feff}// tag::body[]\r\n  Ж {CPP} \\{missing} {missing} {missing\\}\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    let start = part.find(" {missing} ").unwrap() + 1;
    let span = Span::in_source(SourceId(1), start, start + "{missing}".len());
    assert_eq!(result.diagnostics[0].span, span);
    let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
    let refs: Vec<_> = ts
        .iter()
        .filter(|t| t.kind == TokenKind::AttributeRef)
        .collect();
    assert_eq!(refs.len(), 2);
    assert!(refs.iter().any(|t| t.span == span));
    assert!(refs.iter().any(|t| sources.slice(t.span) == Some("{CPP}")));
    check_spans(&result, &sources);
}

#[test]
fn inline_attribute_drop_line_keeps_surviving_cross_source_text_and_reference_tokens() {
    let root = ":attribute-missing: drop-line\n\nfirst\n// removed\ninclude::part.adoc[lines=2..3,indent=0]\nlast";
    let part = "omitted\r\n  discarded {missing}\r\n  Ж {cpp}\r\n";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
        panic!()
    };
    assert!(inlines.iter().any(
        |n| matches!(n, Inline::AttributeRef { name, value: Some(v), span, .. }
        if name == "missing" && v.is_empty() && sources.slice(*span) == Some("{missing}"))
    ));
    assert!(!format!("{inlines:?}").contains("discarded"));
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(sources.slice(result.diagnostics[0].span), Some("{missing}"));
    assert_eq!(result.diagnostics[0].severity, adoctra_core::Severity::Info);
    let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(ts
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && sources.slice(t.span) == Some("{missing}")));
    check_spans(&result, &sources);
}

#[test]
fn shared_attribute_stage_warnings_restore_selected_definition_and_pass_origins() {
    let root = ":attribute-missing: warn\n\ninclude::part.adoc[tags=body,indent=0]";
    let part = "\u{feff}// tag::body[]\r\n  :v: {missing}\r\n\r\n  Ж {v} pass:a[{other}]\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 2, "{result:?}");
    for (d, expected) in result
        .diagnostics
        .iter()
        .zip([":v: {missing}", "pass:a[{other}]"])
    {
        assert_eq!(d.span.source_id, SourceId(1));
        assert_eq!(sources.slice(d.span), Some(expected));
        assert_eq!(d.severity, adoctra_core::Severity::Warning);
    }
    let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
    for (kind, text) in [
        (TokenKind::AttributeEntry, ":v: {missing}"),
        (TokenKind::PassthroughText, "pass:a[{other}]"),
    ] {
        assert!(ts
            .iter()
            .any(|t| t.kind == kind && sources.slice(t.span) == Some(text)));
    }
    check_spans(&result, &sources);
}

#[test]
fn multiline_attribute_warning_retains_composite_include_origin() {
    let root = ":attribute-missing: warn\n:v: before \\\ninclude::part.adoc[]\n\n{v}";
    let part = "{missing} {cpp}";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1, "{result:?}");
    let d = &result.diagnostics[0];
    let fragments = &result.provenance[d.span.provenance.expect("composite definition origin")];
    assert_eq!(fragments.first().unwrap().source_id, options.source_id);
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    assert_eq!(
        result.document.attributes.get("v"),
        Some("before {missing} C&#43;&#43;")
    );
    check_spans(&result, &sources);
}

#[test]
fn repeated_normal_passes_keep_selected_include_diagnostics_and_link_tokens() {
    let root = "include::part.adoc[tags=body,indent=0]";
    let body = r"link:https://e['\pass:bad[Ж\] +',role=x]";
    let part = format!("// tag::body[]\r\n  Ж {body}\r\n// end::body[]");
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", &part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1, "{result:?}");
    assert_eq!(result.diagnostics[0].span.source_id, SourceId(1));
    assert_eq!(sources.slice(result.diagnostics[0].span), Some(body));
    let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(ts
        .iter()
        .any(|t| t.kind == TokenKind::Link && sources.slice(t.span) == Some(body)));
    assert!(!ts.iter().any(|t| t.kind == TokenKind::PassthroughText));
    check_spans(&result, &sources);
}

#[test]
fn repeated_normal_pass_warnings_preserve_cross_source_provenance() {
    let root = "link:https://e['\\pass:bad[Ж\\]',\ninclude::part.adoc[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", "role=x]")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1, "{result:?}");
    let fragments = &result.provenance[result.diagnostics[0].span.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().source_id, options.source_id);
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    assert_eq!(sources.slice(*fragments.last().unwrap()), Some("role=x]"));
    check_spans(&result, &sources);
}

#[test]
fn stateful_definition_side_effects_keep_selected_include_origins_and_snapshots() {
    let root = ":first: {counter:n}\n\nBefore\n\ninclude::part.adoc[tag=body,indent=0]";
    let part = "// tag::body[]\r\n  :second: {set:attribute-missing:warn}{counter:n}{missing}\r\n\r\n  {first} {second} {n}\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1, "{result:?}");
    assert_eq!(result.diagnostics[0].span.source_id, SourceId(1));
    assert_eq!(
        sources.slice(result.diagnostics[0].span),
        Some(":second: {set:attribute-missing:warn}{counter:n}{missing}")
    );
    assert_eq!(result.document.attributes.get("n"), Some("1"));
    assert_eq!(
        result.document.flat_blocks()[1].attribute_context.get("n"),
        Some("2")
    );
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::AttributeEntry && t.span.source_id == SourceId(1)));
    check_spans(&result, &sources);
}

#[test]
fn stateful_multiline_definition_diagnostics_keep_composite_provenance() {
    let root = ":v: {set:attribute-missing:warn}{counter:n} \\\ninclude::part.adoc[]\n\n{v}";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", "{missing}")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1, "{result:?}");
    let spans = &result.provenance[result.diagnostics[0].span.provenance.unwrap()];
    assert_eq!(spans.first().unwrap().source_id, options.source_id);
    assert_eq!(spans.last().unwrap().source_id, SourceId(1));
    assert_eq!(result.document.attributes.get("n"), Some("1"));
    check_spans(&result, &sources);
}

#[test]
fn block_attribute_line_subs_keep_selected_include_origins() {
    let root = ":attribute-missing: warn\n\npara\n\ninclude::part.adoc[tag=body,indent=0]\n\n{n}";
    let part = "// tag::body[]\r\n  [quote, Ж {counter:n:Я} {missing}, 'w {counter:n}']\r\n  ____\r\n  x\r\n  ____\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Quote { attribution, .. } = &result.document.flat_blocks()[1].kind else {
        panic!("{result:?}");
    };
    let attribution = attribution.as_ref().unwrap();
    let author = attribution.author.as_ref().unwrap();
    let Inline::Raw { span, .. } = &author[0] else {
        panic!("{author:?}");
    };
    assert_eq!(span.source_id, SourceId(1));
    assert_eq!(sources.slice(*span), Some("Ж "));
    let counter = author
        .iter()
        .find(|n| matches!(n, Inline::AttributeRef { name, .. } if name == "counter:n:Я"))
        .unwrap();
    assert_eq!(sources.slice(counter.span()), Some("{counter:n:Я}"));
    // The repeated normal pass covers the whole transformed field.
    let cite = attribution.citetitle.as_ref().unwrap();
    assert_eq!(sources.slice(cite[0].span()), Some("w {counter:n}"));
    let warning = result
        .diagnostics
        .iter()
        .find(|d| d.message.contains("missing"))
        .unwrap();
    assert_eq!(sources.slice(warning.span), Some("{missing}"));
    // Reading effects are not replayed into the body.
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[2].kind else {
        panic!("{result:?}");
    };
    assert!(matches!(
        &inlines[0],
        Inline::AttributeRef { value: None, .. }
    ));
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == counter.span()));
}

#[test]
fn inline_anchor_catalog_keeps_selected_include_origins() {
    let root = "para\n\ninclude::part.adoc[tag=body,indent=0]\n\n<<a>> <<b>>";
    let part =
        "// tag::body[]\r\n  Ж [[a,R {counter:n} *x*]]\r\n\r\n  * [[b,L]] item\r\n// end::body[]";
    let (mut sources, resources, options) = setup(root, &[("/docs/part.adoc", part)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.flat_blocks().last().unwrap().kind
    else {
        panic!("{result:?}");
    };
    let refs: Vec<_> = inlines
        .iter()
        .filter_map(|n| match n {
            Inline::CrossReference { children, .. } => Some(children),
            _ => None,
        })
        .collect();
    assert_eq!(refs.len(), 2);
    let strong = refs[0]
        .iter()
        .find(|n| matches!(n, Inline::Strong { .. }))
        .unwrap();
    assert_eq!(strong.span().source_id, SourceId(1));
    assert_eq!(sources.slice(strong.span()), Some("*x*"));
    assert!(matches!(&refs[1][0], Inline::Text { value, .. } if value == "L"));
}

#[test]
fn body_setext_section_spans_title_and_included_underline() {
    let root = "Intro\n\nSec *Я*\ninclude::u.adoc[]\n\n<<_sec_я>>";
    let (mut sources, resources, options) =
        setup(root, &[("/docs/u.adoc", "-------\r\n\r\nBody\r\n")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    check_spans(&result, &sources);
    let block = &result.document.flat_blocks()[1];
    assert!(matches!(block.kind, BlockKind::Heading { level: 1, .. }));
    assert_eq!(block.id.as_deref(), Some("_sec_я"));
    let fragments = &result.provenance[block.span.provenance.unwrap()];
    assert!(sources
        .slice(fragments[0])
        .unwrap()
        .starts_with("Sec *Я*\n"));
    assert_eq!(fragments.last().unwrap().source_id, SourceId(1));
    assert_eq!(sources.slice(*fragments.last().unwrap()), Some("-------"));
    let highlighted = tokens_resource(&mut sources, &options, &resources).unwrap();
    let headings: Vec<_> = highlighted
        .iter()
        .filter(|t| t.kind == TokenKind::Heading)
        .map(|t| (t.span.source_id, sources.slice(t.span).unwrap()))
        .collect();
    assert_eq!(
        headings,
        [(options.source_id, "Sec *Я*"), (SourceId(1), "-------")]
    );
}

#[test]
fn manual_page_name_section_keeps_include_origins() {
    let root =
        "= x(1)\n:doctype: manpage\n:attribute-missing: warn\n\ninclude::name.adoc[]\n\n== DESC";
    let name = "== NAME\r\n\r\n*a* - {missing}\r\n  more";
    let (mut sources, resources, options) = setup(root, &[("/docs/name.adoc", name)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let section = result
        .document
        .manpage
        .as_ref()
        .and_then(|manpage| manpage.name_section.as_ref())
        .unwrap();
    assert_eq!(section.heading.span.source_id, SourceId(1));
    assert_eq!(sources.slice(section.heading.span), Some("== NAME"));
    let body = section.body.unwrap();
    assert_eq!(body.source_id, SourceId(1));
    assert_eq!(sources.slice(body), Some("*a* - {missing}\r\n  more"));
    assert_eq!(
        result.document.attributes.get("manpurpose"),
        Some("{missing} more")
    );
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(sources.slice(result.diagnostics[0].span), Some("{missing}"));
    let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(ts
        .iter()
        .any(|t| t.kind == TokenKind::Heading && t.span == section.heading.span));
    assert!(!ts.iter().any(|t| t.kind == TokenKind::Strong));
    check_spans(&result, &sources);
}

#[test]
fn unreadable_includes_become_unresolved_directive_lines_like_ruby() {
    // Ruby's `replace_next_line %(Unresolved directive in #{@path} - …)`:
    // the primary document's file name or the include's path relative to the
    // document directory, the expanded target (unexpanded when blank) and the
    // raw attribute list.
    let root = ":attribute-missing: warn\n:d: missing\n:empty:\n\n\
                include::{d}.adoc[opts=x, lines=1]\n\ninclude::ch/a.adoc[]\n\n\
                include::{nope}[]\n\ninclude::{empty}x{empty}[]\n\n\
                include::gone.adoc[opts=optional]\n\n----\ninclude::code.rb[]\n----";
    let (mut sources, resources, options) =
        setup(root, &[("/docs/ch/a.adoc", "include::../../lib/b.adoc[]")]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let blocks = result.document.flat_blocks();
    let paragraphs: Vec<String> = blocks
        .iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => Some(
                inlines
                    .iter()
                    .map(|inline| match inline {
                        Inline::Text { value, .. } => value.clone(),
                        Inline::AttributeRef { name, value, .. } => {
                            value.clone().unwrap_or_else(|| format!("{{{name}}}"))
                        }
                        _ => panic!("unexpected inline {inline:?}"),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(
        paragraphs,
        [
            "Unresolved directive in main.adoc - include::missing.adoc[opts=x, lines=1]",
            "Unresolved directive in ch/a.adoc - include::../../lib/b.adoc[]",
            // The generated line gets its own substitutions, like Ruby's.
            "Unresolved directive in main.adoc - include::{nope}[]",
            "Unresolved directive in main.adoc - include::x[]",
        ]
    );
    let BlockKind::Listing { text, .. } = &blocks.last().unwrap().kind else {
        panic!("{blocks:?}");
    };
    assert_eq!(
        text,
        "Unresolved directive in main.adoc - include::code.rb[]"
    );
    // The generated text maps to its directive and has no tokens of its own.
    let first = blocks
        .iter()
        .find(|b| matches!(b.kind, BlockKind::Paragraph { .. }))
        .unwrap();
    assert_eq!(
        sources.slice(first.span),
        Some("include::{d}.adoc[opts=x, lines=1]")
    );
    check_spans(&result, &sources);
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(!tokens
        .iter()
        .any(|t| sources.slice(t.span) == Some("Unresolved")));
}

#[test]
fn exceeded_include_depth_keeps_the_directive_like_ruby() {
    let (mut sources, resources, mut options) =
        setup("include::part.adoc[]", &[("/docs/part.adoc", "content")]);
    options.includes = IncludeLimits {
        max_depth: 0,
        ..IncludeLimits::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&result), ["include::part.adoc[]"]);
}

fn depth_errors(result: &ParseResult) -> Vec<&str> {
    result
        .diagnostics
        .iter()
        .filter(|d| d.message.starts_with("maximum include depth"))
        .map(|d| d.message.as_str())
        .collect()
}

const DEPTH_FILES: &[(&str, &str)] = &[
    ("/docs/part.adoc", "include::nested.adoc[]"),
    ("/docs/nested.adoc", "include::leaf.adoc[]"),
    ("/docs/leaf.adoc", "leaf"),
    ("/docs/p5.adoc", "include::part.adoc[depth=5]"),
];

#[test]
fn max_include_depth_attribute_bounds_includes_like_ruby() {
    let (mut sources, resources, mut options) =
        setup("include::part.adoc[]\n\ninclude::p5.adoc[]", DEPTH_FILES);
    options.attributes.set("max-include-depth", "1");
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(
        paragraphs(&result),
        ["include::nested.adoc[]", "include::part.adoc[depth=5]"]
    );
    assert_eq!(
        depth_errors(&result),
        [
            "maximum include depth of 1 exceeded",
            "maximum include depth of 1 exceeded"
        ]
    );
    let error = &result.diagnostics[0];
    assert_eq!(sources.slice(error.span), Some("include::nested.adoc[]"));
}

#[test]
fn relative_depth_beyond_max_include_depth_is_replaced_by_it() {
    let (mut sources, resources, mut options) =
        setup("include::part.adoc[]\n\ninclude::p5.adoc[]", DEPTH_FILES);
    options.attributes.set("max-include-depth", "2");
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    // A relative depth of 5 under p5 becomes the absolute maximum of 2.
    assert_eq!(
        paragraphs(&result),
        ["include::leaf.adoc[]", "include::nested.adoc[]"]
    );
    assert_eq!(
        depth_errors(&result),
        [
            "maximum include depth of 2 exceeded",
            "maximum include depth of 2 exceeded"
        ]
    );
}

#[test]
fn max_include_depth_without_a_positive_value_disables_includes() {
    for value in ["0", "-3", "wrong", ""] {
        let (mut sources, resources, mut options) =
            setup("include::part.adoc[]\n\ninclude::p5.adoc[]", DEPTH_FILES);
        options.attributes.set("max-include-depth", value);
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(
            paragraphs(&result),
            ["include::part.adoc[]", "include::p5.adoc[]"],
            "{value:?}"
        );
        assert!(result.diagnostics.is_empty(), "{value:?}");
        let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
        assert!(tokens.iter().all(|t| t.span.source_id == options.source_id));
    }
}

#[test]
fn max_include_depth_is_read_before_the_header_like_ruby() {
    let (mut sources, resources, options) =
        setup(":max-include-depth: 0\n\ninclude::part.adoc[]", DEPTH_FILES);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&result), ["leaf"]);
}

#[test]
fn max_include_depth_attribute_never_exceeds_the_host_limit() {
    let (mut sources, resources, mut options) = setup("include::part.adoc[]", DEPTH_FILES);
    options.attributes.set("max-include-depth", "1_0");
    options.includes.max_depth = 1;
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&result), ["include::nested.adoc[]"]);
    assert_eq!(
        depth_errors(&result),
        ["maximum include depth of 1 exceeded"]
    );
}

#[test]
fn zero_relative_depth_reports_a_depth_of_zero() {
    let (mut sources, resources, options) = setup("include::part.adoc[depth=0]", DEPTH_FILES);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(paragraphs(&result), ["include::nested.adoc[]"]);
    assert_eq!(
        depth_errors(&result),
        ["maximum include depth of 0 exceeded"]
    );
}

/// Asciidoctor 2.0.23 reader_test.rb: "include directive should resolve file
/// relative to current include". The source snapshots are byte-for-byte upstream.
#[test]
fn upstream_nested_include_cursors_map_to_physical_sources() {
    let root = include_str!("../../../compatibility/upstream/fixtures/include-relative.adoc");
    let parent =
        include_str!("../../../compatibility/upstream/sources/test/fixtures/parent-include.adoc");
    let child =
        include_str!("../../../compatibility/upstream/sources/test/fixtures/child-include.adoc");
    let grandchild = include_str!(
        "../../../compatibility/upstream/sources/test/fixtures/grandchild-include.adoc"
    );
    let (mut sources, resources, options) = setup(
        root,
        &[
            ("/docs/fixtures/parent-include.adoc", parent),
            ("/docs/fixtures/child-include.adoc", child),
            ("/docs/fixtures/grandchild-include.adoc", grandchild),
        ],
    );
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let blocks = result.document.flat_blocks();
    assert_eq!(blocks.len(), 6);
    for (block, (id, line, text)) in blocks.iter().zip([
        (1, 1, "first line of parent"),
        (2, 1, "first line of child"),
        (3, 1, "first line of grandchild"),
        (3, 3, "last line of grandchild"),
        (2, 5, "last line of child"),
        (1, 5, "last line of parent"),
    ]) {
        assert!(matches!(block.kind, BlockKind::Paragraph { .. }));
        assert_eq!(block.span.source_id, SourceId(id));
        assert_eq!(sources.slice(block.span).unwrap().trim_end(), text);
        let position = sources
            .get(SourceId(id))
            .unwrap()
            .position(block.span.start)
            .unwrap();
        assert_eq!((position.line, position.column), (line, 1));
    }
    check_spans(&result, &sources);
}

#[test]
fn soft_host_depth_uses_normalized_name_before_reading_includes() {
    for key in ["max-include-depth", "max-include-depth@"] {
        let (mut sources, resources, mut options) =
            setup("include::part.adoc[]", &[("/docs/part.adoc", "content")]);
        options.attributes.set(key, "0@");
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(sources.get(SourceId(1)).is_none());
        assert!(result
            .document
            .flat_blocks()
            .iter()
            .any(|block| matches!(&block.kind, BlockKind::Paragraph { .. })));
    }
}

#[test]
fn upstream_tag_selection_retains_physical_line_numbers() {
    let root = include_str!("../../../compatibility/upstream/fixtures/include-tags.adoc");
    let included =
        include_str!("../../../compatibility/upstream/sources/test/fixtures/include-file.adoc");
    let (mut sources, resources, options) =
        setup(root, &[("/docs/fixtures/include-file.adoc", included)]);
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let blocks = result.document.flat_blocks();
    let BlockKind::Paragraph { inlines } = &blocks[0].kind else {
        panic!()
    };
    let Inline::Text { value, span } = &inlines[0] else {
        panic!()
    };
    assert_eq!(value, "snippetA content snippetB content");
    let fragments = &result.provenance[span.provenance.unwrap()];
    for (text, line) in [("snippetA content", 12), ("snippetB content", 18)] {
        let fragment = fragments
            .iter()
            .find(|s| sources.slice(**s).unwrap().contains(text))
            .unwrap();
        assert_eq!(fragment.source_id, SourceId(1));
        assert_eq!(sources.slice(*fragment).unwrap().trim_end(), text);
        assert_eq!(
            sources
                .get(SourceId(1))
                .unwrap()
                .position(fragment.start)
                .unwrap()
                .line,
            line
        );
    }
    check_spans(&result, &sources);
}
