//! The `image::target[…]` block macro: Ruby's `BlockMediaMacroRx` branch of
//! `next_block`, `AbstractBlock#alt`, figure captions and `image_uri`.
use adoctra_core::{
    parse, tokens, AttributePart, Block, BlockKind, Inline, Severity, Span, TokenKind,
};

fn blocks(source: &str) -> Vec<Block> {
    parse(source).document.into_flat_blocks()
}

/// The image fields of a block: target, src and alt parts.
fn image(block: &Block) -> (&str, &str, &[AttributePart]) {
    match &block.kind {
        BlockKind::Image {
            target, src, alt, ..
        } => (target, src, alt),
        other => panic!("not an image: {other:?}"),
    }
}

fn alt_text(block: &Block) -> String {
    adoctra_core::attributes::parts_text(image(block).2)
}

fn text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { value, .. } | Inline::Raw { value, .. } => value.clone(),
            Inline::Strong { children, .. } | Inline::CrossReference { children, .. } => {
                text(children)
            }
            other => panic!("unexpected inline {other:?}"),
        })
        .collect()
}

#[test]
fn macro_line_yields_an_image_with_its_span_and_named_attributes() {
    let source = "Intro\n\nimage::dir/my_cool-pic.png[Alt,200,100,link=l.html]\n";
    let blocks = blocks(source);
    let block = &blocks[1];
    let line = "image::dir/my_cool-pic.png[Alt,200,100,link=l.html]";
    assert_eq!(block.span, Span::new(7, 7 + line.len()));
    assert_eq!(image(block).0, "dir/my_cool-pic.png");
    assert_eq!(image(block).1, "dir/my_cool-pic.png");
    assert_eq!(alt_text(block), "Alt");
    let named = &block.attrs.named;
    assert_eq!(
        (
            named["alt"].as_str(),
            named["width"].as_str(),
            named["height"].as_str()
        ),
        ("Alt", "200", "100")
    );
    assert_eq!(named["link"], "l.html");
    assert_eq!(block.attrs.style, None);
    assert!(block.title.is_none() && block.caption.is_none());
}

#[test]
fn alt_comes_from_the_macro_the_style_or_the_basename() {
    let blocks = blocks(
        "image::my_cool-pic.png[]\n\n\
         [Styled.role#myid]\nimage::a.png[]\n\n\
         [alt=Z]\nimage::b.png[]\n\n\
         [Q]\nimage::c.png[x, alt=y]\n\n\
         image::d.png[alt=]\n\n\
         image::e.png['s *q*']\n",
    );
    let alts: Vec<_> = blocks.iter().map(alt_text).collect();
    assert_eq!(alts, ["my cool pic", "Styled", "Z", "y", "", "s *q*"]);
    // The shorthand still supplies id and roles; the style itself is dropped.
    assert_eq!(blocks[1].id.as_deref(), Some("myid"));
    assert_eq!(blocks[1].roles, ["role"]);
    assert!(blocks.iter().all(|b| b.attrs.style.is_none()));
}

#[test]
fn explicit_alt_gets_replacements_with_raw_entities_but_default_alt_does_not() {
    let blocks = blocks("image::a.png[Alt \"q\" (C) it's]\n\nimage::x_(C).png[]\n");
    assert_eq!(
        image(&blocks[0]).2,
        [
            AttributePart::Text("Alt \"q\" ".into()),
            AttributePart::Raw("&#169;".into()),
            AttributePart::Text(" it".into()),
            AttributePart::Raw("&#8217;".into()),
            AttributePart::Text("s".into()),
        ]
    );
    assert_eq!(image(&blocks[1]).2, [AttributePart::Text("x (C)".into())]);
}

#[test]
fn macro_list_runs_the_early_attribute_stage_without_sub_result() {
    let result = parse(":a: Ж\n\nimage::p.png[{a} '*b*',width={w}]\n");
    let blocks = result.document.into_flat_blocks();
    assert_eq!(alt_text(&blocks[0]), "Ж '*b*'");
    // A missing reference is kept by the default skip policy.
    assert_eq!(blocks[0].attrs.named["width"], "{w}");
    // Named title/reftext/options/id/role share the attribute-line keys.
    let blocks = self::blocks(
        "[[old]]\n.Old\n[.r1]\nimage::p.png[title=New,id=new,role=r2,opts=nofollow,reftext=R]\n\n<<new>>",
    );
    let img = &blocks[0];
    assert_eq!(img.id.as_deref(), Some("new"));
    assert_eq!(img.roles, ["r2"]);
    assert_eq!(img.attrs.options, ["nofollow"]);
    assert_eq!(text(img.title.as_ref().unwrap()), "New");
    assert_eq!(img.caption.as_deref(), Some("Figure 1. "));
    let BlockKind::Paragraph { inlines } = &blocks[1].kind else {
        panic!("{:?}", blocks[1].kind);
    };
    assert_eq!(text(inlines), "R");
}

#[test]
fn target_references_expand_and_drop_line_drops_the_block_and_metadata() {
    let source = ":d: dir\n\nimage::{d}/x.png[]\n";
    assert_eq!(image(&blocks(source)[0]).0, "dir/x.png");

    let source = ":attribute-missing: drop-line\n\n[[keep]]\n.Kept\nimage::{nope}.png[y]\n\npara\n";
    let result = parse(source);
    let blocks = result.document.into_flat_blocks();
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
    assert!(blocks[0].id.is_none() && blocks[0].title.is_none());
    let info = &result.diagnostics[0];
    assert_eq!(info.severity, Severity::Info);
    assert_eq!(
        info.message,
        "dropping line containing reference to missing attribute: nope"
    );
    let at = source.find("{nope}").unwrap();
    assert_eq!(info.span, Span::new(at, at + "{nope}".len()));

    // The warn policy keeps the reference and warns at its physical span.
    let source = ":attribute-missing: warn\n\nimage::a/{nope}.png[]\n";
    let result = parse(source);
    let blocks = result.document.into_flat_blocks();
    assert_eq!(image(&blocks[0]).0, "a/{nope}.png");
    assert_eq!(alt_text(&blocks[0]), "{nope}");
    let warning = &result.diagnostics[0];
    assert_eq!(warning.severity, Severity::Warning);
    let at = source.find("{nope}").unwrap();
    assert_eq!(warning.span, Span::new(at, at + 6));
}

