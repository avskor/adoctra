//! Inline images and icons recognized by the macros stage (Ruby's
//! `InlineImageMacroRx`) before links, over the lexical result of quotes,
//! attribute references and replacements.
use adoctra_core::{
    parse, tokens, AttributeFormat as F, AttributePart as P, Block, BlockKind, Inline, InlineImage,
    Span, TokenKind,
};

fn paragraph(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines } => inlines,
        BlockKind::Section { blocks, .. } => paragraph(blocks.last().unwrap()),
        _ => panic!("{block:?}"),
    }
}

/// The inlines of the last paragraph of a document.
fn last(source: &str) -> Vec<Inline> {
    let doc = parse(source).document;
    paragraph(doc.blocks.last().unwrap()).to_vec()
}

fn images(nodes: &[Inline]) -> Vec<(&InlineImage, Span)> {
    let mut found = Vec::new();
    for node in nodes {
        match node {
            Inline::Image { image, span } => found.push((image, *span)),
            Inline::Strong { children, .. } | Inline::Link { children, .. } => {
                found.extend(images(children))
            }
            // A source link is generated parts around its label.
            Inline::Substituted { parts, span, .. } => {
                for part in parts {
                    if let P::Image(image) = part {
                        found.push((image.as_ref(), *span));
                    }
                }
            }
            _ => {}
        }
    }
    found
}

fn alt(image: &InlineImage) -> String {
    adoctra_core::attributes::parts_text(&image.alt)
}

#[test]
fn macro_names_positionals_and_keeps_lexical_values_and_spans() {
    let source = ":imagesdir: img\n\nA image:a b.png[Alt *b* & (C),20,30,title=T <t>,role=r,float=left,link=https://x.org,window=_blank] b";
    let nodes = last(source);
    let found = images(&nodes);
    let (image, span) = found[0];
    let start = source.find("image:").unwrap();
    assert_eq!(span, Span::new(start, source.len() - 2));
    assert!(!image.icon);
    assert_eq!(
        (image.target.as_str(), image.src.as_str()),
        ("a b.png", "img/a%20b.png")
    );
    // The alt keeps the earlier stages' boundaries and replacements.
    assert_eq!(alt(image), "Alt b & \u{a9}");
    let boundaries: Vec<_> = image.alt.iter().filter(|p| p.text().is_empty()).collect();
    assert_eq!(boundaries, [&P::Open(F::Strong), &P::Close(F::Strong)]);
    let attrs = &image.attributes;
    assert_eq!(attrs["width"], "20");
    assert_eq!(attrs["height"], "30");
    // Values keep their lexical entity protection.
    assert_eq!(attrs["title"], "T &lt;t&gt;");
    assert_eq!(attrs["window"], "_blank");
    assert_eq!(image.classes(), "image left r");
    assert!(matches!(&nodes[0], Inline::Text { value, .. } if value == "A "));
    assert!(matches!(nodes.last(), Some(Inline::Text { value, .. }) if value == " b"));
}

#[test]
fn default_alt_unescaped_lists_and_non_matching_forms() {
    let nodes = last(
        "image:dir/my_pic-1.png[] image:e.png[a\\]b] image:i.png[x[1]] \\image:d.png[] \
         image: k.png[] image::nope.png[] image:m.png[multi,\nline,opts=\"a, b\"]",
    );
    let found: Vec<_> = images(&nodes).into_iter().map(|(i, _)| i).collect();
    let alts: Vec<_> = found.iter().map(|i| alt(i)).collect();
    assert_eq!(alts, ["my pic 1", "a]b", "x[1", "multi"]);
    assert_eq!(found[3].attributes["width"], "line");
    assert_eq!(found[3].options, ["a", "b"]);
    // The escape is dropped; the other forms stay text.
    let text: String = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::Text { value, .. } => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains("image:d.png[]"), "{text}");
    assert!(!text.contains("\\image"), "{text}");
    assert!(text.contains("image: k.png[] image::nope.png[]"), "{text}");
}

#[test]
fn icons_follow_the_icons_mode_iconsdir_and_icontype() {
    let nodes = last("icon:heart[2x,link=l] icon:x[alt=A]");
    let found = images(&nodes);
    assert!(found
        .iter()
        .all(|(i, _)| i.icon && i.icons.is_none() && i.src.is_empty()));
    assert_eq!(found[0].0.attributes["size"], "2x");
    assert_eq!(alt(found[1].0), "A");

    let nodes = last(":icons: font\n\nicon:heart[]");
    assert_eq!(images(&nodes)[0].0.icons.as_deref(), Some("font"));
    // `icons: svg` becomes image icons with the svg type at header end.
    let doc =
        parse(":icons: svg\n\nicon:z[] icon:y[icon=w.gif]\n\n:iconsdir: ic\n\nicon:q[icon=p]")
            .document;
    assert_eq!(doc.attributes.get("icons"), Some(""));
    assert_eq!(doc.attributes.get("icontype"), Some("svg"));
    let srcs: Vec<_> = doc
        .flat_blocks()
        .into_iter()
        .flat_map(|b| images(paragraph(b)))
        .map(|(i, _)| i.src.clone())
        .collect();
    assert_eq!(
        srcs,
        ["./images/icons/z.svg", "./images/icons/w.gif", "ic/p.svg"]
    );
}

