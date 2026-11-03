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

fn bom_bytes(text: &str, little: bool) -> Vec<u8> {
    let mut bytes = if little {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in text.encode_utf16() {
        bytes.extend(if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

#[test]
fn implicit_bom_tag_filters_run_before_conversion_and_keep_raw_diagnostic_spans() {
    for (selector, visible, missing) in [
        ("tag=body", false, true),
        ("tags=*", false, false),
        ("tag=!body", true, false),
        ("tags=**;!body", true, false),
        ("tags=**", true, false),
    ] {
        let (mut resources, mut sources, options) =
            setup(&format!("include::part.adoc[{selector}]"));
        resources
            .insert_binary(
                "/docs/part.adoc",
                bom_bytes(
                    "// tag::body[]\n*shown*\n// end::body[]\n*outside*\n",
                    false,
                ),
            )
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(
            !result.document.flat_blocks().is_empty(),
            visible,
            "{selector}"
        );
        assert_eq!(result.diagnostics.len(), usize::from(missing), "{selector}");
        if visible {
            let file = sources.get(SourceId(1)).unwrap();
            assert!(file.is_complete());
            assert_eq!(
                result.document.flat_blocks()[0].span.start,
                file.text().find("*shown*").unwrap()
            );
            assert!(file.text().contains("*outside*"));
        } else if missing {
            assert!(result.diagnostics[0]
                .message
                .contains("tag 'body' not found"));
        }
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[tag=body]");
    let mut bytes = b"// tag::body[]\n".to_vec();
    bytes.extend(bom_bytes("*shown*\n*second*\n", false));
    bytes.extend(b"// end::body[]\n// end::body[]\n// tag::body[]\n");
    resources.insert_binary("/docs/part.adoc", bytes).unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let file = sources.get(SourceId(1)).unwrap();
    assert_eq!(
        file.text(),
        "// tag::body[]\n\u{feff}*shown*\n*second*\n// end::body[]\n// end::body[]\n// tag::body[]\n"
    );
    assert!(file.omitted_lines().is_empty());
    assert_eq!(result.diagnostics.len(), 2);
    for diagnostic in &result.diagnostics {
        assert_eq!(diagnostic.span.source_id, SourceId(1));
        let text = sources.slice(diagnostic.span).unwrap();
        let position = file.position(diagnostic.span.start).unwrap();
        assert!(matches!(position.line, 5 | 6), "{position:?}/{text:?}");
    }
    assert_eq!(result.document.flat_blocks()[0].span.start, 18);
    assert!(parse_resource(&mut sources, &options, &resources)
        .unwrap()
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.span.source_id == SourceId(1)));
    assert!(sources.get(SourceId(2)).is_none());
    let (mut resources, mut sources, options) =
        setup("include::part.adoc[encoding=BINARY,tag=body]");
    let mut raw = b"\xef\xbb\xbf\xff// tag::body[]\n".to_vec();
    raw.extend(bom_bytes("*shown*\n", false));
    raw.extend(b"// end::other[]\n");
    resources.insert_binary("/docs/part.adoc", raw).unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.span.source_id, SourceId(1));
    assert_eq!(diagnostic.span.start, 0);
    let omitted = diagnostic.omitted_span.unwrap();
    assert_eq!(omitted.line, 1);
    assert_eq!(
        sources.get(SourceId(1)).unwrap().omitted_slice(omitted),
        Some(b"tag::body[]".as_slice())
    );
}

#[test]
fn first_kept_bom_and_embedded_lf_frames_preserve_unicode_positions_and_budget() {
    for (little, glyph) in [(false, '上'), (true, '\u{a41}')] {
        let (mut resources, mut sources, mut options) = setup("include::part.adoc[lines=1;2]");
        resources
            .insert_binary(
                "/docs/part.adoc",
                bom_bytes(&format!("first{glyph}second"), little),
            )
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty());
        let file = sources.get(SourceId(1)).unwrap();
        assert_eq!(file.text(), format!("\u{feff}first{glyph}second"));
        assert!(!file.text().contains('\n'));
        assert!(!file.is_complete());
        let boundary = "\u{feff}first".len() + glyph.len_utf8();
        assert_eq!(
            file.position(boundary).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 1 }
        );
        assert_eq!(
            file.position(file.text().len()).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 7 }
        );
        assert_eq!(
            sources.slice(result.document.flat_blocks()[0].span),
            Some(format!("first{glyph}second").as_str())
        );
        options.includes.max_bytes = file.text().len();
        assert!(parse_resource(&mut sources, &options, &resources)
            .unwrap()
            .diagnostics
            .is_empty());
        options.includes.max_bytes -= 1;
        assert!(parse_resource(&mut sources, &options, &resources)
            .unwrap()
            .diagnostics
            .iter()
            .any(|d| d.message.contains("maximum included byte count")));
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[lines=2]");
    let mut bytes = bom_bytes("ignored\n", false);
    bytes.extend(bom_bytes("*shown*", true));
    resources
        .insert_binary("/docs/part.adoc", bytes.clone())
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let file = sources.get(SourceId(1)).unwrap();
    assert_eq!(file.text(), "\u{feff}*shown*");
    assert_eq!(file.omitted_lines().len(), 1);
    assert_eq!(
        file.position(3).unwrap(),
        adoctra_core::SourcePosition { line: 2, column: 2 }
    );
    let old = file.text().to_owned();
    resources
        .insert_binary("/docs/part.adoc", b"ignored\n\xff\xfeA\0\n")
        .unwrap();
    assert_eq!(
        parse_resource(&mut sources, &options, &resources)
            .unwrap()
            .diagnostics
            .len(),
        1
    );
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), old);
    assert!(sources.get(SourceId(2)).is_none());
}

