use adoctra_core::{
    parse_resource, BlockKind, Inline, MemoryResources, ParseOptions, Resource, ResourceErrorKind,
    ResourceProvider, ResourceRequest, ResourceResult, SourceId, SourceMap,
};

fn setup(source: &str) -> (MemoryResources, SourceMap, ParseOptions) {
    let mut resources = MemoryResources::new();
    resources.insert("/docs/main.adoc", source).unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    (
        resources,
        sources,
        ParseOptions {
            source_id,
            ..Default::default()
        },
    )
}

#[test]
fn big5_selections_keep_decoded_utf8_spans_and_physical_columns() {
    for (encoding, glyph, decoded) in [
        ("Big5", b"\xa4\xa4", "中"),
        ("Big5-HKSCS", b"\x8e\x40", "𣻗"),
        ("Big5-HKSCS:2008", b"\x8e\x40", "𣻗"),
        ("Big5-UAO", b"\x81\x40", "丗"),
        ("CP950", b"\x81\x40", "\u{eeb8}"),
        ("CP951", b"\xfe\x40", "鑂"),
    ] {
        for selection in ["lines=3", "tag=body"] {
            let directive = format!("include::part.adoc[encoding={encoding},{selection},indent=0]");
            let (mut resources, mut sources, options) =
                setup(&format!(":attribute-missing: warn\n\n{directive}"));
            resources
                .insert_binary(
                    "/docs/part.adoc",
                    [
                        &b"ignored\r\n// tag::body[]\r\n  "[..],
                        glyph,
                        b" {missing}\r\n// end::body[]\r\n",
                    ]
                    .concat(),
                )
                .unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            assert_eq!(
                result.diagnostics.len(),
                1,
                "{encoding}: {:?}",
                result.diagnostics
            );
            let source = sources.get(SourceId(1)).unwrap();
            assert_eq!(
                source.text(),
                format!(
                    "ignored\r\n// tag::body[]\r\n  {decoded} {{missing}}\r\n// end::body[]\r\n"
                )
            );
            assert!(source.is_complete());
            assert!(source.omitted_lines().is_empty());
            let diagnostic = &result.diagnostics[0];
            assert_eq!(sources.slice(diagnostic.span), Some("{missing}"));
            assert_eq!(
                source.position(diagnostic.span.start).unwrap(),
                adoctra_core::SourcePosition { line: 3, column: 5 }
            );
            let block = result.document.flat_blocks()[0];
            assert!(matches!(block.kind, BlockKind::Paragraph { .. }));
            assert_eq!(
                sources.slice(block.span),
                Some(format!("{decoded} {{missing}}").as_str())
            );
            assert!(result
                .tokens
                .iter()
                .all(|token| sources.slice(token.span).is_some()));
        }
    }
}

#[test]
fn big5_variants_reuse_only_identical_decoded_snapshots() {
    let names = [
        "Big5",
        "Big5-HKSCS",
        "Big5-UAO",
        "CP950",
        "CP951",
        "Big5-HKSCS:2008",
    ];
    let root = names
        .map(|name| format!("include::part.adoc[encoding={name}]"))
        .join("\n\n");
    let (mut resources, mut sources, options) = setup(&root);
    resources
        .insert_binary("/docs/part.adoc", b"\xa1\x45\xc6\xa1\r\n")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "‧\u{f6b1}\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "•①\r\n");
    assert_eq!(sources.get(SourceId(3)).unwrap().text(), "‧①\r\n");
    assert_eq!(
        result
            .document
            .flat_blocks()
            .iter()
            .map(|block| block.span.source_id.0)
            .collect::<Vec<_>>(),
        [1, 2, 3, 1, 3, 2]
    );
    resources
        .insert_binary("/docs/part.adoc", b"\xa4\xa4\r\n")
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(changed.diagnostics.is_empty());
    assert!(changed
        .document
        .flat_blocks()
        .iter()
        .all(|block| block.span.source_id == SourceId(4)));
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "•①\r\n");
}

#[test]
fn big5_prefix_recovery_does_not_skip_invalid_consumed_bytes() {
    for name in ["Big5", "Big5-HKSCS", "Big5-UAO", "CP950", "CP951"] {
        for (selection, bytes, success) in [
            ("lines=1", &b"*\xa4\xa4*\r\n\x81"[..], true),
            ("lines=2", &b"*\xa4\xa4*\r\n\x81"[..], false),
            ("lines=1;3", &b"first\n\x81\nlast\n"[..], false),
            (
                "tag=body",
                &b"// tag::body[]\n\xa4\xa4\n// end::body[]\n\x81"[..],
                false,
            ),
        ] {
            let directive =
                format!("include::part.adoc[encoding={name},{selection},opts=optional]");
            let (mut resources, mut sources, options) = setup(&directive);
            resources.insert_binary("/docs/part.adoc", bytes).unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            if success {
                assert!(result.diagnostics.is_empty());
                let source = sources.get(SourceId(1)).unwrap();
                assert_eq!(source.text(), "*中*\r\n");
                assert!(!source.is_complete());
                assert!(source.omitted_lines().is_empty());
                assert_eq!(
                    sources.slice(result.document.flat_blocks()[0].span),
                    Some("*中*")
                );
                assert_eq!(
                    sources
                        .load_with_encoding(
                            "part.adoc",
                            Some(options.source_id),
                            &resources,
                            Some(name)
                        )
                        .unwrap_err()
                        .kind,
                    ResourceErrorKind::InvalidEncoding
                );
            } else {
                assert_eq!(result.diagnostics.len(), 1);
                assert_eq!(
                    sources.slice(result.diagnostics[0].span),
                    Some(directive.as_str())
                );
                assert_eq!(
                    result.diagnostics[0].report.message.as_deref(),
                    Some("include file not readable: /docs/part.adoc")
                );
                assert!(sources.get(SourceId(1)).is_none());
            }
        }
    }
}

