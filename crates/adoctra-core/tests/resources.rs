use adoctra_core::{
    parse_with_options, MemoryResources, ParseOptions, Resource, ResourceError, ResourceErrorKind,
    ResourceProvider, ResourceRequest, ResourceResult, SourceId, SourceMap, Span,
};
use std::cell::RefCell;

#[test]
fn memory_resources_resolve_relative_to_the_current_source_and_virtual_root() {
    let mut resources = MemoryResources::new();
    resources.insert("book/main.adoc", "main").unwrap();
    resources.insert("book/parts/один.adoc", "one").unwrap();
    resources.insert("shared.adoc", "shared").unwrap();
    let mut sources = SourceMap::new();
    let main = sources.load("/book/main.adoc", None, &resources).unwrap();
    let part = sources
        .load("parts/один.adoc", Some(main), &resources)
        .unwrap();
    assert_eq!(sources.get(part).unwrap().name(), "/book/parts/один.adoc");
    assert_eq!(
        sources
            .load("../main.adoc", Some(part), &resources)
            .unwrap(),
        main
    );
    let shared = sources
        .load("../../shared.adoc", Some(part), &resources)
        .unwrap();
    assert_eq!(
        sources
            .load("/shared.adoc", Some(part), &resources)
            .unwrap(),
        shared
    );
    assert_eq!(
        sources
            .load("./book//parts/../main.adoc", None, &resources)
            .unwrap(),
        main
    );
}

#[test]
fn snapshots_preserve_bom_crlf_unicode_spans_and_changed_versions() {
    let mut resources = MemoryResources::new();
    let raw = "\u{feff}Я🙂\r\ntext\n";
    resources.insert("raw.adoc", raw).unwrap();
    let mut sources = SourceMap::new();
    let old = sources.load("raw.adoc", None, &resources).unwrap();
    assert_eq!(sources.get(old).unwrap().text(), raw);
    assert_eq!(sources.slice(Span::in_source(old, 3, 5)), Some("Я"));
    assert_eq!(sources.get(old).unwrap().position(11).unwrap().line, 2);
    assert_eq!(
        resources.insert("./raw.adoc", "new").unwrap().as_deref(),
        Some(raw)
    );
    let new = sources.load("raw.adoc", None, &resources).unwrap();
    assert_ne!(old, new);
    assert_eq!(sources.get(old).unwrap().text(), raw);
    assert_eq!(sources.get(new).unwrap().text(), "new");
    assert_eq!(
        resources.remove("/raw.adoc").unwrap().as_deref(),
        Some("new")
    );
    assert!(resources.remove("raw.adoc").unwrap().is_none());
    assert_eq!(
        sources.load("raw.adoc", None, &resources).unwrap_err().kind,
        ResourceErrorKind::NotFound
    );
    assert_eq!(sources.get(new).unwrap().text(), "new");
}

#[test]
fn resource_errors_do_not_mutate_the_source_map() {
    let mut resources = MemoryResources::new();
    resources.insert("good.adoc", "text").unwrap();
    let mut sources = SourceMap::new();
    for (target, kind) in [
        ("missing.adoc", ResourceErrorKind::NotFound),
        ("../secret.adoc", ResourceErrorKind::AccessDenied),
        ("dir/../../secret.adoc", ResourceErrorKind::AccessDenied),
        ("", ResourceErrorKind::InvalidTarget),
        ("/", ResourceErrorKind::InvalidTarget),
        ("dir/", ResourceErrorKind::InvalidTarget),
        ("dir/..", ResourceErrorKind::InvalidTarget),
        ("dir/.", ResourceErrorKind::InvalidTarget),
        ("bad\0name", ResourceErrorKind::InvalidTarget),
        ("dir\\file.adoc", ResourceErrorKind::InvalidTarget),
        (
            "https://example.org/doc.adoc",
            ResourceErrorKind::InvalidTarget,
        ),
    ] {
        let error = sources.load(target, None, &resources).unwrap_err();
        assert_eq!(error.kind, kind, "{target}");
        assert!(sources.get(SourceId(0)).is_none());
    }
    assert_eq!(
        sources.load("good.adoc", None, &resources).unwrap(),
        SourceId(0)
    );
    assert_eq!(
        resources.insert("../bad.adoc", "bad").unwrap_err().kind,
        ResourceErrorKind::AccessDenied
    );
}

#[derive(Default)]
struct Host {
    requests: RefCell<Vec<(String, Option<String>)>>,
}
impl ResourceProvider for Host {
    fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
        self.requests
            .borrow_mut()
            .push((request.target.into(), request.referrer.map(str::to_owned)));
        if request.target == "denied" {
            return Err(ResourceError::new(
                ResourceErrorKind::AccessDenied,
                request.target,
                "host rejected access",
            ));
        }
        Ok(Resource {
            name: format!("host:{}", request.target),
            text: "Я <<missing>>".into(),
        })
    }
}

