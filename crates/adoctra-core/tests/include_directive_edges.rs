//! Include directive edges from the upstream test suite audit: Ruby's
//! IncludeDirectiveRx grammar, a URI target linked without any provider, and
//! `skip-front-matter` (Ruby's `skip_front_matter!` in `prepare_lines`).
use adoctra_core::{
    parse_resource, parse_with_options, Block, BlockKind, Inline, MemoryResources, ParseOptions,
    SourceMap,
};

fn options(attributes: &[(&str, &str)]) -> ParseOptions {
    let mut options = ParseOptions::default();
    for (name, value) in attributes {
        options.attributes.set(*name, *value);
    }
    options
}

fn text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { value, .. } => value.clone(),
            Inline::Substituted { parts, .. } => parts
                .iter()
                .find_map(|part| match part {
                    adoctra_core::AttributePart::OpenLink(link) => {
                        Some(format!("<link {}>", link.target))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| format!("{parts:?}")),
            other => format!("{other:?}"),
        })
        .collect()
}

fn paragraph(block: &Block) -> String {
    match &block.kind {
        BlockKind::Paragraph { inlines } => text(inlines),
        other => panic!("not a paragraph: {other:?}"),
    }
}

#[test]
fn include_targets_follow_rubys_directive_grammar() {
    // An empty target, or one starting or ending with whitespace, is no
    // directive: the line is ordinary text (a description list here).
    for source in ["include:: []", "include:: not-include[]"] {
        let result = parse_with_options(source, &ParseOptions::default());
        assert!(
            matches!(
                result.document.blocks[0].kind,
                BlockKind::DescriptionList { .. }
            ),
            "{source}: {:?}",
            result.document.blocks
        );
        assert!(result.include_directives.is_empty(), "{source}");
    }
    for source in ["include::[]", "include::not-include []"] {
        // Even secure mode (which links every directive) leaves them as text.
        let result = parse_with_options(source, &ParseOptions::default());
        assert_eq!(paragraph(&result.document.blocks[0]), source);
        assert!(result.include_directives.is_empty(), "{source}");
    }
    // A conditional target still may not contain whitespace.
    let result = parse_with_options("ifdef::a b[]\nshown", &ParseOptions::default());
    assert!(result.conditional_directives.is_empty());
}

#[test]
fn a_uri_include_ruby_may_not_read_is_linked_without_a_provider() {
    // Ruby needs no I/O to link a URI target without `allow-uri-read`.
    let safe = |attributes: &[(&str, &str)]| {
        let mut options = options(attributes);
        options.safe = adoctra_core::SafeMode::Safe;
        options
    };
    let result = parse_with_options("include::https://example.org/a.adoc[]", &safe(&[]));
    assert_eq!(
        paragraph(&result.document.blocks[0]),
        "<link https://example.org/a.adoc>"
    );
    assert_eq!(result.include_directives.len(), 1);
    // A target with spaces goes through pass:c, like Ruby.
    let result = parse_with_options("include::https://example.org/a b.adoc[]", &safe(&[]));
    assert_eq!(
        paragraph(&result.document.blocks[0]),
        "<link https://example.org/a b.adoc>"
    );
    // Not without built-in includes, not with permission to read (nothing
    // can be read here) and never for a file target.
    for (source, attributes) in [
        (
            "include::https://example.org/a.adoc[]",
            &[("max-include-depth", "0")][..],
        ),
        (
            "include::https://example.org/a.adoc[]",
            &[("allow-uri-read", "")][..],
        ),
        ("include::part.adoc[]", &[][..]),
    ] {
        let result = parse_with_options(source, &safe(attributes));
        assert_eq!(
            paragraph(&result.document.blocks[0]),
            source,
            "{attributes:?}"
        );
    }
}

#[test]
fn skip_front_matter_hides_it_and_sets_the_front_matter_attribute() {
    let source = "---\nlayout: post\ntitle: x\n---\n= Document Title\nAuthor Name\n\npreamble";
    let result = parse_with_options(source, &options(&[("skip-front-matter", "")]));
    let document = &result.document;
    assert!(document.has_title);
    assert_eq!(
        document.attributes.get("front-matter"),
        Some("layout: post\ntitle: x")
    );
    assert_eq!(document.attributes.get("author"), Some("Author Name"));
    // Physical spans are unchanged: the title starts after the front matter.
    let title = &document.blocks[0];
    assert_eq!(
        &source[title.span.start..title.span.end],
        "= Document Title"
    );
    // Without the attribute, or without a closing line, nothing is skipped.
    for (source, attributes) in [
        (source, &[][..]),
        ("---\nopen\n\npara", &[("skip-front-matter", "")][..]),
    ] {
        let result = parse_with_options(source, &options(attributes));
        assert!(result.document.attributes.get("front-matter").is_none());
        assert!(matches!(
            result.document.blocks[0].kind,
            BlockKind::ThematicBreak
        ));
    }
}

#[test]
fn skip_front_matter_applies_to_included_files() {
    let mut resources = MemoryResources::new();
    resources
        .insert(
            "/docs/main.adoc",
            ":skip-front-matter:\n\ninclude::part.adoc[]\n\nifdef::front-matter[seen]",
        )
        .unwrap();
    resources
        .insert("/docs/part.adoc", "---\nkey: value\n---\nincluded")
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let paragraphs: Vec<_> = result.document.blocks.iter().map(paragraph).collect();
    // Ruby sets the attribute while reading: a later condition sees it.
    assert_eq!(paragraphs, ["included", "seen"]);
    let included = &result.document.blocks[0];
    let part = sources.get(included.span.source_id).unwrap();
    assert_eq!(
        &part.text()[included.span.start..included.span.end],
        "included"
    );
}