#[test]
fn binary_tag_markers_preserve_omitted_bytes_and_selected_source_positions() {
    for encoding in ["BINARY", "ASCII-8BIT"] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding={encoding},tag=body,indent=0]"
        ));
        let bytes = b"\xff\r\n\xfe// tag::body[] \x80\r\n  *good*\r\n// end::body[] \x81\r\n";
        resources.insert_binary("/docs/part.adoc", bytes).unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let block = result.document.flat_blocks()[0];
        assert!(matches!(block.kind, BlockKind::Paragraph { .. }));
        assert_eq!(sources.slice(block.span), Some("*good*"));
        let file = sources.get(block.span.source_id).unwrap();
        assert_eq!(file.text(), "  *good*\r\n");
        assert!(!file.is_complete());
        assert_eq!(
            file.position(block.span.start).unwrap(),
            adoctra_core::SourcePosition { line: 3, column: 3 }
        );
        assert_eq!(
            file.omitted_lines()
                .iter()
                .map(|line| line.line)
                .collect::<Vec<_>>(),
            [1, 2, 4]
        );
        assert_eq!(
            file.omitted_lines()[1].bytes,
            b"\xfe// tag::body[] \x80\r\n"
        );
        assert!(result
            .tokens
            .iter()
            .all(|token| sources.slice(token.span).is_some()));
        let repeated = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(
            repeated.document.flat_blocks()[0].span.source_id,
            block.span.source_id
        );
        let mut limited = options.clone();
        limited.includes.max_bytes = bytes.len() - 1;
        let limited = parse_resource(&mut sources, &limited, &resources).unwrap();
        assert!(limited
            .diagnostics
            .iter()
            .any(|d| d.message.contains("maximum included byte count")));
    }
}

#[test]
fn omitted_marker_diagnostics_identify_distinct_raw_ranges_at_the_same_anchor() {
    let (mut resources, mut sources, options) =
        setup("include::part.adoc[encoding=BINARY,tag=body]");
    resources
        .insert_binary(
            "/docs/part.adoc",
            b"\xff// end::body[]\r\n\xfe// tag::body[]\r\n*good*\r\n",
        )
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 2);
    let file = sources.get(SourceId(1)).unwrap();
    for (diagnostic, line, marker) in [
        (&result.diagnostics[0], 1, b"end::body[]"),
        (&result.diagnostics[1], 2, b"tag::body[]"),
    ] {
        assert_eq!(
            diagnostic.span,
            adoctra_core::Span::in_source(SourceId(1), 0, 0)
        );
        assert_eq!(sources.slice(diagnostic.span), Some(""));
        let raw = diagnostic.omitted_span.unwrap();
        assert_eq!(
            raw,
            adoctra_core::OmittedSourceSpan {
                line,
                start: 4,
                end: 15
            }
        );
        assert_eq!(file.omitted_slice(raw), Some(marker.as_slice()));
        assert_eq!(
            diagnostic.report.location,
            adoctra_core::DiagnosticLocation::Cursor(adoctra_core::DiagnosticCursor {
                source_id: options.source_id,
                line: 1
            })
        );
    }
    assert_eq!(
        file.omitted_slice(adoctra_core::OmittedSourceSpan {
            line: 3,
            start: 0,
            end: 1
        }),
        None
    );
    assert_eq!(
        file.omitted_slice(adoctra_core::OmittedSourceSpan {
            line: 1,
            start: 99,
            end: 100
        }),
        None
    );
    let old = result.diagnostics[0].span.source_id;
    resources
        .insert_binary(
            "/docs/part.adoc",
            b"\x80// end::body[]\r\n\xfe// tag::body[]\r\n*good*\r\n",
        )
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_ne!(changed.diagnostics[0].span.source_id, old);
    assert_eq!(sources.get(old).unwrap().omitted_lines()[0].bytes[0], 0xff);
}