#[test]
fn skipped_implicit_utf16_bom_keeps_raw_selected_bytes_and_distinct_snapshots() {
    for (little, explicit) in [(true, "UTF-16LE"), (false, "UTF-16BE")] {
        for name in ["UTF-8", "ASCII-8BIT", "BINARY", "unknown-bom-audit"] {
            let (mut resources,mut sources,options)=setup(&format!("include::part.adoc[encoding={name},lines=2]\n\ninclude::part.adoc[encoding={explicit},lines=2]"));
            let mut bytes = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in "*one*\n*two*\n*three*\n".encode_utf16() {
                bytes.extend(if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            resources
                .insert_binary("/docs/part.adoc", bytes.clone())
                .unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            assert!(result.diagnostics.is_empty(), "{name}/{explicit}");
            let raw = sources.get(SourceId(1)).unwrap();
            assert_eq!(raw.text(), "\0*\0t\0w\0o\0*\0\n");
            assert!(!raw.is_complete());
            assert_eq!(raw.omitted_lines().len(), 1);
            assert_eq!(raw.omitted_lines()[0].line, 1);
            assert_eq!(raw.omitted_lines()[0].offset, 0);
            assert_eq!(
                raw.omitted_lines()[0].bytes,
                bytes[..if little { 13 } else { 14 }]
            );
            assert_eq!(
                raw.position(0).unwrap(),
                adoctra_core::SourcePosition { line: 2, column: 1 }
            );
            assert_eq!(
                raw.position(4).unwrap(),
                adoctra_core::SourcePosition { line: 2, column: 5 }
            );
            let blocks = result.document.flat_blocks();
            assert_eq!(blocks.len(), 2);
            assert_eq!(blocks[0].span.source_id, SourceId(1));
            assert_eq!(sources.slice(blocks[0].span), Some("\0*\0t\0w\0o\0*"));
            assert_eq!(blocks[1].span.source_id, SourceId(2));
            assert_eq!(sources.slice(blocks[1].span), Some("*two*"));
            assert_eq!(
                sources.get(SourceId(2)).unwrap().text(),
                "\u{feff}*one*\n*two*\n*three*\n"
            );
            assert!(sources.get(SourceId(2)).unwrap().is_complete());
            let again = parse_resource(&mut sources, &options, &resources).unwrap();
            assert!(again.diagnostics.is_empty());
            assert!(sources.get(SourceId(3)).is_none());
            resources
                .insert_binary("/docs/part.adoc", bytes[2..].to_vec())
                .unwrap();
            let updated = parse_resource(&mut sources, &options, &resources).unwrap();
            assert!(updated.diagnostics.is_empty());
            assert_eq!(
                updated.document.flat_blocks()[0].span.source_id,
                SourceId(3)
            );
            assert_eq!(
                sources.get(SourceId(1)).unwrap().text(),
                "\0*\0t\0w\0o\0*\0\n"
            );
        }
        let (mut resources, mut sources, mut options) = setup("include::part.adoc[lines=2]");
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in "*one*\n*two*\n*three*\n".encode_utf16() {
            bytes.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        resources.insert_binary("/docs/part.adoc", bytes).unwrap();
        let budget = 12 + if little { 13 } else { 14 };
        options.includes.max_bytes = budget;
        assert!(parse_resource(&mut sources, &options, &resources)
            .unwrap()
            .diagnostics
            .is_empty());
        options.includes.max_bytes = budget - 1;
        assert!(parse_resource(&mut sources, &options, &resources)
            .unwrap()
            .diagnostics
            .iter()
            .any(|d| d.message.contains("maximum included byte count")));
    }
}

#[test]
fn korean_prefix_selection_and_variant_errors_preserve_snapshots() {
    for encoding in ["EUC-KR", "eucKR", "CP949"] {
        for selection in ["lines=1", "lines=2", "lines=1;3", "tag=body"] {
            let (mut resources, mut sources, options) = setup(&format!(
                "include::part.adoc[encoding={encoding},{selection},opts=optional]"
            ));
            resources
                .insert_binary("/docs/part.adoc", b"*\xb0\xa1*\r\n\x81\nlast\n")
                .unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            if selection == "lines=1" {
                assert!(result.diagnostics.is_empty());
                let source = sources.get(SourceId(1)).unwrap();
                assert_eq!(source.text(), "*가*\r\n");
                assert!(!source.is_complete());
                assert_eq!(
                    sources.slice(result.document.flat_blocks()[0].span),
                    Some("*가*")
                );
            } else {
                assert_eq!(result.diagnostics.len(), 1);
                assert!(sources.get(SourceId(1)).is_none());
            }
        }
    }
    let (mut resources, mut sources, options) = setup(
        "include::part.adoc[encoding=CP949]\n\ninclude::part.adoc[encoding=EUC-KR,opts=optional]",
    );
    resources
        .insert_binary("/docs/part.adoc", b"\x81\x41")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "갂");
    assert!(sources.get(SourceId(2)).is_none());
}

#[test]
fn gb_selections_preserve_decoded_spans_and_unicode_columns() {
    for (encoding, glyph, decoded) in [
        ("GB2312", &b"\xd6\xd0"[..], "中"),
        ("ISO-2022-JP", &b"\x1b$BF|\x1b(B"[..], "日"),
        ("UTF8-KDDI", "\u{e4cc}".as_bytes(), "🇯🇵"),
        ("utf8-kddi", "\u{efa5}".as_bytes(), "🇯🇵"),
        ("UTF8-KDDI", "\u{e522}".as_bytes(), "1\u{20e3}"),
        ("UTF8-KDDI", "\u{effb}".as_bytes(), "1\u{20e3}"),
        ("UTF8-KDDI", "\u{e4a6}".as_bytes(), "🅿"),
        ("UTF8-KDDI", "\u{ef7e}".as_bytes(), "🅿"),
        ("UTF8-MAC", "e\u{301}".as_bytes(), "é"),
        (
            "UTF-8-HFS",
            "\u{3b1}\u{313}\u{300}\u{345}".as_bytes(),
            "\u{1f82}",
        ),
        ("utf-8-mac", "\u{1100}\u{1161}\u{11a8}".as_bytes(), "각"),
        ("UTF8-MAC", "a\u{315}\u{300}".as_bytes(), "a\u{315}\u{300}"),
        ("CESU-8", &b"\xed\xa0\xbd\xed\xb8\x80"[..], "😀"),
        ("cesu-8", &b"\xed\xaf\xbf\xed\xbf\xbf"[..], "\u{10ffff}"),
        (
            "CESU-8",
            &b"\xed\xa0\xbc\xed\xb7\xaf\xed\xa0\xbc\xed\xb7\xb5"[..],
            "🇯🇵",
        ),
        ("UTF8-SoftBank", "\u{e50b}".as_bytes(), "🇯🇵"),
        ("utf8-softbank", "\u{e21c}".as_bytes(), "1\u{20e3}"),
        ("UTF8-SoftBank", "\u{e14f}".as_bytes(), "🅿"),
        ("UTF8-DoCoMo", "\u{e6e2}".as_bytes(), "1\u{20e3}"),
        ("utf8-docomo", "\u{e66c}".as_bytes(), "🅿"),
        ("SJIS-SoftBank", &b"\xf6\xe4"[..], "🇯🇵"),
        ("SJIS-SoftBank", &b"\xfb\xab"[..], "🇯🇵"),
        ("SJIS-SoftBank", &b"\xf2\xe5"[..], "1\u{20e3}"),
        ("SJIS-SoftBank", &b"\xf1\xd4"[..], "🅿"),
        ("SJIS-DoCoMo", &b"\xf9\x87"[..], "1\u{20e3}"),
        ("SJIS-DoCoMo", &b"\xf9\x85"[..], "#\u{20e3}"),
        ("SJIS-DoCoMo", &b"\xf8\xcd"[..], "🅿"),
        ("SJIS-KDDI", &b"\xf6\xa5"[..], "🇯🇵"),
        ("SJIS-KDDI", &b"\xf6\xfb"[..], "1\u{20e3}"),
        ("SJIS-KDDI", &b"\xf6\x7e"[..], "🅿"),
        ("EUC-JIS-2004", &b"\x8f\xa1\xa1"[..], "𠂉"),
        ("EUC-JISX0213", &b"\x8f\xfe\xf6"[..], "𪚲"),
        ("ISO-2022-JP-KDDI", &b"\x1b$Bv'\x1b(B"[..], "🇯🇵"),
        ("stateless-ISO-2022-JP-KDDI", &b"\x92\xf6\xa7"[..], "🇯🇵"),
        (
            "stateless-ISO-2022-JP-KDDI",
            &b"\x92\xf6\xfd"[..],
            "1\u{20e3}",
        ),
        ("stateless-ISO-2022-JP-KDDI", &b"\x92\x93\xa1"[..], "ⅰ"),
        ("ISO2022-JP", &b"\x1b$@F|\x1b(J"[..], "日"),
        ("stateless-ISO-2022-JP", &b"\x92\xc6\xfc"[..], "日"),
        ("STATELESS-ISO-2022-JP", &b"\x90\xc6\xfc"[..], "日"),
        ("CP50220", &b"\x1b(I1\x0f"[..], "ｱ"),
        ("cp50221", &b"\x0e1\x1b(B"[..], "ｱ"),
        ("EUC-KR", &b"\xb0\xa1"[..], "가"),
        ("eucKR", &b"\xb0\xa1"[..], "가"),
        ("CP949", &b"\x81\x41"[..], "갂"),
        ("EUC-CN", &b"\xd6\xd0"[..], "中"),
        ("GB12345", &b"\xb9\xfa"[..], "國"),
        ("GBK", &b"\x80"[..], "€"),
        ("CP936", &b"\x80"[..], "€"),
        ("GB18030", &b"\x90\x30\x81\x30"[..], "\u{10000}"),
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
                adoctra_core::SourcePosition {
                    line: 3,
                    column: 4 + decoded.chars().count()
                }
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
fn iso2022_prefix_reads_validate_states_and_keep_immutable_alias_snapshots() {
    for name in [
        "ISO-2022-JP",
        "ISO2022-JP",
        "iSo-2022-jP",
        "CP50220",
        "CP50221",
    ] {
        for (selection, bytes, success) in [
            ("lines=1", &b"*\x1b$B$\"\x1b(B*\r\n\x1b$"[..], true),
            ("lines=2", &b"*\x1b$B$\"\x1b(B*\r\n\x1b$"[..], false),
            ("lines=1;3", &b"good\n\x1b$\nlast\n"[..], false),
            ("lines=1", &b"\x1b$B$\"\r\n"[..], false),
            ("lines=1", &b"\x1b$B$\n"[..], false),
            (
                "tag=body",
                &b"// tag::body[]\nOK\n// end::body[]\n\x1b"[..],
                false,
            ),
        ] {
            let (mut resources, mut sources, options) = setup(&format!(
                "include::part.adoc[encoding={name},{selection},opts=optional]"
            ));
            resources.insert_binary("/docs/part.adoc", bytes).unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            if success {
                assert!(result.diagnostics.is_empty());
                let source = sources.get(SourceId(1)).unwrap();
                assert_eq!(source.text(), "*あ*\r\n");
                assert!(!source.is_complete());
                assert_eq!(
                    sources.slice(result.document.flat_blocks()[0].span),
                    Some("*あ*")
                );
            } else {
                assert_eq!(result.diagnostics.len(), 1, "{name}/{selection}: {bytes:?}");
                assert!(sources.get(SourceId(1)).is_none());
            }
        }
    }
    let (mut resources, mut sources, options) = setup(
        "include::part.adoc[encoding=ISO-2022-JP]\n\ninclude::part.adoc[encoding=ISO2022-JP]",
    );
    resources
        .insert_binary("/docs/part.adoc", b"\x1b$B$\"\x1b(B\r\n")
        .unwrap();
    let first = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(first.diagnostics.is_empty());
    assert!(first
        .document
        .flat_blocks()
        .iter()
        .all(|block| block.span.source_id == SourceId(1)));
    // A different external designation producing the same text reuses the snapshot.
    resources
        .insert_binary("/docs/part.adoc", b"\x1b$@$\"\x1b(J\r\n")
        .unwrap();
    let same = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(same.diagnostics.is_empty());
    assert!(sources.get(SourceId(2)).is_none());
    resources
        .insert_binary("/docs/part.adoc", b"\x1b$BF|\x1b(B\r\n")
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(changed.diagnostics.is_empty());
    assert!(changed
        .document
        .flat_blocks()
        .iter()
        .all(|block| block.span.source_id == SourceId(2)));
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "あ\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "日\r\n");
}

#[test]
fn cp5022_states_and_variants_preserve_selection_and_source_identity() {
    for name in ["CP50220", "CP50221"] {
        for (selection, bytes, success) in [
            ("lines=1", &b"*\x0e1\x0f*\r\n\x1b$"[..], true),
            ("lines=2", &b"*\x0e1\x0f*\r\n\x1b$"[..], false),
            ("lines=1", &b"\x0e1\r\n\x0f"[..], false),
            ("lines=1;3", &b"good\n\x0e \x0f\nlast\n"[..], false),
            (
                "tag=body",
                &b"// tag::body[]\nOK\n// end::body[]\n\x0e "[..],
                false,
            ),
        ] {
            let (mut resources, mut sources, options) = setup(&format!(
                "include::part.adoc[encoding={name},{selection},opts=optional]"
            ));
            resources.insert_binary("/docs/part.adoc", bytes).unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            if success {
                assert!(result.diagnostics.is_empty());
                let source = sources.get(SourceId(1)).unwrap();
                assert_eq!(source.text(), "*ｱ*\r\n");
                assert!(!source.is_complete());
                assert_eq!(
                    sources.slice(result.document.flat_blocks()[0].span),
                    Some("*ｱ*")
                );
            } else {
                assert_eq!(result.diagnostics.len(), 1, "{name}/{selection}");
                assert!(sources.get(SourceId(1)).is_none());
            }
        }
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[encoding=CP50220]\n\ninclude::part.adoc[encoding=cp50221]\n\ninclude::part.adoc[encoding=ISO-2022-JP,opts=optional]");
    resources
        .insert_binary("/docs/part.adoc", b"\x1b$B!A\x1b(B\r\n")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(
        result
            .document
            .flat_blocks()
            .iter()
            .map(|block| block.span.source_id.0)
            .collect::<Vec<_>>(),
        [1, 1, 2]
    );
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "～\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "〜\r\n");
    resources
        .insert_binary("/docs/part.adoc", b"\x0e1\x0f\r\n")
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(changed.diagnostics.len(), 1);
    assert_eq!(sources.get(SourceId(3)).unwrap().text(), "ｱ\r\n");
    resources
        .insert_binary("/docs/part.adoc", b"\x1b(I1\x1b(B\r\n")
        .unwrap();
    let same = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(same.diagnostics.len(), 1);
    assert!(sources.get(SourceId(4)).is_none());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "～\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "〜\r\n");
}

#[test]
fn stateless_jp_prefixes_and_immutable_snapshots_follow_decoded_text() {
    let name = "stateless-ISO-2022-JP";
    for (selection, bytes, success) in [
        ("lines=1", &b"*\x92\xa4\xa2*\r\n\x90\xa4"[..], true),
        ("lines=1", &b"*\x92\xa4\xa2*\r\n\x1b"[..], true),
        ("lines=2", &b"*\x92\xa4\xa2*\r\n\x90\xa4"[..], false),
        ("lines=1;3", &b"good\n\x90\nlast\n"[..], false),
        ("lines=1;3", &b"good\n\x1b\nlast\n"[..], false),
        ("lines=2", &b"good\n\x0f\n"[..], false),
        ("lines=1", &b"\x92\xa4\nlast\n"[..], false),
        (
            "tag=body",
            &b"// tag::body[]\nOK\n// end::body[]\n\x92"[..],
            false,
        ),
        (
            "tag=body",
            &b"// tag::body[]\nOK\n// end::body[]\n\x0e"[..],
            false,
        ),
    ] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding={name},{selection},opts=optional]"
        ));
        resources.insert_binary("/docs/part.adoc", bytes).unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        if success {
            assert!(result.diagnostics.is_empty());
            let source = sources.get(SourceId(1)).unwrap();
            assert_eq!(source.text(), "*あ*\r\n");
            assert!(!source.is_complete());
            assert_eq!(
                sources.slice(result.document.flat_blocks()[0].span),
                Some("*あ*")
            );
        } else {
            assert_eq!(result.diagnostics.len(), 1, "{selection}");
            assert!(sources.get(SourceId(1)).is_none());
        }
    }
    let (mut resources, mut sources, options) = setup(&format!(
        "include::part.adoc[encoding={name}]\n\ninclude::part.adoc[encoding={}]",
        name.to_uppercase()
    ));
    resources
        .insert_binary("/docs/part.adoc", b"\x92\xa4\xa2\r\n")
        .unwrap();
    let first = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(first.diagnostics.is_empty());
    assert!(first
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.span.source_id == SourceId(1)));
    resources
        .insert_binary("/docs/part.adoc", b"\x90\xa4\xa2\r\n")
        .unwrap();
    let same = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(same.diagnostics.is_empty());
    assert!(sources.get(SourceId(2)).is_none());
    resources
        .insert_binary("/docs/part.adoc", b"\x90\xa4\xa4\r\n")
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(changed.diagnostics.is_empty());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "あ\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "い\r\n");
    assert!(changed
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.span.source_id == SourceId(2)));
}

