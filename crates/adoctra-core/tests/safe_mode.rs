//! Ruby's safe mode levels and `allow-uri-read`.
use adoctra_core::{
    parse_resource, parse_with_options, Attributes, BlockKind, Inline, MemoryResources,
    ParseOptions, Resource, ResourceErrorKind, ResourceProvider, ResourceRequest, ResourceResult,
    SafeMode, SourceMap,
};

fn options(safe: SafeMode) -> ParseOptions {
    ParseOptions {
        safe,
        ..Default::default()
    }
}

fn attrs(source: &str, safe: SafeMode) -> Attributes {
    parse_with_options(source, &options(safe))
        .document
        .attributes
        .clone()
}

#[test]
fn levels_set_ruby_attributes_and_locks() {
    assert_eq!(SafeMode::default(), SafeMode::Safe);
    assert_eq!(SafeMode::from_name("SECURE"), Some(SafeMode::Secure));
    assert_eq!(SafeMode::from_name("paranoid"), None);
    for (mode, name, level) in [
        (SafeMode::Unsafe, "unsafe", "0"),
        (SafeMode::Safe, "safe", "1"),
        (SafeMode::Server, "server", "10"),
        (SafeMode::Secure, "secure", "20"),
    ] {
        let a = attrs(
            ":safe-mode-name: x\n:safe-mode-level: 99\n:allow-uri-read:\n\nx",
            mode,
        );
        assert_eq!(a.get("safe-mode-name"), Some(name));
        assert_eq!(a.get("safe-mode-level"), Some(level));
        assert_eq!(a.get(&format!("safe-mode-{name}")), Some(""));
        assert_eq!(a.get("allow-uri-read"), None, "{name}");
        assert_eq!(SafeMode::of(&a), mode);
    }
    let document = ":icons: font\n:linkcss!:\n:copycss: x\n:backend: html5\n:docdir: /x\n\nx";
    let safe = attrs(document, SafeMode::Safe);
    assert_eq!(
        (
            safe.get("icons"),
            safe.get("linkcss"),
            safe.get("copycss"),
            safe.get("docdir")
        ),
        (Some("font"), None, Some("x"), Some("/x"))
    );
    let server = attrs(document, SafeMode::Server);
    assert_eq!(
        (
            server.get("copycss"),
            server.get("docdir"),
            server.get("user-home"),
            server.get("icons")
        ),
        (None, Some(""), Some("."), Some("font"))
    );
    let secure = attrs(document, SafeMode::Secure);
    assert_eq!(
        (
            secure.get("icons"),
            secure.get("linkcss"),
            secure.get("max-attribute-value-size")
        ),
        (None, Some(""), Some("4096"))
    );
    // Host values win where Ruby uses `||=`/`key?`.
    let mut host = Attributes::new();
    host.set("icons", "font");
    host.set("allow-uri-read", "");
    host.set("user-home", "/home/me");
    host.set("max-attribute-value-size", "5");
    let a = parse_with_options(
        ":short: abcdefgh\n\n{short}",
        &ParseOptions {
            attributes: host,
            ..options(SafeMode::Secure)
        },
    )
    .document
    .attributes
    .clone();
    assert_eq!(
        (
            a.get("icons"),
            a.get("allow-uri-read"),
            a.get("user-home"),
            a.get("short")
        ),
        (Some("font"), Some(""), Some("/home/me"), Some("abcde"))
    );
}

#[test]
fn secure_mode_limits_attribute_values_at_a_character_boundary() {
    let long = format!(":long: {}tail\n\nx", "é".repeat(2049));
    let a = attrs(&long, SafeMode::Secure);
    assert_eq!(a.get("long").unwrap().len(), 4096);
    assert!(a.get("long").unwrap().chars().all(|c| c == 'é'));
    assert!(attrs(&long, SafeMode::Server)
        .get("long")
        .unwrap()
        .ends_with("tail"));
    // A cut inside a character keeps the whole characters before it.
    let mut host = Attributes::new();
    host.set("max-attribute-value-size", "3");
    let a = parse_with_options(
        ":v: éé\n\nx",
        &ParseOptions {
            attributes: host,
            ..options(SafeMode::Unsafe)
        },
    )
    .document
    .attributes
    .clone();
    assert_eq!(a.get("v"), Some("é"));
}