#[test]
fn binary_tag_names_and_word_boundaries_are_byte_oriented() {
    for byte in 128..=255 {
        let (mut resources, mut sources, options) =
            setup("include::part.adoc[encoding=BINARY,tag=body]");
        let bytes = [
            &[byte][..],
            b"tag::body[]\ngood\n",
            &[byte],
            b"end::body[]\n",
        ]
        .concat();
        resources.insert_binary("/docs/part.adoc", bytes).unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{byte}: {:?}",
            result.diagnostics
        );
        assert_eq!(
            sources.slice(result.document.flat_blocks()[0].span),
            Some("good")
        );
    }
    for selection in ["tags=*", "tags=**", "tags=!*", "tag=Я"] {
        let (mut resources, mut sources, options) =
            setup(&format!("include::part.adoc[encoding=BINARY,{selection}]"));
        resources
            .insert_binary(
                "/docs/part.adoc",
                b"// tag::\xff[]\n// tag::\xfe[]\ngood\n// end::\xfe[]\n// end::\xff[]\n",
            )
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), usize::from(selection == "tag=Я"));
        assert_eq!(
            result.document.flat_blocks().len(),
            usize::from(matches!(selection, "tags=*" | "tags=**"))
        );
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[encoding=BINARY,tag=Я]");
    resources
        .insert_binary(
            "/docs/part.adoc",
            "// tag::Я[]\ngood\n// end::Я[]\n".as_bytes(),
        )
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.document.flat_blocks().is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].message.contains("not found"));
}

#[test]
fn invalid_binary_tag_names_have_lossless_diagnostics_without_panicking() {
    let (mut resources, mut sources, options) = setup("include::part.adoc[encoding=BINARY,tags=*]");
    resources
        .insert_binary("/docs/part.adoc", b"// tag::\xff[]\ngood\n")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].message, "unclosed tag '<binary:ff>'");
    assert_eq!(
        sources
            .get(SourceId(1))
            .unwrap()
            .omitted_slice(result.diagnostics[0].omitted_span.unwrap()),
        Some(&b"tag::\xff[]"[..])
    );
}

#[test]
fn physical_tag_warnings_precede_missing_tag_reports_across_repeated_includes() {
    let directive = "include::part.adoc[encoding=BINARY,tag=body]";
    let (mut resources, mut sources, options) = setup(&format!("{directive}\n\n{directive}"));
    resources
        .insert_binary(
            "/docs/part.adoc",
            b"// tag::body[]\xff\ngood\n// end::body[]\n",
        )
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 4);
    for (pair, line) in result.diagnostics.chunks_exact(2).zip([1, 3]) {
        assert_eq!(pair[0].message, "unexpected end tag 'body'");
        assert_eq!(sources.slice(pair[0].span), Some("// end::body[]"));
        assert_eq!(
            pair[1].message,
            "tag 'body' not found in include: part.adoc"
        );
        assert_eq!(sources.slice(pair[1].span), Some(directive));
        assert_eq!(
            pair[0].report.location,
            adoctra_core::DiagnosticLocation::Cursor(adoctra_core::DiagnosticCursor {
                source_id: options.source_id,
                line
            })
        );
    }
}

#[test]
fn raw_markers_do_not_hide_selected_invalid_content_or_utf8_regexp_errors() {
    for (encoding, selection, bytes) in [
        (
            "UTF-8",
            "tag=body",
            &b"\xff// tag::body[]\ngood\n// end::body[]\n"[..],
        ),
        (
            "BINARY",
            "lines=1,tag=body",
            &b"\xff// tag::body[]\ngood\n// end::body[]\n"[..],
        ),
        (
            "BINARY",
            "tag=body",
            &b"\xff// tag::body[]\n\xff\n// end::body[]\n"[..],
        ),
    ] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding={encoding},{selection},opts=optional]"
        ));
        resources.insert_binary("/docs/part.adoc", bytes).unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0]
            .report
            .message
            .as_ref()
            .unwrap()
            .contains("not readable"));
        assert!(sources.get(SourceId(1)).is_none());
    }
}

#[test]
fn selected_bom_is_removed_without_shifting_snapshot_or_physical_positions() {
    for encoding in ["UTF-8", "BINARY"] {
        for (selection, bytes, line) in [
            ("lines=2", &b"\xff\r\n\xef\xbb\xbf*good*\r\n"[..], 2),
            (
                "tag=body",
                &b"\xff\n// tag::body[]\n\xef\xbb\xbf*good*\n// end::body[]\n"[..],
                3,
            ),
            ("lines=2", &b"skip\n\xef\xbb\xbf*good*\n"[..], 2),
        ] {
            let (mut resources, mut sources, options) = setup(&format!(
                "include::part.adoc[encoding={encoding},{selection},indent=0]"
            ));
            resources.insert_binary("/docs/part.adoc", bytes).unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            assert!(
                result.diagnostics.is_empty(),
                "{encoding}/{selection}: {:?}",
                result.diagnostics
            );
            let block = result.document.flat_blocks()[0];
            assert!(matches!(&block.kind, BlockKind::Paragraph { .. }));
            assert_eq!(sources.slice(block.span), Some("*good*"));
            let file = sources.get(block.span.source_id).unwrap();
            assert!(file.text().contains("\u{feff}*good*"));
            let position = file.position(block.span.start).unwrap();
            assert_eq!((position.line, position.column), (line, 2));
            assert!(result
                .tokens
                .iter()
                .all(|token| sources.slice(token.span).is_some()));
        }
    }
}

#[test]
fn bom_on_a_later_selected_line_is_not_normalized_as_a_leading_bom() {
    for encoding in ["UTF-8", "BINARY"] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding={encoding},lines=1;3]"
        ));
        resources
            .insert_binary("/docs/part.adoc", b"first\n\xff\n\xef\xbb\xbflast\n")
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        if encoding == "UTF-8" {
            assert!(result.diagnostics.is_empty());
            assert_eq!(
                sources.slice(result.document.flat_blocks()[0].span),
                Some("first\n\u{feff}last")
            );
        } else {
            assert_eq!(result.diagnostics.len(), 1);
            assert_eq!(
                result.diagnostics[0].report.message.as_deref(),
                Some("include file not readable: /docs/part.adoc")
            );
            assert!(sources.get(SourceId(1)).is_none());
        }
    }
}

