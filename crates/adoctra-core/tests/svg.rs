//! SVG embedding, like Ruby's `convert_image`/`convert_inline_image` below
//! secure mode: `%inline` reads the file through the resource provider
//! (`read_svg_contents`), `%interactive` takes a fallback source.
use adoctra_core::visit::{walk_block_mut, walk_document_mut, walk_inline_mut, VisitorMut};
use adoctra_core::{
    inline_svg, parse_resource, Attributes, Block, BlockKind, Inline, MemoryResources,
    ParseOptions, ParseResult, SafeMode, SourceMap, Svg,
};

const SVG: &[u8] =
    b"\xef\xbb\xbf<?xml version=\"1.0\"?>\r\n<svg width=\"10\" style='a'>  \r\n <g/>\t\n</svg>\n";
const CONTENTS: &str = "<?xml version=\"1.0\"?>\n<svg width=\"10\" style='a'>\n <g/>\n</svg>";

#[test]
fn svg_assets_decode_bom_strictly_without_creating_source_snapshots() {
    let source = "image::encoded.svg[Pic,opts=inline]\n";
    let mut resources = MemoryResources::new();
    let mut map = SourceMap::new();
    let id = map.add("/main.adoc", source);
    let options = ParseOptions {
        source_id: id,
        ..Default::default()
    };
    let bytes: Vec<u8> = std::iter::once(0xfeffu16)
        .chain("<svg>Привет</svg>\r\n".encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    resources.insert_binary("/encoded.svg", bytes).unwrap();
    let good = parse_resource(&mut map, &options, &resources).unwrap();
    assert_eq!(
        images(&good)[0].1,
        Some(Svg::Inline(Some("<svg>Привет</svg>".into())))
    );
    assert!(good.diagnostics.is_empty());
    resources
        .insert_binary("/encoded.svg", b"<svg>\xff</svg>".to_vec())
        .unwrap();
    let bad = parse_resource(&mut map, &options, &resources).unwrap();
    assert_eq!(images(&bad)[0].1, Some(Svg::Inline(None)));
    assert_eq!(
        bad.diagnostics[0].message,
        // Ruby's read_asset names the document (`docfile`, else `<stdin>`).
        "/main.adoc: SVG does not exist or cannot be read: /encoded.svg"
    );
    assert_eq!(map.slice(bad.diagnostics[0].span), Some(source.trim_end()));
    assert!(map.get(adoctra_core::SourceId(1)).is_none());
}

fn convert(root: &str, options: ParseOptions) -> ParseResult {
    let mut resources = MemoryResources::new();
    resources.insert("/docs/main.adoc", root).unwrap();
    resources
        .insert_binary("/docs/a.svg", SVG.to_vec())
        .unwrap();
    resources
        .insert_binary("/docs/img/b.svg", SVG.to_vec())
        .unwrap();
    resources
        .insert_binary("/docs/empty.svg", Vec::new())
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..options
    };
    parse_resource(&mut sources, &options, &resources).unwrap()
}

/// Every image's source and SVG embedding, in document order.
fn images(result: &ParseResult) -> Vec<(String, Option<Svg>)> {
    struct Images(Vec<(String, Option<Svg>)>);
    impl VisitorMut for Images {
        fn visit_block(&mut self, block: &mut Block) {
            if let BlockKind::Image { src, svg, .. } = &block.kind {
                self.0.push((src.clone(), svg.clone()));
            }
            walk_block_mut(self, block);
        }
        fn visit_inline(&mut self, inline: &mut Inline) {
            if let Inline::Image { image, .. } = inline {
                self.0.push((image.src.clone(), image.svg.clone()));
            }
            walk_inline_mut(self, inline);
        }
    }
    let mut collect = Images(Vec::new());
    walk_document_mut(&mut collect, &mut result.document.clone());
    collect.0
}

fn messages(result: &ParseResult) -> Vec<&str> {
    result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect()
}