#[test]
fn server_mode_hides_the_document_directory() {
    let mut resources = MemoryResources::new();
    resources
        .insert("/docs/guide.adoc", "{docdir}|{docfile}")
        .unwrap();
    for (mode, expected) in [
        (SafeMode::Safe, "/docs|/docs/guide.adoc"),
        (SafeMode::Server, "|guide.adoc"),
    ] {
        let mut sources = SourceMap::new();
        let source_id = sources.load("/docs/guide.adoc", None, &resources).unwrap();
        let result = parse_resource(
            &mut sources,
            &ParseOptions {
                source_id,
                ..options(mode)
            },
            &resources,
        )
        .unwrap();
        let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
            panic!()
        };
        let text: String = inlines
            .iter()
            .map(|inline| match inline {
                Inline::Text { value, .. } => value.clone(),
                Inline::AttributeRef { value, .. } => value.clone().unwrap_or_default(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(text, expected, "{mode:?}");
    }
}

/// A provider that serves URI names too.
struct Remote(MemoryResources);

impl ResourceProvider for Remote {
    fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
        if request.target.starts_with("https://") {
            return match request.target {
                "https://x.org/a.adoc" => Ok(Resource {
                    name: request.target.into(),
                    text: "Remote".into(),
                }),
                _ => Err(adoctra_core::ResourceError::new(
                    ResourceErrorKind::NotFound,
                    request.target,
                    "missing",
                )),
            };
        }
        self.0.load(request)
    }
}

fn render(source: &str, safe: SafeMode, allow: bool) -> Vec<String> {
    let mut resources = MemoryResources::new();
    resources.insert("/d/main.adoc", source).unwrap();
    resources.insert("/d/part.adoc", "Part").unwrap();
    let provider = Remote(resources);
    let mut sources = SourceMap::new();
    let source_id = sources.load("/d/main.adoc", None, &provider).unwrap();
    let mut attributes = Attributes::new();
    if allow {
        attributes.set("allow-uri-read", "");
    }
    let result = parse_resource(
        &mut sources,
        &ParseOptions {
            source_id,
            attributes,
            ..options(safe)
        },
        &provider,
    )
    .unwrap();
    result
        .document
        .blocks
        .iter()
        .map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => inlines
                .iter()
                .map(|inline| match inline {
                    Inline::Text { value, .. } => value.clone(),
                    Inline::Link { target, .. } => format!("<{target}>"),
                    Inline::Substituted { parts, .. } => parts
                        .iter()
                        .find_map(|part| match part {
                            adoctra_core::AttributePart::OpenLink(link) => {
                                Some(format!("<{}>", link.target))
                            }
                            _ => None,
                        })
                        .unwrap_or_default(),
                    Inline::AttributeRef { name, .. } => format!("{{{name}}}"),
                    other => format!("{other:?}"),
                })
                .collect(),
            BlockKind::Listing { text, .. } => text.clone(),
            other => format!("{other:?}"),
        })
        .collect()
}

#[test]
fn secure_mode_and_unreadable_uris_turn_includes_into_links() {
    let source = "include::part.adoc[]\n\ninclude::https://x.org/a.adoc[]\n\n\
                  include::{undefined}a b.adoc[]\n\n----\ninclude::part.adoc[]\n----\n";
    assert_eq!(
        render(source, SafeMode::Safe, false),
        [
            "Part",
            "<https://x.org/a.adoc>",
            "Unresolved directive in <stdin> - include::{undefined}a b.adoc[]",
            "Part"
        ]
    );
    assert_eq!(
        render(source, SafeMode::Safe, true)[..2],
        ["Part".to_owned(), "Remote".to_owned()]
    );
    // Secure mode links every include, also in verbatim content.
    let secure = render(source, SafeMode::Secure, true);
    assert_eq!(
        secure[..3],
        [
            "<part.adoc>",
            "<https://x.org/a.adoc>",
            "<{undefined}a b.adoc>"
        ]
    );
    assert_eq!(secure[3], "link:part.adoc[role=include]");
    // Without a provider, secure mode still links.
    let doc = parse_with_options("include::x.adoc[]", &options(SafeMode::Secure)).document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    assert!(
        matches!(&inlines[0], Inline::Substituted { parts, .. }
            if matches!(&parts[0], adoctra_core::AttributePart::OpenLink(link)
                if link.target == "x.adoc" && link.role.as_deref() == Some("bare include"))),
        "{inlines:?}"
    );
    let doc = parse_with_options("include::x.adoc[]", &options(SafeMode::Safe)).document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    assert!(matches!(&inlines[0], Inline::Text { value, .. } if value == "include::x.adoc[]"));
}