#[test]
fn kddi_compounds_preserve_prefix_limits_budgets_and_variant_source_identity() {
    for (name, good, bad) in [
        (
            "ISO-2022-JP-KDDI",
            &b"*\x1b$Bv'\x1b(B*\r\n"[..],
            &b"\x1b$"[..],
        ),
        (
            "stateless-ISO-2022-JP-KDDI",
            &b"*\x92\xf6\xa7*\r\n"[..],
            &b"\x92\xf6"[..],
        ),
    ] {
        for selection in ["lines=1", "lines=2", "lines=1;3", "tag=body"] {
            let (mut resources, mut sources, options) = setup(&format!(
                "include::part.adoc[encoding={name},{selection},opts=optional]"
            ));
            resources
                .insert_binary("/docs/part.adoc", [good, bad, b"\nlast\n"].concat())
                .unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            if selection == "lines=1" {
                assert!(result.diagnostics.is_empty());
                let source = sources.get(SourceId(1)).unwrap();
                assert_eq!(source.text(), "*🇯🇵*\r\n");
                assert!(!source.is_complete());
                assert_eq!(
                    sources.slice(result.document.flat_blocks()[0].span),
                    Some("*🇯🇵*")
                );
                let mut limited = options.clone();
                limited.includes.max_bytes = "*🇯🇵*\r\n".len() - 1;
                let limited = parse_resource(&mut sources, &limited, &resources).unwrap();
                assert!(limited
                    .diagnostics
                    .iter()
                    .any(|d| d.message.contains("maximum included byte count")));
            } else {
                assert_eq!(result.diagnostics.len(), 1, "{name}/{selection}");
                assert!(sources.get(SourceId(1)).is_none());
            }
        }
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[encoding=ISO-2022-JP-KDDI]\n\ninclude::part.adoc[encoding=ISO-2022-JP]\n\ninclude::part.adoc[encoding=CP50220]");
    resources
        .insert_binary("/docs/part.adoc", b"\x1b$B!A\x1b(B\r\n")
        .unwrap();
    let first = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(first.diagnostics.is_empty());
    assert_eq!(
        first
            .document
            .flat_blocks()
            .iter()
            .map(|b| b.span.source_id.0)
            .collect::<Vec<_>>(),
        [1, 2, 1]
    );
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "～\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "〜\r\n");
    resources
        .insert_binary("/docs/part.adoc", b"\x1b$Bv'\x1b(B\r\n")
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(changed.diagnostics.len(), 2);
    assert_eq!(sources.get(SourceId(3)).unwrap().text(), "🇯🇵\r\n");
    assert!(sources.get(SourceId(4)).is_none());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "～\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "〜\r\n");
}

#[test]
fn jis2004_prefixes_decoded_byte_budgets_and_alias_snapshots_stay_consistent() {
    for name in ["EUC-JIS-2004", "EUC-JISX0213"] {
        for selection in ["lines=1", "lines=2", "lines=1;3", "tag=body"] {
            let (mut resources, mut sources, options) = setup(&format!(
                "include::part.adoc[encoding={name},{selection},opts=optional]"
            ));
            resources
                .insert_binary("/docs/part.adoc", b"*\x8f\xa1\xa1*\r\n\xa4\xf7\nlast\n")
                .unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            if selection == "lines=1" {
                assert!(result.diagnostics.is_empty());
                let source = sources.get(SourceId(1)).unwrap();
                assert_eq!(source.text(), "*𠂉*\r\n");
                assert!(!source.is_complete());
                assert_eq!(
                    sources.slice(result.document.flat_blocks()[0].span),
                    Some("*𠂉*")
                );
                let mut limited = options.clone();
                limited.includes.max_bytes = "*𠂉*\r\n".len() - 1;
                let limited = parse_resource(&mut sources, &limited, &resources).unwrap();
                assert!(limited
                    .diagnostics
                    .iter()
                    .any(|d| d.message.contains("maximum included byte count")));
            } else {
                assert_eq!(result.diagnostics.len(), 1, "{name}/{selection}");
                assert!(sources.get(SourceId(1)).is_none());
            }
        }
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[encoding=EUC-JIS-2004]\n\ninclude::part.adoc[encoding=EUC-JISX0213]\n\ninclude::part.adoc[encoding=EUC-JP]");
    resources
        .insert_binary("/docs/part.adoc", b"\xc6\xfc\xcb\xdc\xb8\xec\r\n")
        .unwrap();
    let first = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(first.diagnostics.is_empty());
    assert!(first
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.span.source_id == SourceId(1)));
    resources
        .insert_binary("/docs/part.adoc", b"\x8f\xa1\xa1\r\n")
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(changed.diagnostics.len(), 1);
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "𠂉\r\n");
    assert!(sources.get(SourceId(3)).is_none());
    resources
        .insert_binary("/docs/part.adoc", b"\x8f\xa2\xaf\r\n")
        .unwrap();
    let legacy = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(legacy.diagnostics.len(), 2);
    assert_eq!(sources.get(SourceId(3)).unwrap().text(), "˘\r\n");
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "日本語\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "𠂉\r\n");
    assert!(sources.get(SourceId(4)).is_none());
}

