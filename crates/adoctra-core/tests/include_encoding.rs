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
fn encoded_include_selection_keeps_decoded_utf8_spans_and_physical_lines() {
    for selector in ["tag=body", "lines=3"] {
        for encoding in ["UTF-16LE", "CP1251"] {
            let root = format!(":attribute-missing: warn\n:codec: {encoding}\n\ninclude::part.adoc[encoding={{codec}},{selector},indent=0]");
            let text = "ignored\r\n// tag::body[]\r\n  Ж {missing}\r\n// end::body[]\r\n";
            let bytes: Vec<u8> = if encoding == "UTF-16LE" {
                text.encode_utf16().flat_map(u16::to_le_bytes).collect()
            } else {
                text.chars()
                    .map(|ch| if ch == 'Ж' { 0xc6 } else { ch as u8 })
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
            format!("include::part.adoc[encoding={encoding}]"),
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
