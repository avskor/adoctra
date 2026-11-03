//! `ParseOptions.base_dir`, Ruby's `:base_dir` (the CLI's `-B`): include
//! and image targets of the primary document resolve from it (jailed there
//! from safe mode up), while the provider still reads them relative to the
//! file that refers to them.

use adoctra_core::{
    parse_resource, BlockKind, Inline, MemoryResources, ParseOptions, ParseResult, SafeMode,
    SourceMap,
};

fn run(base_dir: Option<&str>, main: &str) -> ParseResult {
    let mut resources = MemoryResources::new();
    resources.insert("/r/docs/main.adoc", main).unwrap();
    resources.insert("/r/outside.adoc", "outside text").unwrap();
    resources
        .insert("/r/docs/local.adoc", "docs local")
        .unwrap();
    resources.insert("/r/local.adoc", "root local").unwrap();
    resources
        .insert_binary("/r/pic.png", b"\x89PNG\r\n\x1a\nroot".to_vec())
        .unwrap();
    resources
        .insert_binary("/r/docs/pic.png", b"\x89PNG\r\n\x1a\ndocs".to_vec())
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/r/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        safe: SafeMode::Safe,
        base_dir: base_dir.map(str::to_owned),
        ..Default::default()
    };
    parse_resource(&mut sources, &options, &resources).unwrap()
}