#[test]
fn images_inside_quotes_links_and_attribute_values() {
    let nodes = last(
        ":img: image:v.png[From *attr*]\n\n*image:s.png[S]* https://x.org[image:g.png[G]] {img}",
    );
    let found = images(&nodes);
    assert_eq!(
        found.iter().map(|(i, _)| alt(i)).collect::<Vec<_>>(),
        ["S", "G"]
    );
    // An image generated wholly by an attribute value stays a part of it.
    let parts = nodes
        .iter()
        .find_map(|n| match n {
            Inline::AttributeRef { name, parts, .. } if name == "img" => Some(parts.clone()),
            _ => None,
        })
        .unwrap();
    let [P::Image(image)] = parts.as_slice() else {
        panic!("{parts:?}");
    };
    assert_eq!(
        (image.target.as_str(), alt(image)),
        ("v.png", "From *attr*".to_owned())
    );
}

#[test]
fn sources_use_imagesdir_at_the_macro_and_drop_line_removes_lines() {
    let doc = parse(
        ":imagesdir: a\n\nimage:x.png[]\n\n:imagesdir: b/\n\nimage:x.png[] image:{imagesdir}y.png[]\n\n\
         :attribute-missing: drop-line\n\nline image:{nope}.png[]\n\nkept",
    )
    .document;
    let blocks = doc.flat_blocks();
    assert_eq!(images(paragraph(blocks[0]))[0].0.src, "a/x.png");
    let second: Vec<_> = images(paragraph(blocks[1]))
        .into_iter()
        .map(|(i, _)| i.src.clone())
        .collect();
    assert_eq!(second, ["b/x.png", "b/b/y.png"]);
    assert!(images(paragraph(blocks[2])).is_empty());
}

#[test]
fn section_ids_ignore_images_and_xref_labels_drop_their_links() {
    let doc = parse("== See image:a.png[A,link=x.html] here\n\n<<_see_here>>").document;
    let BlockKind::Section { blocks, .. } = &doc.blocks[0].kind else {
        panic!("{:?}", doc.blocks[0].kind);
    };
    assert_eq!(blocks[0].id.as_deref(), Some("_see_here"));
    let label = paragraph(&blocks[1]);
    let Inline::CrossReference { children, .. } = &label[0] else {
        panic!("{label:?}");
    };
    let (image, _) = images(children)[0];
    assert!(!image.attributes.contains_key("link"));
}

#[test]
fn macros_are_inline_macro_tokens() {
    let source = "A image:a.png[*b*] and icon:x[] \\image:c.png[]";
    let found: Vec<_> = tokens(source)
        .into_iter()
        .filter(|t| t.kind == TokenKind::InlineMacro)
        .map(|t| t.span)
        .collect();
    assert_eq!(found, [Span::new(2, 18), Span::new(23, 31)]);
}

#[test]
fn standalone_icon_font_defaults_follow_ruby() {
    // Ruby's fallback `stylesdir` and the standalone `iconfont-remote`.
    let doc = parse(":icons: font\n\n{stylesdir}|{iconfont-remote}|").document;
    assert_eq!(doc.attributes.get("stylesdir"), Some("."));
    assert_eq!(doc.attributes.get("iconfont-remote"), Some(""));
    let doc = parse(":iconfont-remote!:\n:stylesdir: css\n\nx").document;
    assert_eq!(doc.attributes.get("iconfont-remote"), None);
    assert_eq!(doc.attributes.get("stylesdir"), Some("css"));
    let mut attributes = adoctra_core::Attributes::new();
    attributes.set("stylesdir", "host");
    let options = adoctra_core::ParseOptions {
        attributes,
        ..Default::default()
    };
    let doc = adoctra_core::parse_with_options(":stylesdir: doc\n\nx", &options).document;
    assert_eq!(doc.attributes.get("stylesdir"), Some("host"));
}

#[test]
fn web_paths_can_join_uri_targets_like_stylesheets() {
    use adoctra_core::normalize_web_path as web_path;
    assert_eq!(
        web_path("font-awesome.css", Some("."), false),
        "./font-awesome.css"
    );
    assert_eq!(
        web_path("fa 5.css", Some("css/a b"), false),
        "css/a%20b/fa%205.css"
    );
    assert_eq!(web_path("x.css", Some("../s/./t"), false), "../s/t/x.css");
    assert_eq!(web_path("x.css", Some("/abs/"), false), "/abs/x.css");
    assert_eq!(web_path("x.css", None, false), "x.css");
    // Only a preserved URI target keeps its form.
    assert_eq!(
        web_path("https://e.com/x.css", Some("."), false),
        "./https://e.com/x.css"
    );
    assert_eq!(
        web_path("https://e.com/a b.css", Some("."), true),
        "https://e.com/a%20b.css"
    );
}
