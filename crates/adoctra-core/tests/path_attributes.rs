use adoctra_core::{
    parse, parse_resource, parse_with_options, tokens_resource, BlockKind, Inline, MemoryResources,
    ParseOptions, Resource, ResourceProvider, ResourceRequest, ResourceResult, SourceId, SourceMap,
    Span, TokenKind,
};

const KEYS: [&str; 4] = ["docfile", "docdir", "docname", "docfilesuffix"];

#[test]
fn primary_paths_use_reference_basename_and_suffix_rules() {
    for (path, dir, name, suffix) in [
        ("/docs/guide.adoc", "/docs", "guide", ".adoc"),
        ("/docs.v1/guide", "/docs.v1", "guide", ""),
        ("/docs/guide.tar.adoc", "/docs", "guide.tar", ".adoc"),
        ("/docs/.hidden", "/docs", ".hidden", ".hidden"),
        ("/docs/.hidden.adoc", "/docs", ".hidden", ".adoc"),
        ("/docs/guide.", "/docs", "guide", "."),
        ("/docs/guide..adoc", "/docs", "guide.", ".adoc"),
        ("/имя 🙂.ADOC", "/", "имя 🙂", ".ADOC"),
    ] {
        let mut resources = MemoryResources::new();
        resources.insert(path, "{docname}").unwrap();
        let mut sources = SourceMap::new();
        let source_id = sources.load(path, None, &resources).unwrap();
        let options = ParseOptions {
            source_id,
            ..Default::default()
        };
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty(), "{result:?}");
        for (key, expected) in KEYS.into_iter().zip([path, dir, name, suffix]) {
            assert_eq!(
                result.document.attributes.get(key),
                Some(expected),
                "{path}: {key}"
            );
            assert_eq!(
                result.document.blocks[0].attribute_context.get(key),
                Some(expected)
            );
            assert_eq!(options.attributes.get(key), None);
        }
        let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
            panic!()
        };
        assert!(
            matches!(&inlines[0], Inline::AttributeRef { value: Some(value), span, .. }
            if value == name && *span == Span::in_source(source_id, 0, 9))
        );
    }
}

#[test]
fn derived_paths_are_locked_in_header_body_and_included_files() {
    let mut resources = MemoryResources::new();
    let declarations =
        ":docname: changed \\\n ignored\n:docdir!: gone\n:!docfile: gone\n:docfilesuffix: .other\n";
    let main = format!("{declarations}= {{docname}}\n\nBefore {{docname}}.\n\ninclude::parts/child.adoc[]\n\n{declarations}\nAfter {{docname}}.");
    resources.insert("docs/main.adoc", &main).unwrap();
    resources
        .insert(
            "docs/parts/child.adoc",
            format!("{declarations}\nInside {{docname}}."),
        )
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("docs/main.adoc", None, &resources).unwrap();
    let mut options = ParseOptions {
        source_id,
        ..Default::default()
    };
    for key in KEYS {
        options.attributes.set(key, "caller");
    }
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{result:?}");
    assert_eq!(result.document.blocks.len(), 4);
    for block in &result.document.blocks {
        for (key, expected) in KEYS
            .into_iter()
            .zip(["/docs/main.adoc", "/docs", "main", ".adoc"])
        {
            assert_eq!(result.document.attributes.get(key), Some(expected));
            assert_eq!(block.attribute_context.get(key), Some(expected));
            assert_eq!(options.attributes.get(key), Some("caller"));
        }
    }
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    let start = main.find(" ignored").unwrap();
    assert!(tokens.iter().any(|t| t.kind == TokenKind::AttributeEntry
        && t.span == Span::in_source(source_id, start, start + " ignored".len())));
}

