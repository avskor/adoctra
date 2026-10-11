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
            "include::part.adoc[encoding=UTF-16]\n\nafter",
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
    let block = &result.document.blocks[0];
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
        matches!(&result.document.blocks[0].kind, BlockKind::Listing { text } if text == "kept\nnotag::b[]\n// tag::bad name[]\n// tag::bad[]\t\ninside")
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
            "include::missing.adoc[opts=optional,encoding=UTF-16]",
            IncludeLimits::default(),
            "options",
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
        .blocks
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
        result.document.blocks[1]
            .attribute_context
            .get("leveloffset"),
        Some("1")
    );
    assert_eq!(
        result.document.blocks[2]
            .attribute_context
            .get("leveloffset"),
        Some("2")
    );
    assert_eq!(
        result.document.blocks[4]
            .attribute_context
            .get("leveloffset"),
        Some("1")
    );
    assert_eq!(
        result.document.blocks[5]
            .attribute_context
            .get("leveloffset"),
        None
    );
    assert_eq!(
        sources.slice(result.document.blocks[1].span),
        Some("= Глава")
    );
    assert_eq!(
        result.document.blocks[1].span,
        Span::in_source(SourceId(1), 3, 3 + "= Глава".len())
    );
    let BlockKind::Heading { inlines, .. } = &result.document.blocks[1].kind else {
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
    let BlockKind::Listing { text } = &result.document.blocks[0].kind else {
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
    assert!(result.diagnostics.is_empty());
    assert_eq!(levels(&result), [2, 3]);
    assert!(result
        .document
        .blocks
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
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!("{result:?}")
    };
    assert!(matches!(inlines[0], Inline::Strong { .. }));
    assert!(result.document.blocks[0].span.provenance.is_some());
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
    let BlockKind::Listing { text } = &result.document.blocks[0].kind else {
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
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
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
