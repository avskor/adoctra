//! Adapted from Asciidoctor 2.0.23 attributes_test.rb (see compatibility/upstream).
use adoctra_core::{
    parse_with_options, AttributeFormat, AttributePart, Attributes, BlockKind, Inline,
    ParseOptions, SafeMode, SourceId,
};

fn options(key: &str, value: &str) -> ParseOptions {
    let mut attributes = Attributes::new();
    attributes.set(key, value);
    ParseOptions {
        attributes,
        source_id: SourceId(7),
        ..Default::default()
    }
}

#[test]
fn upstream_host_priority_and_unsets_preserve_reference_spans() {
    let cases = [
        ("cash", "heroes", Some("heroes"), Some("heroes")),
        ("cash@", "heroes", Some("money"), None),
        ("cash", "heroes@", Some("money"), None),
        ("cash!", "", None, None),
        ("!cash", "", None, None),
        ("cash!@", "", Some("money"), None),
        ("!cash@", "", Some("money"), None),
        ("cash!", "@", Some("money"), None),
        ("!cash", "@", Some("money"), None),
    ];
    for (key, value, set, unset) in cases {
        for (declaration, expected) in [(":cash: money", set), (":cash!:", unset)] {
            let input = format!("{declaration}\r\n\r\nЖ {{cash}}");
            let options = options(key, value);
            let result = parse_with_options(&input, &options);
            assert!(result.diagnostics.is_empty());
            assert_eq!(
                result.document.attributes.get("cash"),
                expected,
                "{key}={value}"
            );
            let BlockKind::Paragraph { inlines } = &result.document.flat_blocks()[0].kind else {
                panic!()
            };
            let reference = inlines
                .iter()
                .find(|n| matches!(n, Inline::AttributeRef { .. }))
                .unwrap();
            let Inline::AttributeRef {
                value: resolved,
                span,
                ..
            } = reference
            else {
                panic!()
            };
            assert_eq!(resolved.as_deref(), expected);
            assert_eq!(span.source_id, SourceId(7));
            assert_eq!(&input[span.start..span.end], "{cash}");
            assert_eq!(options.attributes.get(key), Some(value));
        }
    }
}

#[test]
fn soft_defaults_survive_without_document_entries_and_allow_body_changes() {
    for (key, value, initial) in [
        ("cash@", "heroes", "heroes"),
        ("cash", "heroes@", "heroes"),
        ("cash@", "heroes@", "heroes@"),
    ] {
        let options = options(key, value);
        let result = parse_with_options(
            "{cash}\n\n:cash: money\n\n{cash}\n\n:cash!:\n\n{cash}",
            &options,
        );
        assert_eq!(result.document.attributes.get("cash"), Some(initial));
        assert_eq!(
            result.document.flat_blocks()[1]
                .attribute_context
                .get("cash"),
            Some("money")
        );
        assert_eq!(result.document.final_attributes.get("cash"), None);
        assert_eq!(options.attributes.get(key), Some(value));
    }
}

#[test]
fn soft_builtin_unsets_allow_document_to_restore_heading_ids() {
    for (key, value) in [
        ("sectids!@", ""),
        ("!sectids@", ""),
        ("sectids!", "@"),
        ("!sectids", "@"),
    ] {
        let opts = options(key, value);
        for (input, expected) in [
            ("== Heading", None),
            (":sectids:\n\n== Heading", Some("_heading")),
        ] {
            let result = parse_with_options(input, &opts);
            assert_eq!(result.document.flat_blocks()[0].id.as_deref(), expected);
        }
    }
}

#[test]
fn soft_aliases_and_semantic_parts_keep_their_meaning() {
    let result = parse_with_options(":sectnums!:\n\n== Heading", &options("numbered@", ""));
    assert_eq!(result.document.attributes.get("sectnums"), None);
    let mut opts = options("cash", "");
    let parts = vec![
        AttributePart::Open(AttributeFormat::Strong),
        AttributePart::Text("heroes".into()),
        AttributePart::Close(AttributeFormat::Strong),
        AttributePart::Raw("@".into()),
    ];
    opts.attributes.set_parts("cash", parts.clone());
    let doc = parse_with_options("{cash}", &opts).document;
    assert!(doc
        .attributes
        .raw_parts("cash")
        .unwrap()
        .contains(&AttributePart::Open(AttributeFormat::Strong)));
    assert_eq!(doc.attributes.get("cash"), Some("heroes"));
    assert_eq!(opts.attributes.raw_parts("cash").unwrap(), parts);
}

#[test]
fn processor_and_safe_overrides_remain_locked_but_soft_safe_defaults_do_not() {
    let mut opts = options("asciidoctor-version@", "wrong");
    opts.attributes.set("safe-mode-level@", "0");
    opts.attributes.set("docdir@", "/secret");
    opts.attributes.set("icons@", "font");
    opts.safe = SafeMode::Secure;
    let doc = parse_with_options(
        ":asciidoctor-version: wrong\n:safe-mode-level: 0\n:docdir: /other\n:icons: image\n\nx",
        &opts,
    )
    .document;
    assert_eq!(doc.attributes.get("asciidoctor-version"), Some("2.0.23"));
    assert_eq!(doc.attributes.get("safe-mode-level"), Some("20"));
    assert_eq!(doc.attributes.get("docdir"), Some(""));
    assert_eq!(doc.attributes.get("icons"), Some(""));
    let mut opts = options("icons!@", "");
    opts.safe = SafeMode::Secure;
    assert_eq!(
        parse_with_options(":icons: font\n\nx", &opts)
            .document
            .attributes
            .get("icons"),
        None
    );
}

#[test]
fn embedded_title_defaults_and_their_inverse_follow_soft_locks() {
    for (key, value, declaration, shown) in [
        ("showtitle", "@", "", true),
        ("showtitle@", "", ":notitle:", false),
        ("showtitle!@", "", "", false),
        ("showtitle!@", "", ":showtitle:", true),
        ("notitle!@", "", "", true),
        ("notitle!@", "", ":notitle:", false),
        ("showtitle!", "", ":showtitle:", false),
    ] {
        let mut opts = options(key, value);
        opts.embedded = true;
        let doc = parse_with_options(&format!("= Title\n{declaration}\n\nText"), &opts).document;
        assert_eq!(
            doc.attributes.get("notitle").is_none(),
            shown,
            "{key}={value}, {declaration}"
        );
    }
}