#[test]
fn sjis_kddi_prefixes_budgets_and_windows_variants_preserve_decoded_snapshots() {
    for selection in ["lines=1", "lines=2", "lines=1;3", "tag=body"] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding=SJIS-KDDI,{selection},opts=optional]"
        ));
        resources
            .insert_binary("/docs/part.adoc", b"*\xf6\xa5*\r\n\xf6\x4a\nlast\n")
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        if selection == "lines=1" {
            assert!(result.diagnostics.is_empty());
            let source = sources.get(SourceId(1)).unwrap();
            assert_eq!(source.text(), "*🇯🇵*\r\n");
            assert!(!source.is_complete());
            assert_eq!(
                sources.slice(result.document.flat_blocks()[0].span),
                Some("*🇯🇵*")
            );
            let mut limited = options.clone();
            limited.includes.max_bytes = "*🇯🇵*\r\n".len() - 1;
            let limited = parse_resource(&mut sources, &limited, &resources).unwrap();
            assert!(limited
                .diagnostics
                .iter()
                .any(|d| d.message.contains("maximum included byte count")));
        } else {
            assert_eq!(result.diagnostics.len(), 1, "{selection}");
            assert!(sources.get(SourceId(1)).is_none());
        }
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[encoding=SJIS-KDDI]\n\ninclude::part.adoc[encoding=sjis-kddi]\n\ninclude::part.adoc[encoding=Windows-31J]");
    resources
        .insert_binary("/docs/part.adoc", b"\xf6\xa5\r\n")
        .unwrap();
    let first = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(first.diagnostics.is_empty());
    assert_eq!(
        first
            .document
            .flat_blocks()
            .iter()
            .map(|b| b.span.source_id.0)
            .collect::<Vec<_>>(),
        [1, 1, 2]
    );
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "🇯🇵\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "\u{e4cc}\r\n");
    resources
        .insert_binary("/docs/part.adoc", b"\xf0\x40\r\n")
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(changed.diagnostics.is_empty());
    assert!(changed
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.span.source_id == SourceId(3)));
    assert_eq!(sources.get(SourceId(3)).unwrap().text(), "\u{e000}\r\n");
    assert!(sources.get(SourceId(4)).is_none());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "🇯🇵\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "\u{e4cc}\r\n");
}