#[test]
fn inline_svgs_read_their_normalized_contents() {
    let result = convert(
        "image::a.svg[opts=inline]\n\n:imagesdir: img\n\nText image:b.svg[x,opts=inline] t",
        ParseOptions::default(),
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    // The BOM is removed and every line right-stripped, like Ruby's
    // `prepare_source_string`; the source is not resolved.
    assert_eq!(
        images(&result),
        [
            ("a.svg".into(), Some(Svg::Inline(Some(CONTENTS.into())))),
            ("img/b.svg".into(), Some(Svg::Inline(Some(CONTENTS.into())))),
        ]
    );
}

#[test]
fn unreadable_inline_svgs_show_their_alt_and_warn() {
    let result = convert(
        "image::empty.svg[opts=inline]\n\nimage::gone.svg[opts=inline]\n\nimage::http://e/a.svg[opts=inline]",
        ParseOptions::default(),
    );
    assert!(images(&result)
        .iter()
        .all(|(_, svg)| svg == &Some(Svg::Inline(None))));
    assert_eq!(
        messages(&result),
        [
            "contents of SVG is empty: /docs/empty.svg",
            "/docs/main.adoc: SVG does not exist or cannot be read: /docs/gone.svg",
            "cannot retrieve contents of SVG at URI: http://e/a.svg (allow-uri-read attribute not enabled)",
        ]
    );
}

#[test]
fn interactive_svgs_take_a_fallback_source() {
    let result = convert(
        "image::a.svg[opts=interactive]\n\n:imagesdir: img\n\nimage::b.svg[A,opts=interactive,fallback=f.png]\n\nimage::x[format=svg,opts=interactive]",
        ParseOptions::default(),
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        images(&result),
        [
            ("a.svg".into(), Some(Svg::Interactive { fallback: None })),
            (
                "img/b.svg".into(),
                Some(Svg::Interactive {
                    fallback: Some("img/f.png".into())
                })
            ),
            ("img/x".into(), Some(Svg::Interactive { fallback: None })),
        ]
    );
    // With `data-uri` the object embeds the data; an inline SVG is not read
    // as an image.
    let result = convert(
        ":data-uri:\n\nimage::a.svg[opts=interactive]\n\nimage::empty.svg[opts=inline]",
        ParseOptions::default(),
    );
    let images = images(&result);
    assert!(images[0].0.starts_with("data:image/svg+xml;base64,77u/"));
    assert_eq!(images[1].0, "empty.svg");
    assert_eq!(
        messages(&result),
        ["contents of SVG is empty: /docs/empty.svg"]
    );
}

#[test]
fn other_images_and_secure_mode_keep_an_img() {
    let source = "image::a.png[opts=inline]\n\nimage::a.svg[]\n\nimage::a.svg[opts=inline]";
    let result = convert(source, ParseOptions::default());
    let svgs: Vec<_> = images(&result).into_iter().map(|(_, svg)| svg).collect();
    assert_eq!(svgs[..2], [None, None]);
    assert!(svgs[2].is_some());
    let result = convert(
        source,
        ParseOptions {
            safe: SafeMode::Secure,
            attributes: Attributes::new(),
            ..Default::default()
        },
    );
    assert!(images(&result).iter().all(|(_, svg)| svg.is_none()));
}

#[test]
fn inline_svg_markup_follows_ruby() {
    let svg = "<?xml?>\n<svg width=\"1\" height='2' style=\"s\" viewBox=\"0 0 1 1\">\n<g/></svg>";
    assert_eq!(
        inline_svg(svg, None, None),
        "<svg width=\"1\" height='2' style=\"s\" viewBox=\"0 0 1 1\">\n<g/></svg>"
    );
    assert_eq!(
        inline_svg(svg, Some("5"), Some("7")),
        "<svg viewBox=\"0 0 1 1\" width=\"5\" height=\"7\">\n<g/></svg>"
    );
    assert_eq!(
        inline_svg("<svg>x</svg>", None, Some("3")),
        "<svg height=\"3\">x</svg>"
    );
    // No start tag at the start: unchanged.
    assert_eq!(inline_svg("<svgx/>", Some("3"), None), "<svgx/>");
}
