use adoctra_core::{parse, Attributes, Block, BlockKind, Inline, ParseOptions};

fn text(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Inline::Text { value, .. } | Inline::Raw { value, .. } => value.clone(),
            Inline::Strong { children, .. } | Inline::CrossReference { children, .. } => {
                text(children)
            }
            Inline::AttributeRef { name, value, .. } => {
                value.clone().unwrap_or_else(|| format!("{{{name}}}"))
            }
            Inline::Anchor { .. } => String::new(),
            _ => panic!("{node:?}"),
        })
        .collect()
}

fn label(block: &Block) -> String {
    match &block.kind {
        BlockKind::Admonition { label, .. } => adoctra_core::attributes::parts_text(label),
        _ => panic!("{block:?}"),
    }
}

fn heading(block: &Block) -> (u8, String) {
    match &block.kind {
        BlockKind::Heading { level, inlines } => (*level, text(inlines)),
        _ => panic!("{block:?}"),
    }
}

#[test]
fn admonition_labels_use_caption_then_document_captions_while_reading() {
    let doc = parse(
        "[NOTE,caption=Hey]\nx\n\nNOTE: y\n\n:note-caption: Nota\n\n[NOTE]\n====\nz\n====\n\n[TIP]\nt",
    )
    .document;
    assert_eq!(label(&doc.blocks[0]), "Hey");
    assert_eq!(label(&doc.blocks[1]), "Note");
    assert_eq!(label(&doc.blocks[2]), "Nota");
    assert_eq!(label(&doc.blocks[3]), "Tip");
    assert_eq!(doc.attributes.get("warning-caption"), Some("Warning"));
    // A `caption` attribute is raw, as asciidoctor emits it; document captions
    // keep the protection of their definition.
    let doc = parse(":note-caption: A <b>\n\nNOTE: x\n\n[NOTE,caption=<i>B</i>]\ny").document;
    let BlockKind::Admonition { label: first, .. } = &doc.blocks[0].kind else {
        panic!()
    };
    assert_eq!(first, &[adoctra_core::AttributePart::Text("A <b>".into())]);
    let BlockKind::Admonition { label: second, .. } = &doc.blocks[1].kind else {
        panic!()
    };
    assert_eq!(
        second,
        &[adoctra_core::AttributePart::Raw("<i>B</i>".into())]
    );
    // An unset caption leaves an empty label.
    let doc = parse(":caution-caption!:\n\nCAUTION: x").document;
    assert_eq!(label(&doc.blocks[0]), "");
    // A list continuation admonition reads the same state.
    let doc = parse(":tip-caption: Hint\n\n* item\n+\nTIP: x").document;
    let BlockKind::List { items, .. } = &doc.blocks[0].kind else {
        panic!()
    };
    assert_eq!(label(&items[0].blocks[0]), "Hint");
}