#[test]
fn skipped_invalid_utf8_lines_have_explicit_bytes_and_physical_positions() {
    let root = ":attribute-missing: warn\n\ninclude::part.adoc[lines=2;4;6,indent=0]";
    let (mut resources, mut sources, options) = setup(root);
    let first = "  Ж {missing}\r\n";
    let second = "  🙂 {missing}\r\n";
    let last = "  last\r\n";
    resources
        .insert_binary(
            "/docs/part.adoc",
            [
                &b"\xff\r\n"[..],
                first.as_bytes(),
                &b"\xc0\xaf\n"[..],
                second.as_bytes(),
                &b"\xed\xa0\x80\r\n"[..],
                last.as_bytes(),
                &b"\xff"[..],
            ]
            .concat(),
        )
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
    let file = sources.get(SourceId(1)).unwrap();
    assert!(!file.is_complete());
    assert_eq!(file.text(), format!("{first}{second}{last}"));
    let omitted = file.omitted_lines();
    assert_eq!(omitted.len(), 3);
    for (actual, line, offset, bytes) in [
        (&omitted[0], 1, 0, &b"\xff\r\n"[..]),
        (&omitted[1], 3, first.len(), &b"\xc0\xaf\n"[..]),
        (
            &omitted[2],
            5,
            first.len() + second.len(),
            &b"\xed\xa0\x80\r\n"[..],
        ),
    ] {
        assert_eq!(
            (actual.line, actual.offset, actual.bytes.as_slice()),
            (line, offset, bytes)
        );
    }
    for (diagnostic, line) in result.diagnostics.iter().zip([2, 4]) {
        assert_eq!(sources.slice(diagnostic.span), Some("{missing}"));
        let position = file.position(diagnostic.span.start).unwrap();
        assert_eq!((position.line, position.column), (line, 5));
    }
    let block = result.document.flat_blocks()[0];
    assert_eq!(block.span.source_id, SourceId(1));
    assert!(matches!(&block.kind, BlockKind::Paragraph { .. }));
    assert!(result
        .tokens
        .iter()
        .all(|token| sources.slice(token.span).is_some()));
    // A sparse source cannot be parsed as an implicitly valid whole document.
    assert_eq!(
        parse_resource(
            &mut sources,
            &ParseOptions {
                source_id: SourceId(1),
                ..Default::default()
            },
            &resources
        )
        .unwrap_err()
        .kind,
        ResourceErrorKind::InvalidEncoding
    );
}

#[test]
fn tag_selection_skips_invalid_plain_lines_but_validates_selected_or_scanned_bytes() {
    for encoding in ["UTF-8", "BINARY", "unknown-codec"] {
        let root = format!("include::part.adoc[encoding={encoding},tag=body]");
        let (mut resources, mut sources, options) = setup(&root);
        resources
            .insert_binary(
                "/docs/part.adoc",
                b"\xff\n// tag::body[]\n*good*\n// end::body[]\n\xff",
            )
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{encoding}: {:?}",
            result.diagnostics
        );
        let source = sources.get(SourceId(1)).unwrap();
        assert_eq!(source.text(), "// tag::body[]\n*good*\n// end::body[]\n");
        assert_eq!(
            source
                .omitted_lines()
                .iter()
                .map(|line| line.line)
                .collect::<Vec<_>>(),
            [1, 5]
        );
        let block = result.document.flat_blocks()[0];
        assert_eq!(sources.slice(block.span), Some("*good*"));
        assert_eq!(source.position(block.span.start).unwrap().line, 3);
    }
    for (encoding, selection, bytes) in [
        ("UTF-8", "lines=2", &b"good\n\xff\nlast\n"[..]),
        ("BINARY", "lines=2", &b"good\n\xff\nlast\n"[..]),
        ("Shift_JIS", "lines=1;3", &b"good\n\xff\nlast\n"[..]),
        (
            "UTF-8",
            "tag=body",
            &b"// tag::body[]\n\xff\n// end::body[]\n"[..],
        ),
        (
            "UTF-8",
            "tag=body",
            &b"// tag::body[]\ngood\n// end::body[]\n\xff :: []"[..],
        ),
    ] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding={encoding},{selection},opts=optional]"
        ));
        resources.insert_binary("/docs/part.adoc", bytes).unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 1, "{encoding}/{selection}");
        assert_eq!(
            result.diagnostics[0].report.message.as_deref(),
            Some("include file not readable: /docs/part.adoc")
        );
        assert!(sources.get(SourceId(1)).is_none());
    }
}