#[test]
fn sjis_docomo_prefixes_byte_budgets_and_windows_variants_preserve_snapshots() {
    for selection in ["lines=1", "lines=2", "lines=1;3", "tag=body"] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding=SJIS-DoCoMo,{selection},opts=optional]"
        ));
        resources
            .insert_binary("/docs/part.adoc", b"*\xf9\x87*\r\n\xf9\x96\nlast\n")
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        if selection == "lines=1" {
            assert!(result.diagnostics.is_empty());
            let source = sources.get(SourceId(1)).unwrap();
            assert_eq!(source.text(), "*1\u{20e3}*\r\n");
            assert!(!source.is_complete());
            assert_eq!(
                sources.slice(result.document.flat_blocks()[0].span),
                Some("*1\u{20e3}*")
            );
            let mut limited = options.clone();
            limited.includes.max_bytes = "*1\u{20e3}*\r\n".len() - 1;
            let limited = parse_resource(&mut sources, &limited, &resources).unwrap();
            assert!(limited
                .diagnostics
                .iter()
                .any(|d| d.message.contains("maximum included byte count")));
        } else {
            assert_eq!(result.diagnostics.len(), 1, "{selection}");
            assert!(sources.get(SourceId(1)).is_none());
        }
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[encoding=SJIS-DoCoMo]\n\ninclude::part.adoc[encoding=sjis-docomo]\n\ninclude::part.adoc[encoding=Windows-31J]");
    resources
        .insert_binary("/docs/part.adoc", b"\xf9\x87\r\n")
        .unwrap();
    let first = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(first.diagnostics.is_empty());
    assert_eq!(
        first
            .document
            .flat_blocks()
            .iter()
            .map(|b| b.span.source_id.0)
            .collect::<Vec<_>>(),
        [1, 1, 2]
    );
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "1\u{20e3}\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "\u{e6e2}\r\n");
    resources
        .insert_binary("/docs/part.adoc", b"\xf0\x40\r\n")
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(changed.diagnostics.is_empty());
    assert!(changed
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.span.source_id == SourceId(3)));
    assert_eq!(sources.get(SourceId(3)).unwrap().text(), "\u{e000}\r\n");
    assert!(sources.get(SourceId(4)).is_none());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "1\u{20e3}\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "\u{e6e2}\r\n");
}

#[test]
fn softbank_duplicate_bytes_reuse_snapshots_and_prefixes_keep_decoded_budgets() {
    for selection in ["lines=1", "lines=2", "lines=1;3", "tag=body"] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding=SJIS-SoftBank,{selection},opts=optional]"
        ));
        resources
            .insert_binary("/docs/part.adoc", b"*\xf6\xe4*\r\n\xf9\x9a\nlast\n")
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        if selection == "lines=1" {
            assert!(result.diagnostics.is_empty());
            let source = sources.get(SourceId(1)).unwrap();
            assert_eq!(source.text(), "*🇯🇵*\r\n");
            assert!(!source.is_complete());
            assert_eq!(
                sources.slice(result.document.flat_blocks()[0].span),
                Some("*🇯🇵*")
            );
            let mut limited = options.clone();
            limited.includes.max_bytes = "*🇯🇵*\r\n".len() - 1;
            let limited = parse_resource(&mut sources, &limited, &resources).unwrap();
            assert!(limited
                .diagnostics
                .iter()
                .any(|d| d.message.contains("maximum included byte count")));
        } else {
            assert_eq!(result.diagnostics.len(), 1, "{selection}");
            assert!(sources.get(SourceId(1)).is_none());
        }
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[encoding=SJIS-SoftBank]\n\ninclude::part.adoc[encoding=sjis-softbank]\n\ninclude::part.adoc[encoding=Windows-31J]");
    resources
        .insert_binary("/docs/part.adoc", b"\xf6\xe4\r\n")
        .unwrap();
    let first = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(first.diagnostics.is_empty());
    assert_eq!(
        first
            .document
            .flat_blocks()
            .iter()
            .map(|b| b.span.source_id.0)
            .collect::<Vec<_>>(),
        [1, 1, 2]
    );
    resources
        .insert_binary("/docs/part.adoc", b"\xfb\xab\r\n")
        .unwrap();
    let duplicate = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(duplicate.diagnostics.is_empty());
    assert_eq!(
        duplicate
            .document
            .flat_blocks()
            .iter()
            .map(|b| b.span.source_id.0)
            .collect::<Vec<_>>(),
        [1, 1, 3]
    );
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "🇯🇵\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "\u{e50b}\r\n");
    assert_eq!(sources.get(SourceId(3)).unwrap().text(), "\u{8b53}\r\n");
    resources
        .insert_binary("/docs/part.adoc", b"\xf0\x40\r\n")
        .unwrap();
    let common = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(common.diagnostics.is_empty());
    assert!(common
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.span.source_id == SourceId(4)));
    assert_eq!(sources.get(SourceId(4)).unwrap().text(), "\u{e000}\r\n");
    assert!(sources.get(SourceId(5)).is_none());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "🇯🇵\r\n");
}