#[test]
fn host_captions_override_document_captions() {
    let mut attributes = Attributes::new();
    attributes.set("note-caption", "Host");
    let doc = adoctra_core::parse_with_options(
        ":note-caption: Doc\n\nNOTE: x",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    assert_eq!(label(&doc.blocks[0]), "Host");
}

#[test]
fn discrete_headings_get_ids_but_are_not_sections() {
    let doc = parse("p\n\n[discrete]\n== H\n\n[float#f.r]\n=== F\n\n== H\n\n<<_h>> <<f>> <<_h_2>>")
        .document;
    assert_eq!(doc.blocks[1].id.as_deref(), Some("_h"));
    assert_eq!(doc.blocks[1].attrs.style.as_deref(), Some("discrete"));
    assert_eq!(doc.blocks[2].id.as_deref(), Some("f"));
    assert_eq!(doc.blocks[3].id.as_deref(), Some("_h_2"));
    let BlockKind::Paragraph { inlines } = &doc.blocks[4].kind else {
        panic!()
    };
    assert_eq!(text(inlines), "H F H");
}

#[test]
fn level_zero_headings_outside_the_title_are_parts_with_ids() {
    let doc = parse("= Doc\n\n= Part\n\n<<_part>>").document;
    assert!(doc.has_title);
    assert_eq!(doc.blocks[0].id, None);
    assert_eq!(doc.blocks[1].id.as_deref(), Some("_part"));
    // A block title above the first heading prevents a document title.
    let doc = parse(".T\n= Part\n\npara").document;
    assert!(!doc.has_title && doc.header.is_none());
    assert_eq!(doc.blocks[0].id.as_deref(), Some("_part"));
    // A doctitle attribute without a source title is still the title.
    let doc = parse(":doctitle: Only\n\n== S").document;
    assert!(doc.has_title);
    assert_eq!(doc.blocks[0].id, None);
}

#[test]
fn trailing_section_anchors_name_the_section() {
    let source = "p\n\n[reftext=R]\n== Ж *t* [[s,*SS* {x}]]\n\n<<s>>";
    let doc = parse(source).document;
    let block = &doc.blocks[1];
    assert_eq!(block.id.as_deref(), Some("s"));
    assert_eq!(heading(block), (1, "Ж t".to_owned()));
    // Title inlines stop before the anchor; the raw reftext keeps its spans
    // and gets c/q/r without the attribute stage.
    let BlockKind::Heading { inlines, .. } = &block.kind else {
        panic!()
    };
    let last = inlines.last().unwrap().span();
    assert_eq!(&source[..last.end], &source[..source.find(" [[s").unwrap()]);
    let reftext = block.reftext.as_ref().unwrap();
    assert_eq!(text(reftext), "SS {x}");
    let strong = reftext[0].span();
    assert_eq!(&source[strong.start..strong.end], "*SS*");
}

#[test]
fn section_anchors_respect_metadata_ids_escapes_and_the_document_title() {
    let doc = parse("p\n\n[[x]]\n== S [[s,SS]]\n\n<<x>> <<s>>").document;
    assert_eq!(doc.blocks[1].id.as_deref(), Some("x"));
    assert_eq!(heading(&doc.blocks[1]).1, "S ");
    let BlockKind::Paragraph { inlines } = &doc.blocks[2].kind else {
        panic!()
    };
    // The borrowed title drops the inline anchor; `s` is not cataloged.
    assert_eq!(text(inlines), "S  [s]");
    let doc = parse("p\n\n== S \\[[s,SS]]\n\n== T[[t]]").document;
    assert_eq!(doc.blocks[1].id.as_deref(), Some("_s_sss"));
    assert_eq!(doc.blocks[2].id.as_deref(), Some("_t"));
    let doc = parse("= Doc [[top,T]]\n\npara <<top>>").document;
    assert_eq!(doc.blocks[0].id.as_deref(), Some("top"));
    assert_eq!(doc.attributes.get("doctitle"), Some("Doc"));
    assert_eq!(doc.header.as_ref().unwrap().title, "Doc");
    let BlockKind::Paragraph { inlines } = &doc.blocks[1].kind else {
        panic!()
    };
    // The document title lends its text; the anchor reftext is ignored.
    assert_eq!(text(inlines), "para Doc");
}

#[test]
fn incomplete_section_anchors_do_not_panic() {
    for source in [
        "== S [[",
        "== S [[s,",
        "== S [[Ж,]]",
        "== S [[s, ]]",
        "= T [[t]]",
        "== \\[[s]]",
    ] {
        let result = parse(source);
        assert!(result
            .diagnostics
            .iter()
            .all(|d| source.get(d.span.start..d.span.end).is_some()));
        for block in &result.document.blocks {
            if let BlockKind::Heading { inlines, .. } = &block.kind {
                for node in inlines {
                    assert!(source.get(node.span().start..node.span().end).is_some());
                }
            }
        }
    }
}
