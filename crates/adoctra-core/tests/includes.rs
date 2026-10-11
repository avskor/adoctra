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

fn paragraphs(result: &ParseResult) -> Vec<String> {
    result
        .document
        .blocks
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
                blocks: vec![result.document.blocks[2].clone()],
                ..Default::default()
            },
            ..result.clone()
        }),
        ["yes"]
    );
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert!(matches!(&inlines[1], Inline::CrossReference { .. }));
    assert_eq!(
        sources
            .get(result.document.blocks[0].span.source_id)
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
    assert_eq!(result.document.blocks.len(), 1);
    let block = &result.document.blocks[0];
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
    assert_eq!(result.document.blocks[0].span.start, 3);
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
                matches!(&result.document.blocks[0].kind, BlockKind::List { items, .. } if items.len() == 3)
            ),
            _ => assert!(
                matches!(&result.document.blocks[0].kind, BlockKind::Example { blocks } if blocks.len() == 1)
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
        if let BlockKind::Example { blocks } = &result.document.blocks[0].kind {
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
    let fragments = &result.provenance[result.document.blocks[0].span.provenance.unwrap()];
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
fn cycles_limits_missing_files_and_unsupported_options_leave_partial_output() {
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
            "include::part.adoc[lines=1]\n\nafter",
            IncludeLimits::default(),
            "options",
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
            matches!(&result.document.blocks[0].kind, BlockKind::Listing { text } if text == expected)
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