#[test]
fn omitted_bytes_are_part_of_immutable_snapshot_identity_and_resource_budget() {
    let (mut resources, mut sources, options) = setup("include::part.adoc[lines=1;3]");
    resources
        .insert_binary("/docs/part.adoc", b"first\n\xff\nlast\n")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let again = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(again.document.flat_blocks()[0].span.source_id, SourceId(1));
    resources
        .insert_binary("/docs/part.adoc", b"first\n\xfe\nlast\n")
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(
        changed.document.flat_blocks()[0].span.source_id,
        SourceId(2)
    );
    assert_eq!(
        sources.get(SourceId(1)).unwrap().omitted_lines()[0].bytes,
        b"\xff\n"
    );
    assert_eq!(
        sources.get(SourceId(2)).unwrap().omitted_lines()[0].bytes,
        b"\xfe\n"
    );
    assert_eq!(
        sources.get(SourceId(1)).unwrap().text(),
        sources.get(SourceId(2)).unwrap().text()
    );
    assert_eq!(
        sources
            .load_with_encoding("part.adoc", Some(options.source_id), &resources, None)
            .unwrap_err()
            .kind,
        ResourceErrorKind::InvalidEncoding
    );
    let mut limited = options.clone();
    limited.includes.max_bytes = "first\nlast\n".len();
    let result = parse_resource(&mut sources, &limited, &resources).unwrap();
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("maximum included byte count")));
}

#[test]
fn sparse_nested_includes_and_tag_warnings_keep_physical_locations() {
    let (mut resources, mut sources, options) = setup("include::sub/outer.adoc[lines=1;3;4]");
    resources
        .insert_binary(
            "/docs/sub/outer.adoc",
            b"outer\n\xff\n\ninclude::inner.adoc[lines=1;3]\n",
        )
        .unwrap();
    resources
        .insert_binary("/docs/sub/inner.adoc", b"first\n\xff\nlast\n")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = result.document.flat_blocks();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[1].span.source_id, SourceId(2));
    assert_eq!(
        sources.get(SourceId(2)).unwrap().name(),
        "/docs/sub/inner.adoc"
    );
    assert_eq!(
        sources
            .get(SourceId(1))
            .unwrap()
            .position("outer\n\n".len())
            .unwrap()
            .line,
        4
    );
    assert_eq!(
        sources
            .get(SourceId(2))
            .unwrap()
            .position("first\n".len())
            .unwrap()
            .line,
        3
    );

    let (mut resources, mut sources, options) = setup("include::part.adoc[tag=body]");
    resources
        .insert_binary(
            "/docs/part.adoc",
            b"\xff\n// end::body[]\n\xfe\n// tag::body[]\ngood\n",
        )
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
    let source = sources.get(SourceId(1)).unwrap();
    for (diagnostic, line, marker) in [
        (&result.diagnostics[0], 2, "// end::body[]"),
        (&result.diagnostics[1], 4, "// tag::body[]"),
    ] {
        assert_eq!(sources.slice(diagnostic.span), Some(marker));
        assert_eq!(source.position(diagnostic.span.start).unwrap().line, line);
    }
    assert!(result.diagnostics[1]
        .report
        .message
        .as_deref()
        .unwrap()
        .contains("starting at line 4"));
}

#[test]
fn skipped_non_ascii_bytes_and_empty_selections_remain_explicit() {
    for encoding in ["UTF-8", "BINARY"] {
        let mut bytes = b"first\n".to_vec();
        for byte in 128..=255 {
            bytes.extend_from_slice(&[byte, b'\n']);
        }
        bytes.extend_from_slice(b"last\n");
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding={encoding},lines=1;130]"
        ));
        resources.insert_binary("/docs/part.adoc", bytes).unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty());
        let source = sources.get(SourceId(1)).unwrap();
        assert_eq!(source.text(), "first\nlast\n");
        assert_eq!(source.omitted_lines().len(), 128);
        assert_eq!(source.position(6).unwrap().line, 130);
    }
    for selection in ["lines=0", "lines=9", "tag=absent"] {
        let (mut resources, mut sources, options) =
            setup(&format!("include::part.adoc[{selection}]"));
        resources
            .insert_binary("/docs/part.adoc", b"\xff\n\xfe")
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.document.flat_blocks().is_empty());
        assert_eq!(
            result.diagnostics.len(),
            usize::from(selection == "tag=absent")
        );
        let source = sources.get(SourceId(1)).unwrap();
        assert_eq!(source.text(), "");
        assert_eq!(source.omitted_lines().len(), 2);
        assert_eq!(source.omitted_lines()[1].bytes, b"\xfe");
    }
}

#[test]
fn sparse_tag_reports_keep_ruby_names_order_and_physical_line_numbers() {
    let (mut resources, mut sources, options) = setup("include::part.adoc[tags=z;a;b]");
    resources
        .insert_binary("/docs/part.adoc", b"\xff\n")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 3);
    assert_eq!(
        result
            .diagnostics
            .iter()
            .filter(|d| !d.report.suppressed)
            .count(),
        1
    );
    assert_eq!(
        result.diagnostics[0].report.message.as_deref(),
        Some("tags 'z, a, b' not found in include file: /docs/part.adoc")
    );
    for (diagnostic, name) in result.diagnostics.iter().zip(["z", "a", "b"]) {
        assert!(diagnostic.message.starts_with(&format!("tag '{name}'")));
        assert_eq!(
            sources.slice(diagnostic.span),
            Some("include::part.adoc[tags=z;a;b]")
        );
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[tags=a;b]");
    resources
        .insert_binary(
            "/docs/part.adoc",
            b"\xff\n// tag::a[]\n// tag::b[]\ngood\n// end::a[]\n// end::b[]\n",
        )
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(result.diagnostics[0].report.message.as_deref(), Some("mismatched end tag (expected 'b' but found 'a') at line 5 of include file: /docs/part.adoc"));
    assert_eq!(
        sources
            .get(SourceId(1))
            .unwrap()
            .position(result.diagnostics[0].span.start)
            .unwrap()
            .line,
        5
    );
}