fn texts(result: &ParseResult) -> Vec<String> {
    result
        .document
        .blocks
        .iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => Some(
                inlines
                    .iter()
                    .filter_map(|inline| match inline {
                        Inline::Text { value, .. } => Some(value.as_str()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

fn messages(result: &ParseResult) -> Vec<String> {
    result
        .diagnostics
        .iter()
        .map(|d| {
            d.report
                .message
                .clone()
                .unwrap_or_else(|| d.message.clone())
        })
        .collect()
}

#[test]
fn includes_resolve_from_the_base_directory() {
    let source = "include::local.adoc[]\n\ninclude::../outside.adoc[]\n";
    // Without one, the primary file's directory (docdir) is the base:
    // `..` leaves the jail and recovers inside it.
    let default = run(None, source);
    assert_eq!(texts(&default)[0], "docs local");
    assert!(messages(&default).contains(&"include file not found: /r/docs/outside.adoc".to_owned()));
    // `-B /r`: Ruby starts the primary document's includes there.
    let based = run(Some("/r"), source);
    assert_eq!(texts(&based), ["root local", "outside text"]);
    assert_eq!(
        messages(&based),
        ["include file has illegal reference to ancestor of jail; recovering automatically"]
    );
}

#[test]
fn images_resolve_from_the_base_directory() {
    let source = ":data-uri:\n\nimage::pic.png[]\n";
    let src = |result: &ParseResult| match &result.document.blocks[0].kind {
        BlockKind::Image { src, .. } => src.clone(),
        other => panic!("{other:?}"),
    };
    // Base64 of "\x89PNG\r\n\x1a\n" plus "docs"/"root".
    assert_eq!(
        src(&run(None, source)),
        "data:image/png;base64,iVBORw0KGgpkb2Nz"
    );
    assert_eq!(
        src(&run(Some("/r"), source)),
        "data:image/png;base64,iVBORw0KGgpyb290"
    );
}

/// Ruby's sub_macros logs an unknown reference as an unlocated INFO only
/// when its logger is at INFO level; the editor keeps the located warning.
#[test]
fn unresolved_references_report_ruby_info_for_verbose_logs() {
    let result = adoctra_core::parse("See <<nope>>.\n");
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.severity, adoctra_core::Severity::Warning);
    assert_eq!(diagnostic.message, "unresolved cross reference: 'nope'");
    assert_eq!(diagnostic.span, adoctra_core::Span::new(4, 12));
    let report = &diagnostic.report;
    assert!(report.verbose_only && !report.suppressed);
    assert_eq!(report.severity, Some(adoctra_core::Severity::Info));
    assert_eq!(
        report.message.as_deref(),
        Some("possible invalid reference: nope")
    );
    assert_eq!(report.location, adoctra_core::DiagnosticLocation::Unlocated);
}

fn run_in(base_dir: Option<&str>, safe: SafeMode, main: &str) -> ParseResult {
    let mut resources = MemoryResources::new();
    resources.insert("/r/docs/main.adoc", main).unwrap();
    resources
        .insert("/r/docs/sub.adoc", "include::missing.adoc[]\n")
        .unwrap();
    resources.insert("/r/local.adoc", "root local").unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/r/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        safe,
        base_dir: base_dir.map(str::to_owned),
        ..Default::default()
    };
    parse_resource(&mut sources, &options, &resources).unwrap()
}

fn attribute(result: &ParseResult, name: &str) -> Option<String> {
    result.document.attributes.get(name).map(str::to_owned)
}

#[test]
fn the_base_directory_replaces_docdir_even_outside_the_document() {
    // Ruby's Document#initialize: `attr_overrides['docdir'] = base_dir`; the
    // directory need not contain the document or exist at all.
    let source = "include::local.adoc[]\n";
    let result = run_in(Some("/r"), SafeMode::Unsafe, source);
    assert_eq!(attribute(&result, "docdir").as_deref(), Some("/r"));
    assert_eq!(
        attribute(&result, "docfile").as_deref(),
        Some("/r/docs/main.adoc")
    );
    assert_eq!(texts(&result), ["root local"]);
    let result = run_in(Some("/elsewhere"), SafeMode::Safe, source);
    assert_eq!(attribute(&result, "docdir").as_deref(), Some("/elsewhere"));
    assert!(messages(&result).contains(&"include file not found: /elsewhere/local.adoc".to_owned()));
    let result = run_in(None, SafeMode::Unsafe, source);
    assert_eq!(attribute(&result, "docdir").as_deref(), Some("/r/docs"));
}

#[test]
fn server_mode_trims_docfile_by_the_length_of_docdir_like_ruby() {
    // `docfile[(docdir.length + 1)..-1]` whether or not docfile is inside;
    // the cursor (`Unresolved directive in`) is its basename.
    let source = "include::nope.adoc[]\n";
    for (base_dir, docfile, path) in [
        (None, Some("main.adoc"), "main.adoc"),
        (Some("/r"), Some("docs/main.adoc"), "main.adoc"),
        (Some("/q/z"), Some("cs/main.adoc"), "main.adoc"),
        (Some("/abcdefghijk"), Some("adoc"), "adoc"),
        // Longer than docfile: Ruby's nil unsets it; the cursor is `<stdin>`.
        (Some("/a-very-long-base-directory"), None, "<stdin>"),
    ] {
        let result = run_in(base_dir, SafeMode::Server, source);
        assert_eq!(
            attribute(&result, "docfile").as_deref(),
            docfile,
            "{base_dir:?}"
        );
        assert_eq!(attribute(&result, "docdir").as_deref(), Some(""));
        assert_eq!(
            texts(&result),
            [format!(
                "Unresolved directive in {path} - include::nope.adoc[]"
            )],
            "{base_dir:?}"
        );
    }
}

#[test]
fn unresolved_include_paths_are_relative_to_the_base_directory() {
    // Ruby's `relpath = relative_path inc_path, doc.base_dir`.
    for (base_dir, target, expected) in [
        (None, "sub.adoc", "sub.adoc"),
        (Some("/r"), "docs/sub.adoc", "docs/sub.adoc"),
        (Some("/r/docs/deeper"), "../sub.adoc", "../sub.adoc"),
    ] {
        let result = run_in(
            base_dir,
            SafeMode::Unsafe,
            &format!("include::{target}[]\n"),
        );
        assert_eq!(
            texts(&result),
            [format!(
                "Unresolved directive in {expected} - include::missing.adoc[]"
            )],
            "{base_dir:?}"
        );
    }
}