#[test]
fn utf8_mac_selection_snapshots_and_budgets_use_composed_utf8() {
    for (selection, bad, success) in [
        ("lines=1", &b"A\xff"[..], true),
        ("lines=1", &b"\xff"[..], false),
        ("lines=1;3", &b"\xff"[..], false),
        ("tag=body", &b"\xff"[..], false),
    ] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding=UTF8-MAC,{selection},opts=optional]"
        ));
        resources
            .insert_binary(
                "/docs/part.adoc",
                ["*e\u{301}*\r\n".as_bytes(), bad, b"\nlast\n"].concat(),
            )
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        if success {
            assert!(result.diagnostics.is_empty());
            let source = sources.get(SourceId(1)).unwrap();
            assert_eq!(source.text(), "*é*\r\n");
            assert!(!source.is_complete());
            assert!(source.omitted_lines().is_empty());
            let mut limited = options.clone();
            limited.includes.max_bytes = source.text().len() - 1;
            assert!(parse_resource(&mut sources, &limited, &resources)
                .unwrap()
                .diagnostics
                .iter()
                .any(|d| d.message.contains("maximum included byte count")));
        } else {
            assert_eq!(result.diagnostics.len(), 1);
            assert!(sources.get(SourceId(1)).is_none());
        }
    }
    let (mut resources,mut sources,options)=setup("include::part.adoc[encoding=UTF8-MAC,lines=2]\n\ninclude::part.adoc[encoding=utf-8-mac,lines=2]\n\ninclude::part.adoc[encoding=UTF-8-HFS,lines=2]");
    resources
        .insert_binary(
            "/docs/part.adoc",
            "ignored\r\n\u{feff}*e\u{301}*\r\n".as_bytes(),
        )
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    let source = sources.get(SourceId(1)).unwrap();
    assert_eq!(source.text(), "ignored\r\n\u{feff}*é*\r\n");
    for block in result.document.flat_blocks() {
        assert_eq!(block.span.source_id, SourceId(1));
        assert_eq!((block.span.start, block.span.end), (12, 16));
        assert_eq!(sources.slice(block.span), Some("*é*"));
    }
    assert_eq!(
        source.position(15).unwrap(),
        adoctra_core::SourcePosition { line: 2, column: 4 }
    );
    assert!(sources.get(SourceId(2)).is_none());
    assert_eq!(
        sources
            .load_with_encoding("/docs/part.adoc", None, &resources, Some("UTF-8"))
            .unwrap(),
        SourceId(2)
    );
    assert_eq!(
        sources.get(SourceId(2)).unwrap().text(),
        "ignored\r\n\u{feff}*e\u{301}*\r\n"
    );
    resources
        .insert_binary("/docs/part.adoc", "ignored\r\n\u{feff}*é*\r\n".as_bytes())
        .unwrap();
    assert!(parse_resource(&mut sources, &options, &resources)
        .unwrap()
        .diagnostics
        .is_empty());
    assert!(sources.get(SourceId(3)).is_none());
    resources
        .insert_binary(
            "/docs/part.adoc",
            "ignored\r\n\u{feff}*\u{3b1}\u{313}\u{300}\u{345}*\r\n".as_bytes(),
        )
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(changed.diagnostics.is_empty());
    assert!(changed
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.span.source_id == SourceId(3)));
    assert_eq!(
        sources.get(SourceId(3)).unwrap().text(),
        "ignored\r\n\u{feff}*\u{1f82}*\r\n"
    );
    assert_eq!(
        sources.get(SourceId(1)).unwrap().text(),
        "ignored\r\n\u{feff}*é*\r\n"
    );
}

#[test]
fn cesu8_selection_budgets_and_snapshots_use_decoded_supplementary_scalars() {
    for (selection, bad, success) in [
        ("lines=1", &b"\xed\xa0\x80"[..], true),
        ("lines=1", &b"\xf0\x9f\x98\x80"[..], true),
        ("lines=1;3", &b"\xed\xa0\x80"[..], false),
        ("lines=1;3", &b"\xff"[..], false),
        ("tag=body", &b"\xed\xb0\x80"[..], false),
    ] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding=CESU-8,{selection},opts=optional]"
        ));
        resources
            .insert_binary(
                "/docs/part.adoc",
                [&b"*\xed\xa0\xbd\xed\xb8\x80*\r\n"[..], bad, b"\nlast\n"].concat(),
            )
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        if success {
            assert!(result.diagnostics.is_empty());
            let source = sources.get(SourceId(1)).unwrap();
            assert_eq!(source.text(), "*😀*\r\n");
            assert!(!source.is_complete());
            assert!(source.omitted_lines().is_empty());
            let mut limited = options.clone();
            limited.includes.max_bytes = source.text().len() - 1;
            assert!(parse_resource(&mut sources, &limited, &resources)
                .unwrap()
                .diagnostics
                .iter()
                .any(|d| d.message.contains("maximum included byte count")));
        } else {
            assert_eq!(result.diagnostics.len(), 1);
            assert!(sources.get(SourceId(1)).is_none());
        }
    }
    let (mut resources, mut sources, options) = setup("include::part.adoc[encoding=CESU-8,lines=2]\n\ninclude::part.adoc[encoding=cesu-8,lines=2]");
    resources
        .insert_binary(
            "/docs/part.adoc",
            b"ignored\r\n\xef\xbb\xbf*\xed\xa0\xbd\xed\xb8\x80*\r\n",
        )
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(
        sources.get(SourceId(1)).unwrap().text(),
        "ignored\r\n\u{feff}*😀*\r\n"
    );
    for block in result.document.flat_blocks() {
        assert_eq!(block.span.source_id, SourceId(1));
        assert_eq!((block.span.start, block.span.end), (12, 18));
        assert_eq!(sources.slice(block.span), Some("*😀*"));
    }
    assert_eq!(
        sources.get(SourceId(1)).unwrap().position(17).unwrap(),
        adoctra_core::SourcePosition { line: 2, column: 4 }
    );
    assert!(sources.get(SourceId(2)).is_none());
    // Equivalent UTF-8 can reuse the canonical decoded snapshot through the provider API.
    resources
        .insert_binary("/docs/part.adoc", "ignored\r\n\u{feff}*😀*\r\n".as_bytes())
        .unwrap();
    assert_eq!(
        sources
            .load_with_encoding("/docs/part.adoc", None, &resources, Some("UTF-8"))
            .unwrap(),
        SourceId(1)
    );
    let invalid = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(invalid.diagnostics.len(), 2);
    assert!(sources.get(SourceId(2)).is_none());
    resources
        .insert_binary(
            "/docs/part.adoc",
            b"ignored\r\n*\xed\xa0\x80\xed\xb0\x80*\r\n",
        )
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(changed.diagnostics.is_empty());
    assert_eq!(
        sources.get(SourceId(2)).unwrap().text(),
        "ignored\r\n*\u{10000}*\r\n"
    );
    assert_eq!(
        sources.get(SourceId(1)).unwrap().text(),
        "ignored\r\n\u{feff}*😀*\r\n"
    );
}