#[test]
fn finite_line_selection_preserves_positions_in_an_explicitly_incomplete_prefix() {
    for (encoding, glyph, bad) in [
        ("Shift_JIS", &b"\x95\x5c"[..], &b"\x81"[..]),
        ("EUC-JP", &b"\xc9\xbd"[..], &b"\x8f"[..]),
        ("UTF-8", "表".as_bytes(), &b"\xff"[..]),
    ] {
        let root = format!(":attribute-missing: warn\n:codec: {encoding}\n\ninclude::part.adoc[encoding={{codec}},lines=2,indent=0]\n\ninclude::part.adoc[encoding={{codec}},lines=2,indent=0]");
        let (mut resources, mut sources, options) = setup(&root);
        let prefix = "ignored\r\n  表 {missing}\r\n";
        resources
            .insert_binary(
                "/docs/part.adoc",
                [
                    b"ignored\r\n  ".as_slice(),
                    glyph,
                    b" {missing}\r\nlast\r\n",
                    bad,
                ]
                .concat(),
            )
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
        let file = sources.get(SourceId(1)).unwrap();
        assert!(!file.is_complete());
        assert_eq!(file.text(), prefix);
        assert!(sources.get(SourceId(2)).is_none());
        for diagnostic in &result.diagnostics {
            assert_eq!(diagnostic.span.source_id, SourceId(1));
            assert_eq!(sources.slice(diagnostic.span), Some("{missing}"));
            assert_eq!(diagnostic.span.start, prefix.find("{missing}").unwrap());
            let position = file.position(diagnostic.span.start).unwrap();
            assert_eq!((position.line, position.column), (2, 5));
        }
        for block in result.document.flat_blocks() {
            assert_eq!(block.span.source_id, SourceId(1));
            let BlockKind::Paragraph { inlines } = &block.kind else {
                panic!("{block:?}")
            };
            assert!(matches!(&inlines[0], Inline::Text { value, .. } if value.starts_with('表')));
        }
        assert!(result
            .tokens
            .iter()
            .all(|token| sources.slice(token.span).is_some()));
        assert!(result
            .tokens
            .iter()
            .any(|token| sources.slice(token.span) == Some("{missing}")));
    }
}

#[test]
fn complete_and_prefix_snapshots_keep_distinct_immutable_identity() {
    let (mut resources, mut sources, options) =
        setup("include::part.adoc[encoding=Shift_JIS,lines=1]");
    resources
        .insert_binary("/docs/part.adoc", b"\x95\x5c\r\n\x81")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert!(!sources.get(SourceId(1)).unwrap().is_complete());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "表\r\n");
    // A full read still fails; the earlier prefix is never used as a cache hit.
    assert_eq!(
        sources
            .load_with_encoding(
                "part.adoc",
                Some(options.source_id),
                &resources,
                Some("Shift_JIS")
            )
            .unwrap_err()
            .kind,
        ResourceErrorKind::InvalidEncoding
    );
    assert!(sources.get(SourceId(2)).is_none());
    resources
        .insert_binary("/docs/part.adoc", b"\x95\x5c\r\n")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.document.flat_blocks()[0].span.source_id, SourceId(2));
    assert!(sources.get(SourceId(2)).unwrap().is_complete());
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "表\r\n");
    assert!(!sources.get(SourceId(1)).unwrap().is_complete());
    let again = sources
        .load_with_encoding(
            "part.adoc",
            Some(options.source_id),
            &resources,
            Some("Shift_JIS"),
        )
        .unwrap();
    assert_eq!(again, SourceId(2));
}

#[test]
fn nested_prefix_includes_keep_relative_paths_and_source_provenance() {
    let (mut resources, mut sources, options) =
        setup("include::sub/outer.adoc[encoding=Shift_JIS,lines=1..3]");
    resources
        .insert_binary(
            "/docs/sub/outer.adoc",
            b"\x95\x5c\r\n\r\ninclude::inner.adoc[encoding=EUC-JP,lines=1]\r\n\x81",
        )
        .unwrap();
    resources
        .insert_binary("/docs/sub/inner.adoc", b"*\xc6\xfc\xcb\xdc*\r\n\x8f")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    for (id, name, text) in [
        (
            SourceId(1),
            "/docs/sub/outer.adoc",
            "表\r\n\r\ninclude::inner.adoc[encoding=EUC-JP,lines=1]\r\n",
        ),
        (SourceId(2), "/docs/sub/inner.adoc", "*日本*\r\n"),
    ] {
        let file = sources.get(id).unwrap();
        assert_eq!(file.name(), name);
        assert_eq!(file.text(), text);
        assert!(!file.is_complete());
    }
    let blocks = result.document.flat_blocks();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].span.source_id, SourceId(1));
    assert_eq!(blocks[1].span.source_id, SourceId(2));
    assert_eq!(sources.slice(blocks[1].span).unwrap().trim(), "*日本*");
    assert!(result
        .tokens
        .iter()
        .all(|token| sources.slice(token.span).is_some()));
}

