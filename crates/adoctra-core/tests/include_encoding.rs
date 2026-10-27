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
