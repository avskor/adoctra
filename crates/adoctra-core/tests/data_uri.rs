//! `data-uri`, like Ruby's `image_uri`: below secure mode image sources are
//! embedded as base64 data URIs read through the resource provider.

use adoctra_core::visit::{walk_document_mut, walk_inline_mut, VisitorMut};
use adoctra_core::{
    parse, parse_resource, AttributePart, BinaryResource, Block, BlockKind, Inline,
    MemoryResources, ParseOptions, ParseResult, Resource, ResourceError, ResourceErrorKind,
    ResourceProvider, ResourceRequest, ResourceResult, SafeMode, SourceMap,
};

/// Every image source in document order: block images, inline images and
/// icons (also inside substituted parts), admonition and callout icons.
fn sources(result: &ParseResult) -> Vec<String> {
    struct Sources(Vec<String>);
    impl Sources {
        fn parts(&mut self, parts: &[AttributePart]) {
            for part in parts {
                match part {
                    AttributePart::Image(image) => self.0.push(image.src.clone()),
                    AttributePart::Callout(callout) => self.0.push(callout.src.clone()),
                    _ => {}
                }
            }
        }
    }
    impl VisitorMut for Sources {
        fn enter_block(&mut self, block: &mut Block) -> bool {
            match &block.kind {
                BlockKind::Image { src, .. } | BlockKind::Admonition { src, .. } => {
                    self.0.push(src.clone())
                }
                BlockKind::Listing { callouts, .. } => {
                    self.0.extend(callouts.iter().map(|c| c.src.clone()))
                }
                BlockKind::CalloutList { items } => {
                    self.0.extend(items.iter().map(|i| i.src.clone()))
                }
                _ => {}
            }
            true
        }
        fn visit_inline(&mut self, inline: &mut Inline) {
            match inline {
                Inline::Image { image, .. } => self.0.push(image.src.clone()),
                Inline::Substituted { parts, .. } => self.parts(parts),
                Inline::AttributeRef { parts, .. } => self.parts(parts),
                _ => {}
            }
            walk_inline_mut(self, inline);
        }
    }
    let mut collect = Sources(Vec::new());
    walk_document_mut(&mut collect, &mut result.document.clone());
    collect.0
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nxyz";
const PNG_DATA: &str = "data:image/png;base64,iVBORw0KGgp4eXo=";

#[test]
fn cache_activation_precedes_permitted_asset_reads_and_preserves_spans() {
    #[derive(Default)]
    struct Host(std::cell::RefCell<Vec<String>>);
    impl ResourceProvider for Host {
        fn enable_uri_cache(&self) {
            self.0.borrow_mut().push("enable".into());
        }
        fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
            Err(ResourceError::new(
                ResourceErrorKind::NotFound,
                request.target,
                "missing",
            ))
        }
        fn load_binary(&self, request: ResourceRequest<'_>) -> ResourceResult<BinaryResource> {
            self.0.borrow_mut().push(request.target.into());
            Ok(BinaryResource {
                name: request.target.into(),
                bytes: b"<svg/>".to_vec(),
                content_type: Some("image/svg+xml".into()),
            })
        }
    }
    for svg in [false, true] {
        for (safe, allow, data_uri, target, expected) in [
            (
                SafeMode::Safe,
                true,
                true,
                "https://host/a.svg",
                vec!["enable", "https://host/a.svg"],
            ),
            (SafeMode::Safe, false, true, "https://host/a.svg", vec![]),
            (SafeMode::Secure, true, true, "https://host/a.svg", vec![]),
            (
                SafeMode::Safe,
                true,
                false,
                "https://host/a.svg",
                if svg {
                    vec!["enable", "https://host/a.svg"]
                } else {
                    vec![]
                },
            ),
            (SafeMode::Safe, true, true, "local.svg", vec!["local.svg"]),
        ] {
            let source = format!(
                ":cache-uri:\n{}\nimage::{target}[Pic{}]\n",
                if data_uri { ":data-uri:\n" } else { "" },
                if svg { ",opts=inline" } else { "" }
            );
            let mut map = SourceMap::new();
            let source_id = map.add("main.adoc", source.clone());
            let host = Host::default();
            let mut options = ParseOptions {
                source_id,
                safe,
                ..Default::default()
            };
            if allow {
                options.attributes.set("allow-uri-read", "");
            }
            let result = parse_resource(&mut map, &options, &host).unwrap();
            assert_eq!(*host.0.borrow(), expected, "{source} {safe:?} {allow}");
            let block = &result.document.blocks[0];
            assert_eq!(map.slice(block.span), Some(source.lines().last().unwrap()));
            if svg && !allow && safe != SafeMode::Secure {
                let diagnostic = &result.diagnostics[0];
                assert_eq!(diagnostic.span, block.span);
                assert_eq!(
                    diagnostic.report.location,
                    adoctra_core::DiagnosticLocation::Unlocated
                );
            } else {
                assert!(result.diagnostics.is_empty());
            }
        }
    }
}