#[test]
fn finite_reads_do_not_hide_errors_in_consumed_lines_or_unbounded_selections() {
    for selection in [
        "lines=2",
        "lines=3",
        "lines=1;3",
        "lines=0;1",
        "lines=1..",
        "lines=2..1",
        "lines=",
        "tag=body",
        "tags=**",
    ] {
        let directive = format!("include::part.adoc[encoding=Shift_JIS,{selection},opts=optional]");
        let (mut resources, mut sources, options) = setup(&directive);
        resources
            .insert_binary("/docs/part.adoc", b"good\n\x81\nlast\n")
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 1, "{selection}");
        let diagnostic = &result.diagnostics[0];
        assert_eq!(
            diagnostic.report.message.as_deref(),
            Some("include file not readable: /docs/part.adoc")
        );
        assert_eq!(sources.slice(diagnostic.span), Some(directive.as_str()));
        assert!(sources.get(SourceId(1)).is_none());
    }
}

#[test]
fn encoded_include_selection_keeps_decoded_utf8_spans_and_physical_lines() {
    for selector in ["tag=body", "lines=3"] {
        for (encoding, glyph) in [
            ("UTF-16LE", &b""[..]),
            ("CP1251", &b"\xc6"[..]),
            ("Shift_JIS", &b"\x84\x47"[..]),
            ("Windows-31J", &b"\x84\x47"[..]),
            ("EUC-JP", &b"\xa7\xa8"[..]),
            ("eucJP-ms", &b"\xa7\xa8"[..]),
            ("CP51932", &b"\xa7\xa8"[..]),
        ] {
            let root = format!(":attribute-missing: warn\n:codec: {encoding}\n\ninclude::part.adoc[encoding={{codec}},{selector},indent=0]");
            let text = "ignored\r\n// tag::body[]\r\n  Ж {missing}\r\n// end::body[]\r\n";
            let bytes: Vec<u8> = if encoding == "UTF-16LE" {
                text.encode_utf16().flat_map(u16::to_le_bytes).collect()
            } else {
                text.chars()
                    .flat_map(|ch| {
                        if ch == 'Ж' {
                            glyph.to_vec()
                        } else {
                            vec![ch as u8]
                        }
                    })
                    .collect()
            };
            let (mut resources, mut sources, options) = setup(&root);
            resources.insert_binary("/docs/part.adoc", bytes).unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
            let diagnostic = &result.diagnostics[0];
            assert_eq!(sources.get(diagnostic.span.source_id).unwrap().text(), text);
            assert_eq!(sources.slice(diagnostic.span), Some("{missing}"));
            assert_eq!(diagnostic.span.start, text.find("{missing}").unwrap());
            let position = sources
                .get(diagnostic.span.source_id)
                .unwrap()
                .position(diagnostic.span.start)
                .unwrap();
            assert_eq!((position.line, position.column), (3, 5));
            let block = result.document.flat_blocks()[0];
            let BlockKind::Paragraph { inlines } = &block.kind else {
                panic!("{block:?}")
            };
            assert!(
                matches!(inlines.first(), Some(Inline::Text { value, .. }) if value.starts_with('Ж'))
            );
            assert!(result
                .tokens
                .iter()
                .any(|token| sources.slice(token.span) == Some("{missing}")));
            assert!(result
                .tokens
                .iter()
                .all(|token| sources.slice(token.span).is_some()));
        }
    }
}

#[test]
fn repeated_include_decodings_have_independent_immutable_snapshots() {
    let (mut resources, mut sources, options) = setup(
        "include::part.adoc[encoding=UTF-8]\n\ninclude::part.adoc[encoding=ISO-8859-1]\n\ninclude::part.adoc[encoding=CP65001]",
    );
    resources
        .insert_binary("/docs/part.adoc", b"\xc3\xa9")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "é");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "Ã©");
    assert!(sources.get(SourceId(3)).is_none());
    let ids: Vec<_> = result
        .document
        .flat_blocks()
        .iter()
        .map(|b| b.span.source_id)
        .collect();
    assert_eq!(ids, [SourceId(1), SourceId(2), SourceId(1)]);
    let error = sources
        .load_with_encoding(
            "part.adoc",
            Some(options.source_id),
            &resources,
            Some("US-ASCII"),
        )
        .unwrap_err();
    assert_eq!(error.kind, ResourceErrorKind::InvalidEncoding);
    assert!(sources.get(SourceId(3)).is_none());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "é");
}

#[test]
fn japanese_variant_snapshots_keep_their_own_text_and_aliases_reuse_sources() {
    let (mut resources, mut sources, options) = setup(
        "include::part.adoc[encoding=Shift_JIS]\n\ninclude::part.adoc[encoding=SJIS]\n\ninclude::part.adoc[encoding=CP932]",
    );
    resources
        .insert_binary("/docs/part.adoc", b"\x81\x60\r\n")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "〜\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "～\r\n");
    assert!(sources.get(SourceId(3)).is_none());
    let blocks = result.document.flat_blocks();
    assert_eq!(
        blocks.iter().map(|b| b.span.source_id).collect::<Vec<_>>(),
        [SourceId(1), SourceId(2), SourceId(2)]
    );
    for (block, expected) in blocks.iter().zip(["〜", "～", "～"]) {
        assert_eq!(sources.slice(block.span).unwrap().trim(), expected);
    }
}

