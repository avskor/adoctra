use adoctra_core::{
    parse_resource, BlockKind, ParseOptions, Resource, ResourceError, ResourceErrorKind,
    ResourceProvider, ResourceRequest, ResourceResult, SafeMode, SourceId, SourceMap, Span,
};
use std::cell::RefCell;

#[derive(Default)]
struct Host {
    events: RefCell<Vec<String>>,
    missing: bool,
}

impl ResourceProvider for Host {
    fn enable_uri_cache(&self) {
        self.events.borrow_mut().push("enable".into());
    }

    fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
        self.events.borrow_mut().push(request.target.into());
        if self.missing {
            return Err(ResourceError::new(
                ResourceErrorKind::NotFound,
                request.target,
                "missing",
            ));
        }
        Ok(Resource {
            name: request.target.into(),
            text: "*remote*\n".into(),
        })
    }
}

#[test]
fn cache_activation_follows_uri_permission_depth_and_lazy_attributes() {
    for (source, allow, safe, depth, expected) in [
        (":cache-uri:\n\ninclude::http://host/a.adoc[]", true, SafeMode::Safe, "64", vec!["enable", "http://host/a.adoc"]),
        (":cache-uri:\n\ninclude::http://host/a.adoc[]", false, SafeMode::Safe, "64", vec![]),
        (":cache-uri:\n\ninclude::http://host/a.adoc[]", true, SafeMode::Secure, "64", vec![]),
        (":cache-uri:\n\ninclude::http://host/a.adoc[]", true, SafeMode::Safe, "0", vec![]),
        (":cache-uri:\n\ninclude::local.adoc[]", true, SafeMode::Safe, "64", vec!["local.adoc"]),
        (":cache-uri:\n:cache-uri!:\n\ninclude::http://host/a.adoc[]", true, SafeMode::Safe, "64", vec!["http://host/a.adoc"]),
        ("ifdef::absent[]\ninclude::http://host/a.adoc[]\nendif::[]", true, SafeMode::Safe, "64", vec![]),
        ("include::http://host/a.adoc[]\n\n:cache-uri:\n\ninclude::http://host/a.adoc[]\n\n:cache-uri!:\n\ninclude::http://host/a.adoc[]", true, SafeMode::Server, "64", vec!["http://host/a.adoc", "enable", "http://host/a.adoc", "http://host/a.adoc"]),
    ] {
        let host = Host::default();
        let mut sources = SourceMap::new();
        let primary = sources.add("main.adoc", source);
        let mut options = ParseOptions { source_id: primary, safe, ..Default::default() };
        options.attributes.set("max-include-depth", depth);
        if allow { options.attributes.set("allow-uri-read", ""); }
        let result = parse_resource(&mut sources, &options, &host).unwrap();
        assert!(result.diagnostics.is_empty(), "{source}: {:?}", result.diagnostics);
        assert_eq!(*host.events.borrow(), expected, "{source}");
        if expected.contains(&"http://host/a.adoc") {
            let block = &result.document.blocks[0];
            assert!(matches!(block.kind, BlockKind::Paragraph { .. }));
            assert_eq!(block.span.source_id, SourceId(1));
            assert_eq!(sources.slice(Span::in_source(SourceId(1), 0, 8)), Some("*remote*"));
        }
    }
}

#[test]
fn optional_uri_errors_keep_the_directive_cursor_and_resolved_uri() {
    for optional in ["", "opts=optional"] {
        let source = format!(":cache-uri:\n\ninclude::http://host/missing.adoc[{optional}]\n");
        let host = Host {
            missing: true,
            ..Default::default()
        };
        let mut sources = SourceMap::new();
        let primary = sources.add("main.adoc", source.clone());
        let mut options = ParseOptions {
            source_id: primary,
            ..Default::default()
        };
        options.attributes.set("allow-uri-read", "");
        let result = parse_resource(&mut sources, &options, &host).unwrap();
        assert_eq!(result.diagnostics.len(), 1);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(
            diagnostic.report.message.as_deref(),
            Some("include uri not readable: http://host/missing.adoc")
        );
        assert_eq!(diagnostic.span.source_id, primary);
        assert_eq!(
            sources
                .get(primary)
                .unwrap()
                .position(diagnostic.span.start)
                .unwrap()
                .line,
            3
        );
        assert_eq!(
            sources.slice(diagnostic.span),
            Some(source.lines().nth(2).unwrap())
        );
        assert!(sources.get(SourceId(1)).is_none());
        assert_eq!(
            *host.events.borrow(),
            ["enable", "http://host/missing.adoc"]
        );
    }
}