#[test]
fn nested_targets_conditions_and_selection_use_primary_paths_with_physical_spans() {
    let mut resources = MemoryResources::new();
    let main = "\u{feff}= {docname}\r\n:target: {docdir}/parts/{docname}{docfilesuffix}\r\n\r\ninclude::{target}[tag={docname},indent=0]\r\n\r\nAfter {docfilesuffix}.";
    let child = "// tag::главная[]\r\n  ifeval::[\"{docname}\" == \"главная\"]\r\n  include::{docdir}/leaf.adoc[]\r\n  endif::[]\r\n\r\n  Я {docname} <<missing>>\r\n// end::главная[]";
    resources.insert("docs/главная.adoc", main).unwrap();
    resources.insert("docs/parts/главная.adoc", child).unwrap();
    resources.insert("docs/leaf.adoc", "*{docname}*").unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("docs/главная.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert_eq!(
        result.document.attributes.get("target"),
        Some("/docs/parts/главная.adoc")
    );
    assert_eq!(result.document.blocks.len(), 4, "{result:?}");
    assert_eq!(result.include_directives.len(), 2);
    assert_eq!(result.conditional_directives.len(), 2);
    let child_id = sources
        .load("docs/parts/главная.adoc", None, &resources)
        .unwrap();
    let leaf_id = sources.load("docs/leaf.adoc", None, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.blocks[2].kind else {
        panic!()
    };
    let attr_start = child.find("Я {docname}").unwrap() + "Я ".len();
    assert!(inlines.iter().any(
        |i| matches!(i, Inline::AttributeRef { value: Some(value), span, .. }
        if value == "главная" && *span == Span::in_source(child_id, attr_start, attr_start + 9))
    ));
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = result.diagnostics[0].span;
    assert_eq!(diagnostic.source_id, child_id);
    assert_eq!(sources.slice(diagnostic), Some("<<missing>>"));
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(tokens.iter().any(|t| t.kind == TokenKind::AttributeRef
        && t.span == Span::in_source(child_id, attr_start, attr_start + 9)));
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Strong && t.span.source_id == leaf_id));
    assert_eq!(sources.get(child_id).unwrap().text(), child);
    assert_eq!(sources.get(source_id).unwrap().text(), main);
}

#[test]
fn loaded_primary_snapshots_need_no_reload_and_keep_independent_path_contexts() {
    let mut resources = MemoryResources::new();
    resources.insert("docs/old.adoc", "{docname}").unwrap();
    let mut sources = SourceMap::new();
    let old = sources.load("docs/old.adoc", None, &resources).unwrap();
    resources.remove("docs/old.adoc").unwrap();
    resources
        .insert("other/new.adoc", "{docname} changed")
        .unwrap();
    let new = sources.load("other/new.adoc", None, &resources).unwrap();
    for (source_id, expected) in [(old, "old"), (new, "new"), (old, "old")] {
        let options = ParseOptions {
            source_id,
            ..Default::default()
        };
        let result = parse_resource(&mut sources, &options, &resources).unwrap();
        assert_eq!(result.document.attributes.get("docname"), Some(expected));
        assert!(result.diagnostics.is_empty());
    }
    assert_eq!(sources.get(old).unwrap().text(), "{docname}");
}

struct Opaque;
impl ResourceProvider for Opaque {
    fn load(&self, _: ResourceRequest<'_>) -> ResourceResult<Resource> {
        panic!("parsing an existing primary snapshot must not reload it")
    }
}

#[test]
fn string_and_opaque_resources_do_not_invent_paths_and_keep_explicit_overrides() {
    let text = ":docname: local\n\n{docname}";
    let mut sources = SourceMap::new();
    let source_id = sources.add("opaque/id.with.dots", text);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    for result in [
        parse(text),
        parse_resource(&mut sources, &options, &Opaque).unwrap(),
    ] {
        assert_eq!(result.document.attributes.get("docname"), Some("local"));
        for key in ["docfile", "docdir", "docfilesuffix"] {
            assert_eq!(result.document.attributes.get(key), None);
        }
    }
    let mut options = options;
    options.attributes.set("docname", "host");
    for result in [
        parse_with_options(text, &options),
        parse_resource(&mut sources, &options, &Opaque).unwrap(),
    ] {
        assert_eq!(result.document.attributes.get("docname"), Some("host"));
    }
    assert!(parse_resource(
        &mut sources,
        &ParseOptions {
            source_id: SourceId(999),
            ..Default::default()
        },
        &Opaque
    )
    .is_err());
}

#[test]
fn displayed_paths_do_not_replace_canonical_provider_referrers() {
    struct Provider(&'static str);
    impl ResourceProvider for Provider {
        fn document_path(&self, name: &str) -> Option<String> {
            assert_eq!(name, "primary-id");
            Some(self.0.into())
        }
        fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
            assert_eq!(request.referrer, Some("primary-id"));
            assert_eq!(request.target, "guide.adoc");
            Ok(Resource {
                name: "child-id".into(),
                text: "{docfile}".into(),
            })
        }
    }
    let mut sources = SourceMap::new();
    let source_id = sources.add("primary-id", "include::{docname}{docfilesuffix}[]");
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    for (path, directory) in [
        ("/virtual/guide.adoc", "/virtual"),
        ("C:/guide.adoc", "C:/"),
    ] {
        let provider = Provider(path);
        let result = parse_resource(&mut sources, &options, &provider).unwrap();
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert_eq!(
            result.document.blocks[0].attribute_context.get("docfile"),
            Some(path)
        );
        assert_eq!(result.document.attributes.get("docdir"), Some(directory));
        assert_eq!(
            sources
                .get(result.document.blocks[0].span.source_id)
                .unwrap()
                .name(),
            "child-id"
        );
        assert!(!tokens_resource(&mut sources, &options, &provider)
            .unwrap()
            .is_empty());
    }
}