#[test]
fn invalid_japanese_includes_report_the_directive_without_registering_a_source() {
    for (encoding, bytes) in [
        ("Shift_JIS", &b"\x81"[..]),
        ("Windows-31J", &b"\xfc\xfc"[..]),
        ("EUC-JP", &b"\x8f\xa2"[..]),
        ("eucJP-ms", &b"\x8e\xe0"[..]),
        ("CP51932", &b"\x8f\xa2\xaf"[..]),
    ] {
        let directive = format!("include::part.adoc[encoding={encoding},opts=optional]");
        let (mut resources, mut sources, options) =
            setup(&format!("before\n\n{directive}\n\nafter"));
        resources.insert_binary("/docs/part.adoc", bytes).unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.diagnostics.len(), 1, "{encoding}");
        let diagnostic = &result.diagnostics[0];
        assert_eq!(
            diagnostic.report.message.as_deref(),
            Some("include file not readable: /docs/part.adoc")
        );
        assert_eq!(sources.slice(diagnostic.span), Some(directive.as_str()));
        assert!(sources.get(SourceId(1)).is_none());
        assert_eq!(result.document.flat_blocks().len(), 3);
    }
}

#[test]
fn invalid_encoded_include_reports_unreadable_and_optional_only_suppresses_missing() {
    let (mut resources, mut sources, options) = setup(
        "include::missing.adoc[opts=optional,encoding=UTF-16]\ninclude::broken.adoc[opts=optional,encoding=UTF-16LE]\n\ninclude::good.adoc[encoding=iso-1000-1]",
    );
    resources
        .insert_binary("/docs/broken.adoc", b"\x00\xd8")
        .unwrap();
    resources.insert("/docs/good.adoc", "Ж good").unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(
        diagnostic.report.message.as_deref(),
        Some("include file not readable: /docs/broken.adoc")
    );
    assert_eq!(
        sources.slice(diagnostic.span),
        Some("include::broken.adoc[opts=optional,encoding=UTF-16LE]")
    );
    assert_eq!(sources.get(SourceId(1)).unwrap().name(), "/docs/good.adoc");
    assert_eq!(result.document.flat_blocks().len(), 2);
}

#[test]
fn text_only_providers_remain_compatible_and_hosts_can_supply_other_decoders() {
    struct TextOnly;
    impl ResourceProvider for TextOnly {
        fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
            assert_eq!(request.target, "part.adoc");
            assert_eq!(request.referrer, Some("/main.adoc"));
            Ok(Resource {
                name: "/part.adoc".into(),
                text: "Ж".into(),
            })
        }
    }
    for encoding in ["UTF-8", "iso-1000-1"] {
        let mut sources = SourceMap::new();
        let id = sources.add(
            "/main.adoc",
            format!("include::part.adoc[encoding={encoding},lines=1]"),
        );
        let result = parse_resource(
            &mut sources,
            &ParseOptions {
                source_id: id,
                ..Default::default()
            },
            &TextOnly,
        )
        .unwrap();
        assert!(result.diagnostics.is_empty());
        assert_eq!(sources.get(SourceId(1)).unwrap().text(), "Ж");
    }
    struct HostDecoder;
    impl ResourceProvider for HostDecoder {
        fn load(&self, _: ResourceRequest<'_>) -> ResourceResult<Resource> {
            panic!("use the encoding-aware entry point")
        }
        fn load_with_encoding(
            &self,
            request: ResourceRequest<'_>,
            encoding: Option<&str>,
        ) -> ResourceResult<Resource> {
            assert_eq!(encoding, Some("Shift_JIS"));
            assert_eq!(request.referrer, Some("/main.adoc"));
            Ok(Resource {
                name: "/part.adoc".into(),
                text: "日本".into(),
            })
        }
    }
    let mut sources = SourceMap::new();
    let id = sources.add("/main.adoc", "include::part.adoc[encoding=Shift_JIS]");
    let result = parse_resource(
        &mut sources,
        &ParseOptions {
            source_id: id,
            ..Default::default()
        },
        &HostDecoder,
    )
    .unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "日本");
}

#[test]
fn nested_includes_select_their_own_encoding_and_keep_relative_paths() {
    let (mut resources, mut sources, options) = setup("include::sub/outer.adoc[encoding=UTF-16LE]");
    let outer = "= Ж\r\n\r\ninclude::inner.adoc[encoding=CP1251]\r\n";
    resources
        .insert_binary(
            "/docs/sub/outer.adoc",
            outer
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    resources
        .insert_binary("/docs/sub/inner.adoc", b"\xcf\xf0\xe8\xe2\xe5\xf2\r\n")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), outer);
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "Привет\r\n");
    assert_eq!(
        sources.get(SourceId(2)).unwrap().name(),
        "/docs/sub/inner.adoc"
    );
}