fn resources(root: &str) -> (SourceMap, MemoryResources, ParseOptions) {
    let mut resources = MemoryResources::new();
    resources.insert("/docs/main.adoc", root).unwrap();
    for name in [
        "/docs/a.png",
        "/docs/img/b.png",
        "/docs/img/icons/note.png",
        "/docs/img/icons/callouts/1.png",
        "/docs/i/x.png",
    ] {
        resources.insert_binary(name, PNG).unwrap();
    }
    resources
        .insert_binary("/docs/s.svg", b"<svg/>".to_vec())
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    (sources, resources, options)
}

fn convert(root: &str) -> ParseResult {
    let (mut sources, resources, options) = resources(root);
    parse_resource(&mut sources, &options, &resources).unwrap()
}

#[test]
fn every_image_source_embeds_its_data() {
    let result = convert(
        ":data-uri:\n:icons:\n:iconsdir: img/icons\n\nimage::a.png[]\n\n\
         :imagesdir: img\n\nimage:b.png[] icon:note[]\n\nNOTE: n\n\n\
         [subs=+quotes]\n----\nx <1>\n----\n<1> one\n\n----\ny <1>\n----\n<1> two\n\n\
         :x: image:../i/x.png[]\n\n{x}",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(sources(&result), vec![PNG_DATA; 9]);
}

#[test]
fn types_follow_the_extension_and_unreadable_images_embed_nothing() {
    let result = convert(":data-uri:\n\nimage:s.svg[] image:gone.jpg[] image:a[]");
    assert_eq!(
        sources(&result),
        [
            "data:image/svg+xml;base64,PHN2Zy8+",
            "data:image/jpg;base64,",
            "data:application/octet-stream;base64,",
        ]
    );
    let messages: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "image to embed not found or not readable: /docs/gone.jpg",
            "image to embed not found or not readable: /docs/a",
        ]
    );
    // Warnings point at the physical macros.
    let source = ":data-uri:\n\nimage:s.svg[] image:gone.jpg[] image:a[]";
    let spans: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| &source[d.span.start..d.span.end])
        .collect();
    assert_eq!(spans, ["image:gone.jpg[]", "image:a[]"]);
}

#[test]
fn the_attribute_applies_where_images_convert() {
    let result =
        convert("image::a.png[]\n\n:data-uri:\n\nimage::a.png[]\n\n:data-uri!:\n\nimage:a.png[]");
    assert_eq!(sources(&result), ["a.png", PNG_DATA, "a.png"]);
}

#[test]
fn secure_mode_and_uris_without_allow_uri_read_keep_web_paths() {
    let (mut sources_map, resources, options) =
        resources(":data-uri:\n\nimage::a.png[]\n\nimage::https://x.org/a b.png[]");
    let secure = ParseOptions {
        safe: SafeMode::Secure,
        ..options.clone()
    };
    let result = parse_resource(&mut sources_map, &secure, &resources).unwrap();
    assert_eq!(sources(&result), ["a.png", "https://x.org/a%20b.png"]);
    let result = parse_resource(&mut sources_map, &options, &resources).unwrap();
    assert_eq!(sources(&result), [PNG_DATA, "https://x.org/a%20b.png"]);
}

