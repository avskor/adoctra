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
