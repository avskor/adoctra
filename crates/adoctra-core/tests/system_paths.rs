//! Include and image paths resolve like Ruby's `normalize_system_path`
//! against the base directory (an absolute `docdir`): the reported path,
//! jail recovery from safe mode up and its warnings, with the provider
//! still asked relative to the referring resource.
use adoctra_core::{
    parse_resource, Block, BlockKind, DiagnosticLocation, Inline, MemoryResources, ParseOptions,
    ParseResult, SafeMode, Severity, SourceMap,
};

fn parse(main: &str, files: &[(&str, &str)], safe: SafeMode) -> ParseResult {
    let mut resources = MemoryResources::new();
    resources.insert("/docs/main.adoc", main).unwrap();
    for (name, text) in files {
        resources.insert(name, *text).unwrap();
    }
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        safe,
        ..Default::default()
    };
    parse_resource(&mut sources, &options, &resources).unwrap()
}

fn texts(result: &ParseResult) -> Vec<String> {
    fn text(block: &Block) -> String {
        match &block.kind {
            BlockKind::Paragraph { inlines } => inlines
                .iter()
                .map(|inline| match inline {
                    Inline::Text { value, .. } => value.clone(),
                    other => format!("{other:?}"),
                })
                .collect(),
            other => format!("{other:?}"),
        }
    }
    result.document.blocks.iter().map(text).collect()
}

fn reports(result: &ParseResult) -> Vec<(Severity, String, bool)> {
    result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.severity,
                d.report
                    .message
                    .clone()
                    .unwrap_or_else(|| d.message.clone()),
                d.report.location == DiagnosticLocation::Unlocated,
            )
        })
        .collect()
}

#[test]
fn a_missing_include_reports_its_resolved_path() {
    // reader_test.rb: "should replace include directive that references
    // missing file with message".
    let result = parse(
        "include::sub/../none.adoc[]\n\ninclude::sub/inner.adoc[]",
        &[("/docs/sub/inner.adoc", "include::deeper.adoc[]")],
        SafeMode::Safe,
    );
    assert_eq!(
        reports(&result),
        [
            (
                Severity::Error,
                "include file not found: /docs/none.adoc".to_owned(),
                false
            ),
            // Relative to the including file's directory.
            (
                Severity::Error,
                "include file not found: /docs/sub/deeper.adoc".to_owned(),
                false
            ),
        ]
    );
    assert_eq!(
        texts(&result),
        [
            "Unresolved directive in main.adoc - include::sub/../none.adoc[]",
            "Unresolved directive in sub/inner.adoc - include::deeper.adoc[]"
        ]
    );
}

#[test]
fn the_jail_recovers_ancestor_and_outside_paths_from_safe_mode_up() {
    // reader_test.rb: "can resolve include directive with absolute path"
    // and path_resolver_test.rb's jail recovery.
    let files = [
        ("/docs/outside.adoc", "recovered"),
        ("/outside.adoc", "escaped"),
        ("/docs/etc/x.adoc", "rebased"),
        ("/etc/x.adoc", "absolute"),
    ];
    let source = "include::../outside.adoc[]\n\ninclude::/etc/x.adoc[]";
    let safe = parse(source, &files, SafeMode::Safe);
    assert_eq!(texts(&safe), ["recovered", "rebased"]);
    assert_eq!(
        reports(&safe),
        [
            (
                Severity::Warning,
                "include file has illegal reference to ancestor of jail; recovering automatically"
                    .to_owned(),
                true
            ),
            (
                Severity::Warning,
                "include file is outside of jail; recovering automatically".to_owned(),
                true
            ),
        ]
    );
    // Unsafe mode has no jail.
    let unsafe_ = parse(source, &files, SafeMode::Unsafe);
    assert_eq!(texts(&unsafe_), ["escaped", "absolute"]);
    assert!(unsafe_.diagnostics.is_empty());
}

#[test]
fn a_single_line_conditional_reprocesses_an_include() {
    // reader_test.rb: "ifdef with defined attribute processes include
    // directive in brackets"; other directives stay text, like Ruby.
    let result = parse(
        "ifdef::asciidoctor-version[include::part.adoc[]]\n\nifdef::asciidoctor-version[ifdef::x[]]",
        &[("/docs/part.adoc", "included")],
        SafeMode::Safe,
    );
    assert_eq!(texts(&result), ["included", "ifdef::x[]"]);
    assert!(result.diagnostics.is_empty());
}

#[test]
fn data_uri_images_resolve_under_the_jail() {
    // blocks_test.rb: "cleans reference to ancestor directories in
    // imagesdir before reading image if safe mode level is at least SAFE".
    let mut resources = MemoryResources::new();
    let main = ":data-uri:\n:imagesdir: ../../images\n\nimage::dot.gif[Dot]\n\nimage::none.gif[]";
    resources.insert("/docs/main.adoc", main).unwrap();
    resources
        .insert_binary("/docs/images/dot.gif", b"GIF".to_vec())
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let srcs: Vec<_> = result
        .document
        .blocks
        .iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Image { src, .. } => Some(src.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        srcs,
        ["data:image/gif;base64,R0lG", "data:image/gif;base64,"]
    );
    let warning = "image has illegal reference to ancestor of jail; recovering automatically";
    assert_eq!(
        reports(&result),
        [
            (Severity::Warning, warning.to_owned(), true),
            (Severity::Warning, warning.to_owned(), true),
            (
                Severity::Warning,
                "image to embed not found or not readable: /docs/images/none.gif".to_owned(),
                true
            ),
        ]
    );
}

#[test]
fn standard_input_registers_includes_relative_to_the_base_directory() {
    // links_test.rb: an inter-document xref to an included file is
    // internal (Ruby's relpath from base_dir), also without a docfile.
    let mut resources = MemoryResources::new();
    resources
        .insert("other.adoc", "[#ch2]\n== Chapter 2")
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.add("-", "<<other.adoc#ch2>>\n\ninclude::other.adoc[]");
    let mut options = ParseOptions {
        source_id,
        ..Default::default()
    };
    options.attributes.set("docdir", "/base");
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!("{:?}", result.document.blocks[0].kind);
    };
    let Inline::CrossReference { xref, .. } = &inlines[0] else {
        panic!("{inlines:?}");
    };
    assert_eq!(xref.target, "#ch2");
}