#[test]
fn utf8_kddi_duplicate_banks_reuse_snapshots_and_count_decoded_flag_bytes() {
    let (mut resources, mut sources, options) = setup("include::part.adoc[encoding=UTF8-KDDI,lines=2]\n\ninclude::part.adoc[encoding=utf8-kddi,lines=2]");
    for input in ["\u{e4cc}", "\u{efa5}", "🇯🇵"] {
        resources
            .insert_binary(
                "/docs/part.adoc",
                format!("ignored\r\n\u{feff}*{input}*\r\n").as_bytes(),
            )
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty(), "{input:?}");
        let source = sources.get(SourceId(1)).unwrap();
        assert_eq!(source.text(), "ignored\r\n\u{feff}*🇯🇵*\r\n");
        assert!(source.is_complete());
        for block in result.document.flat_blocks() {
            assert_eq!(block.span.source_id, SourceId(1));
            assert_eq!(sources.slice(block.span), Some("*🇯🇵*"));
            assert_eq!(block.span.start, 12);
            assert_eq!(
                source.position(21).unwrap(),
                adoctra_core::SourcePosition { line: 2, column: 5 }
            );
        }
        assert!(sources.get(SourceId(2)).is_none());
        let mut limited = options.clone();
        limited.includes.max_bytes = source.text().len() - 1;
        let result = parse_resource(&mut sources, &limited, &resources).unwrap();
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("maximum included byte count")));
    }
    resources
        .insert_binary(
            "/docs/part.adoc",
            "ignored\r\n*\u{effb}*\r\n\u{e472}".as_bytes(),
        )
        .unwrap();
    let mut prefix_options = options.clone();
    let prefix = parse_resource(&mut sources, &prefix_options, &resources).unwrap();
    assert!(prefix.diagnostics.is_empty());
    let source = sources.get(SourceId(2)).unwrap();
    assert_eq!(source.text(), "ignored\r\n*1\u{20e3}*\r\n");
    assert!(!source.is_complete());
    assert!(source.omitted_lines().is_empty());
    prefix_options.includes.max_bytes = source.text().len() - 1;
    let limited = parse_resource(&mut sources, &prefix_options, &resources).unwrap();
    assert!(limited
        .diagnostics
        .iter()
        .any(|d| d.message.contains("maximum included byte count")));
    // Undefined PUA and malformed UTF-8 on skipped lines are still consumed.
    for bad in ["\u{e472}".as_bytes(), &b"\xff"[..]] {
        for selection in ["lines=1;5", "tag=body"] {
            let (mut resource, mut map, opts) = setup(&format!(
                "include::part.adoc[encoding=UTF8-KDDI,{selection},opts=optional]"
            ));
            resource
                .insert_binary(
                    "/docs/part.adoc",
                    [
                        b"// tag::body[]\n",
                        "\u{e4cc}\n".as_bytes(),
                        b"// end::body[]\n",
                        bad,
                        b"\nlast\n",
                    ]
                    .concat(),
                )
                .unwrap();
            let result = parse_resource(&mut map, &opts, &resource).unwrap();
            assert_eq!(result.diagnostics.len(), 1);
            assert!(map.get(SourceId(1)).is_none());
        }
    }
    assert_eq!(
        sources.get(SourceId(1)).unwrap().text(),
        "ignored\r\n\u{feff}*🇯🇵*\r\n"
    );
}

#[test]
fn utf8_docomo_strict_selection_and_unicode_snapshots_keep_identity_and_budgets() {
    utf8_mobile_selection_and_identity(
        "UTF8-DoCoMo",
        "utf8-docomo",
        "\u{e6e2}",
        "\u{e66c}",
        "\u{e6f1}",
    );
}

#[test]
fn utf8_softbank_strict_selection_and_unicode_snapshots_keep_identity_and_budgets() {
    utf8_mobile_selection_and_identity(
        "UTF8-SoftBank",
        "utf8-softbank",
        "\u{e21c}",
        "\u{e14f}",
        "\u{e059}",
    );
}