#[test]
fn custom_provider_controls_identity_and_receives_the_registered_referrer() {
    let host = Host::default();
    let mut sources = SourceMap::new();
    let root = sources.load("root", None, &host).unwrap();
    let child = sources.load("child", Some(root), &host).unwrap();
    assert_eq!(
        *host.requests.borrow(),
        vec![
            ("root".into(), None),
            ("child".into(), Some("host:root".into()))
        ]
    );
    assert_eq!(sources.get(child).unwrap().name(), "host:child");
    let source = sources.get(child).unwrap();
    let result = parse_with_options(
        source.text(),
        &ParseOptions {
            source_id: child,
            ..Default::default()
        },
    );
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(
        sources.slice(result.diagnostics[0].span),
        Some("<<missing>>")
    );
    assert_eq!(result.diagnostics[0].span.source_id, child);
    let before = host.requests.borrow().len();
    assert_eq!(
        sources
            .load("child", Some(SourceId(999)), &host)
            .unwrap_err()
            .kind,
        ResourceErrorKind::InvalidTarget
    );
    assert_eq!(host.requests.borrow().len(), before);
    assert_eq!(
        sources.load("denied", Some(root), &host).unwrap_err().kind,
        ResourceErrorKind::AccessDenied
    );
    assert!(sources.get(SourceId(2)).is_none());
}

fn utf16(little: bool, text: &str) -> Vec<u8> {
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
fn load_document_decodes_a_utf16_bom_like_ruby_prepare_source_string() {
    let mut resources = MemoryResources::new();
    let text = "= Тест\n\n*жир* a\rb\n";
    resources
        .insert_binary("/le.adoc", utf16(true, text))
        .unwrap();
    resources
        .insert_binary("/be.adoc", utf16(false, text))
        .unwrap();
    resources.insert("/bom.adoc", "\u{feff}= T\n").unwrap();
    resources
        .insert_binary("/bad.adoc", b"= T\n\nText \xff.\n".to_vec())
        .unwrap();
    // An odd byte count after a UTF-16 BOM is malformed UTF-16.
    let mut odd = utf16(true, "= T\n");
    odd.push(0);
    resources.insert_binary("/odd.adoc", odd).unwrap();
    let mut sources = SourceMap::new();
    for name in ["/le.adoc", "/be.adoc"] {
        // The text API keeps rejecting the bytes; the document API decodes
        // the whole source, BOM retained for the reader, CR kept mid-line.
        assert_eq!(
            sources.load(name, None, &resources).unwrap_err().kind,
            ResourceErrorKind::NotFound
        );
        let source_id = sources.load_document(name, None, &resources).unwrap();
        let file = sources.get(source_id).unwrap();
        assert_eq!(file.text(), format!("\u{feff}{text}"));
        let options = ParseOptions {
            source_id,
            ..Default::default()
        };
        let result = adoctra_core::parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.document.attributes.get("doctitle"), Some("Тест"));
        // Spans address the decoded UTF-8 snapshot.
        let span = result.document.blocks.last().unwrap().span;
        assert_eq!(span.source_id, source_id);
        assert_eq!(sources.slice(span), Some("*жир* a\rb"));
        let position = sources
            .get(source_id)
            .unwrap()
            .position(span.start)
            .unwrap();
        assert_eq!((position.line, position.column), (3, 1));
    }
    let bom = sources
        .load_document("/bom.adoc", None, &resources)
        .unwrap();
    assert_eq!(sources.get(bom).unwrap().text(), "\u{feff}= T\n");
    for name in ["/bad.adoc", "/odd.adoc"] {
        assert_eq!(
            sources
                .load_document(name, None, &resources)
                .unwrap_err()
                .kind,
            ResourceErrorKind::InvalidEncoding
        );
    }
    assert_eq!(
        sources
            .load_document("/missing.adoc", None, &resources)
            .unwrap_err()
            .kind,
        ResourceErrorKind::NotFound
    );

    // A text-only host (no binary support) keeps its own snapshot.
    struct TextOnly;
    impl ResourceProvider for TextOnly {
        fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
            Ok(Resource {
                name: format!("/{}", request.target),
                text: "\u{feff}hostet\n".to_owned(),
            })
        }
    }
    let id = sources.load_document("host.adoc", None, &TextOnly).unwrap();
    assert_eq!(sources.get(id).unwrap().text(), "\u{feff}hostet\n");
}