#[test]
fn src_resolves_imagesdir_at_conversion() {
    let blocks = blocks(
        ":imagesdir: img/\n\nimage::../a b.png[]\n\n:imagesdir: b\n\n\
         image::x.png[]\n\nimage::/root.png[]\n\nimage::https://h.org/a b.png[]\n",
    );
    let srcs: Vec<_> = blocks.iter().map(|b| image(b).1).collect();
    assert_eq!(
        srcs,
        [
            "a%20b.png",
            "b/x.png",
            "/root.png",
            "https://h.org/a%20b.png"
        ]
    );
}

#[test]
fn titled_images_are_numbered_figures() {
    let blocks = blocks(
        ".A\nimage::a.png[]\n\nimage::untitled.png[]\n\n\
         .Ex\n====\n.Inner\nimage::i.png[]\n====\n\n\
         .Own\nimage::c.png[caption=\"Cap: \"]\n\n\
         .B\nimage::b.png[]\n",
    );
    let captions: Vec<_> = blocks.iter().map(|b| b.caption.as_deref()).collect();
    assert_eq!(
        captions,
        [
            Some("Figure 1. "),
            None,
            Some("Example 1. "),
            Some("Cap: "),
            Some("Figure 3. ")
        ]
    );
    // A nested figure is numbered when it completes, before its container.
    let BlockKind::Example { blocks: inner } = &blocks[2].kind else {
        panic!("{:?}", blocks[2].kind);
    };
    assert_eq!(inner[0].caption.as_deref(), Some("Figure 2. "));
    // Localized and unset prefixes, as document attributes.
    let blocks = self::blocks(
        ":figure-caption: Рис.\n\n.One\nimage::a.png[]\n\n:figure-caption!:\n\n.Two\nimage::b.png[]\n",
    );
    assert_eq!(blocks[0].caption.as_deref(), Some("Рис. 1. "));
    assert_eq!(blocks[1].caption, None);
    assert_eq!(
        parse("= D").document.attributes.get("figure-caption"),
        Some("Figure")
    );
}

#[test]
fn image_titles_label_references_and_xrefstyles() {
    let blocks = blocks(
        "[[fig1]]\n.A *bold* title\nimage::a.png[]\n\n[[plain]]\nimage::p.png[]\n\n\
         <<fig1>> <<plain>>\n\n:xrefstyle: full\n\n<<fig1>>\n\n:xrefstyle: short\n\n<<fig1>>\n",
    );
    let labels: Vec<_> = blocks[2..]
        .iter()
        .map(|b| match &b.kind {
            BlockKind::Paragraph { inlines } => text(inlines),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        labels,
        [
            "A bold title [plain]",
            "Figure 1, \u{201c}A bold title\u{201d}",
            "Figure 1"
        ]
    );
}

#[test]
fn verbatim_styles_and_paragraph_text_keep_macro_lines() {
    let blocks = blocks(
        "[literal]\nimage::a.png[]\n\n[verse]\nimage::b.png[]\n\n[literal]\ntoc::[]\n\n\
         para\nimage::c.png[]\n\nimage::d.png [x]\n\n[NOTE]\nimage::e.png[]\n",
    );
    assert!(matches!(blocks[0].kind, BlockKind::Literal { .. }));
    assert!(matches!(blocks[1].kind, BlockKind::Verse { .. }));
    assert!(matches!(blocks[2].kind, BlockKind::Literal { .. }));
    assert!(matches!(blocks[3].kind, BlockKind::Paragraph { .. }));
    assert!(matches!(blocks[4].kind, BlockKind::Paragraph { .. }));
    // Another style becomes the alt text.
    assert_eq!(alt_text(&blocks[5]), "NOTE");
}

#[test]
fn list_continuation_images_take_their_macro_metadata() {
    let doc = parse("* item\n+\nimage::d.png[li,title=Named,id=x]\n").document;
    let BlockKind::List { items, .. } = &doc.blocks[0].kind else {
        panic!("{:?}", doc.blocks[0].kind);
    };
    let img = &items[0].blocks[0];
    assert_eq!(alt_text(img), "li");
    assert_eq!(img.id.as_deref(), Some("x"));
    assert_eq!(text(img.title.as_ref().unwrap()), "Named");
    assert_eq!(img.caption.as_deref(), Some("Figure 1. "));
}

#[test]
fn scaledwidth_without_units_is_a_percentage() {
    let blocks = blocks("image::a.png[scaledwidth=50]\n\nimage::b.png[scaledwidth=]\n");
    assert_eq!(blocks[0].attrs.named["scaledwidth"], "50%");
    assert!(!blocks[1].attrs.named.contains_key("scaledwidth"));
}

#[test]
fn image_lines_are_block_macro_tokens() {
    let source = "Intro\n\nimage::a.png[A]\n\ntext\nimage::b.png[]\n";
    let tokens = tokens(source);
    let macros: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::BlockMacro)
        .map(|t| t.span)
        .collect();
    assert_eq!(macros, [Span::new(7, 22)]);
}