#[test]
fn string_parses_have_no_provider_and_warn() {
    let result = parse(":data-uri:\n\nimage::a.png[]");
    assert_eq!(sources(&result), ["data:image/png;base64,"]);
    assert_eq!(
        result.diagnostics[0].message,
        "image to embed not found or not readable: a.png"
    );
}

/// A provider for remote images with a reported content type.
struct Remote;
impl ResourceProvider for Remote {
    fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
        match request.target {
            "main.adoc" => Ok(Resource {
                name: "main.adoc".into(),
                text: ":data-uri:\n:imagesdir: https://x.org/i\n\n\
                       image::ok.ico[]\n\nimage::https://x.org/gone.png[]"
                    .into(),
            }),
            target => Err(ResourceError::new(ResourceErrorKind::NotFound, target, "")),
        }
    }
    fn load_binary(&self, request: ResourceRequest<'_>) -> ResourceResult<BinaryResource> {
        match request.target {
            "https://x.org/i/ok.ico" => Ok(BinaryResource {
                name: request.target.into(),
                bytes: b"foo".to_vec(),
                content_type: Some("image/x-icon".into()),
            }),
            target => Err(ResourceError::new(ResourceErrorKind::NotFound, target, "")),
        }
    }
}

#[test]
fn uris_embed_with_allow_uri_read_and_keep_the_uri_on_failure() {
    let mut sources_map = SourceMap::new();
    let source_id = sources_map.load("main.adoc", None, &Remote).unwrap();
    // `allow-uri-read` is the host's.
    let mut attributes = adoctra_core::Attributes::default();
    attributes.set("allow-uri-read", "");
    let options = ParseOptions {
        source_id,
        attributes,
        ..Default::default()
    };
    let result = parse_resource(&mut sources_map, &options, &Remote).unwrap();
    assert_eq!(
        sources(&result),
        ["data:image/x-icon;base64,Zm9v", "https://x.org/gone.png"]
    );
    assert_eq!(
        result.diagnostics[0].message,
        "could not retrieve image data from URI: https://x.org/gone.png"
    );
}

#[test]
fn memory_resources_load_binary_and_text_buffers() {
    let (_, mut resources, _) = resources("x");
    let request = |target| ResourceRequest {
        target,
        referrer: Some("/docs/main.adoc"),
    };
    assert_eq!(resources.load_binary(request("a.png")).unwrap().bytes, PNG);
    assert_eq!(
        resources.load_binary(request("main.adoc")).unwrap().bytes,
        b"x"
    );
    // Binary buffers are not text resources.
    assert_eq!(
        resources.load(request("a.png")).unwrap_err().kind,
        ResourceErrorKind::NotFound
    );
    assert_eq!(
        resources.remove_binary("/docs/a.png").unwrap().as_deref(),
        Some(PNG)
    );
    assert_eq!(
        resources.load_binary(request("a.png")).unwrap_err().kind,
        ResourceErrorKind::NotFound
    );
}

#[test]
fn providers_without_binary_support_report_it() {
    let error = Remote
        .load_binary(ResourceRequest {
            target: "x",
            referrer: None,
        })
        .unwrap_err();
    assert_eq!(error.kind, ResourceErrorKind::NotFound);
    struct Text;
    impl ResourceProvider for Text {
        fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
            Err(ResourceError::new(
                ResourceErrorKind::NotFound,
                request.target,
                "",
            ))
        }
    }
    let error = Text
        .load_binary(ResourceRequest {
            target: "x",
            referrer: None,
        })
        .unwrap_err();
    assert_eq!(error.kind, ResourceErrorKind::Unsupported);
}