fn utf8_mobile_selection_and_identity(
    name: &str,
    lower: &str,
    keycap: &str,
    supplementary: &str,
    undefined: &str,
) {
    for (selection, bad, success) in [
        ("lines=1", undefined.as_bytes(), true),
        ("lines=1", &b"\xff"[..], true),
        ("lines=2", undefined.as_bytes(), false),
        ("lines=1;3", undefined.as_bytes(), false),
        ("lines=1;3", &b"\xff"[..], false),
        ("tag=body", undefined.as_bytes(), false),
        ("tag=body", &b"\xff"[..], false),
    ] {
        let (mut resources, mut sources, options) = setup(&format!(
            "include::part.adoc[encoding={name},{selection},opts=optional]"
        ));
        resources
            .insert_binary(
                "/docs/part.adoc",
                [format!("*{keycap}*\r\n").as_bytes(), bad, b"\nlast\n"].concat(),
            )
            .unwrap();
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        if success {
            assert!(result.diagnostics.is_empty());
            let source = sources.get(SourceId(1)).unwrap();
            assert_eq!(source.text(), "*1\u{20e3}*\r\n");
            assert!(!source.is_complete());
            assert!(source.omitted_lines().is_empty());
            assert_eq!(
                sources.slice(result.document.flat_blocks()[0].span),
                Some("*1\u{20e3}*")
            );
            let mut limited = options.clone();
            limited.includes.max_bytes = "*1\u{20e3}*\r\n".len() - 1;
            let limited = parse_resource(&mut sources, &limited, &resources).unwrap();
            assert!(limited
                .diagnostics
                .iter()
                .any(|d| d.message.contains("maximum included byte count")));
        } else {
            assert_eq!(result.diagnostics.len(), 1, "{selection}/{bad:?}");
            assert!(sources.get(SourceId(1)).is_none());
        }
    }
    let (mut resources, mut sources, options) =
        setup(&format!("include::part.adoc[encoding={name},lines=2]"));
    resources
        .insert_binary(
            "/docs/part.adoc",
            format!("ignored\r\n\u{feff}*{keycap}*\r\n").as_bytes(),
        )
        .unwrap();
    let bom = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(bom.diagnostics.is_empty());
    assert_eq!(
        sources.get(SourceId(1)).unwrap().text(),
        "ignored\r\n\u{feff}*1\u{20e3}*\r\n"
    );
    let block = bom.document.flat_blocks()[0];
    assert_eq!(sources.slice(block.span), Some("*1\u{20e3}*"));
    assert_eq!(block.span.start, 12);
    assert_eq!(
        sources
            .get(SourceId(1))
            .unwrap()
            .position(block.span.start)
            .unwrap(),
        adoctra_core::SourcePosition { line: 2, column: 2 }
    );
    let (mut resources, mut sources, options) = setup(&format!("include::part.adoc[encoding={name}]\n\ninclude::part.adoc[encoding={lower}]\n\ninclude::part.adoc[encoding=UTF-8]"));
    resources
        .insert_binary("/docs/part.adoc", format!("{keycap}\r\n").as_bytes())
        .unwrap();
    let first = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(first.diagnostics.is_empty());
    assert_eq!(
        first
            .document
            .flat_blocks()
            .iter()
            .map(|b| b.span.source_id.0)
            .collect::<Vec<_>>(),
        [1, 1, 2]
    );
    resources
        .insert_binary("/docs/part.adoc", "1\u{20e3}\r\n".as_bytes())
        .unwrap();
    let equivalent = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(equivalent.diagnostics.is_empty());
    assert!(equivalent
        .document
        .flat_blocks()
        .iter()
        .all(|b| b.span.source_id == SourceId(1)));
    assert!(sources.get(SourceId(3)).is_none());
    assert_eq!(
        sources.get(SourceId(2)).unwrap().text(),
        format!("{keycap}\r\n")
    );
    resources
        .insert_binary("/docs/part.adoc", format!("{supplementary}\r\n").as_bytes())
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(changed.diagnostics.is_empty());
    assert_eq!(sources.get(SourceId(3)).unwrap().text(), "🅿\r\n");
    assert_eq!(
        sources.get(SourceId(4)).unwrap().text(),
        format!("{supplementary}\r\n")
    );
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "1\u{20e3}\r\n");
}

#[test]
fn gb_variants_and_aliases_keep_immutable_decoded_source_identity() {
    let names = ["GB2312", "GB12345", "GBK", "GB18030", "CP936", "eucCN"];
    let root = names
        .map(|name| format!("include::part.adoc[encoding={name}]"))
        .join("\n\n");
    let (mut resources, mut sources, options) = setup(&root);
    resources
        .insert_binary("/docs/part.adoc", b"\xb9\xfa\r\n")
        .unwrap();
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty());
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "国\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "國\r\n");
    assert_eq!(
        result
            .document
            .flat_blocks()
            .iter()
            .map(|block| block.span.source_id.0)
            .collect::<Vec<_>>(),
        [1, 2, 1, 1, 1, 1]
    );
    resources
        .insert_binary("/docs/part.adoc", b"\xd6\xd0\r\n")
        .unwrap();
    let changed = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(changed.diagnostics.is_empty());
    assert!(changed
        .document
        .flat_blocks()
        .iter()
        .all(|block| block.span.source_id == SourceId(3)));
    assert_eq!(sources.get(SourceId(1)).unwrap().text(), "国\r\n");
    assert_eq!(sources.get(SourceId(2)).unwrap().text(), "國\r\n");
}

#[test]
fn gb_prefix_reads_validate_skipped_bytes_and_incomplete_four_byte_sequences() {
    for name in ["GB2312", "GB12345", "GBK", "GB18030"] {
        for (selection, bytes, success) in [
            ("lines=1", &b"*\xd6\xd0*\r\n\x81"[..], true),
            ("lines=2", &b"*\xd6\xd0*\r\n\x81"[..], false),
            ("lines=1;3", &b"first\n\x81\nlast\n"[..], false),
            (
                "tag=body",
                &b"// tag::body[]\n\xd6\xd0\n// end::body[]\n\x81"[..],
                false,
            ),
            ("lines=2", &b"*\xd6\xd0*\r\n\x81\x30\x81\n"[..], false),
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

#[test]
fn a_second_bom_after_the_removed_one_is_content_like_ruby() {
    // `bom_bytes` prepends the first BOM; the text holds the second.
    let utf16 = |little: bool| bom_bytes("\u{feff}*a*\n", little);
    let utf32be: Vec<u8> = "\u{feff}\u{feff}*a*\n"
        .chars()
        .flat_map(|c| (c as u32).to_be_bytes())
        .collect();
    let utf8 = b"\xef\xbb\xbf\xef\xbb\xbf*a*\n".to_vec();
    // Ruby fails on a selection of a default/BINARY UTF-16 read (the raw
    // lines are converted one by one), so those only read whole.
    let kept = "\u{feff}*a*";
    for (bytes, encoding, select, expected) in [
        (utf8.clone(), None, true, kept),
        (utf8.clone(), Some("CESU-8"), true, kept),
        (utf8, Some("UTF8-MAC"), true, kept),
        (utf16(true), None, false, kept),
        (utf16(true), Some("UTF-16LE"), true, kept),
        (utf16(false), Some("BINARY"), false, kept),
        (utf16(false), Some("UTF-16BE"), true, kept),
        // The dummy converters consume the BOM they read the order from;
        // the BOM check then removes the second one, like a transcoded read.
        (utf16(false), Some("UTF-16"), true, "*a*"),
        (utf32be.clone(), Some("UTF-32BE"), true, kept),
        (utf32be, Some("UTF-32"), true, "*a*"),
    ] {
        for selection in ["", "lines=1"].into_iter().take(1 + usize::from(select)) {
            let attrs = encoding
                .map(|name| format!("encoding={name}"))
                .into_iter()
                .chain((!selection.is_empty()).then(|| selection.to_owned()))
                .collect::<Vec<_>>()
                .join(",");
            let (mut resources, mut sources, options) =
                setup(&format!("include::part.adoc[{attrs}]"));
            resources
                .insert_binary("/docs/part.adoc", bytes.clone())
                .unwrap();
            let result = parse_resource(&mut sources, &options, &resources).unwrap();
            let context = format!("{encoding:?}{selection}");
            assert!(
                result.diagnostics.is_empty(),
                "{context} {:?}",
                result.diagnostics
            );
            let block = &result.document.flat_blocks()[0];
            assert!(
                matches!(block.kind, BlockKind::Paragraph { .. }),
                "{context}"
            );
            assert_eq!(block.span.source_id, SourceId(1), "{context}");
            assert_eq!(sources.slice(block.span), Some(expected), "{context}");
        }
    }
}
