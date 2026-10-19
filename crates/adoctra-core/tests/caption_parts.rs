//! Captions keep the markup Ruby stores in `@caption`: attribute values with
//! formatting or raw markup, raw or substituted `caption=` values and section
//! signifiers. The flat `caption` stays the visible text.
use adoctra_core::{parse, AttributeFormat as Format, AttributePart, Block, BlockKind};

fn blocks(source: &str) -> Vec<Block> {
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    result.document.flat_blocks().into_iter().cloned().collect()
}

fn captioned(source: &str) -> Block {
    blocks(source)
        .into_iter()
        .find(|b| b.caption.is_some())
        .expect("a captioned block")
}

fn text(value: &str) -> AttributePart {
    AttributePart::Text(value.into())
}

fn raw(value: &str) -> AttributePart {
    AttributePart::Raw(value.into())
}

#[test]
fn caption_attributes_keep_their_parts() {
    let block = captioned(":example-caption: pass:q[*Ex*]\n\n.T\n====\nx\n====");
    assert_eq!(block.caption.as_deref(), Some("Ex 1. "));
    assert_eq!(
        block.caption_parts,
        [
            AttributePart::Open(Format::Strong),
            raw("Ex"),
            AttributePart::Close(Format::Strong),
            text(" 1. "),
        ]
    );
    for (source, word) in [
        (
            ":table-caption: pass:q[*T*]\n\n.Tt\n|===\n|a\n|===",
            "T 1. ",
        ),
        (
            ":figure-caption: pass:q[*F*]\n\n.T\nimage::a.png[]",
            "F 1. ",
        ),
        (
            ":listing-caption: pass:q[*L*]\n\n.T\n----\nx\n----",
            "L 1. ",
        ),
    ] {
        let block = captioned(source);
        assert_eq!(block.caption.as_deref(), Some(word));
        assert_eq!(block.caption_parts[0], AttributePart::Open(Format::Strong));
    }
    // The document `caption` replaces the prefix with its own parts.
    let block = captioned(":caption: pass:q[_C_]\n\n.T\n====\nx\n====");
    assert_eq!(block.caption.as_deref(), Some("C"));
    assert_eq!(
        block.caption_parts[0],
        AttributePart::Open(Format::Emphasis)
    );
}

#[test]
fn plain_captions_have_no_parts() {
    let block = captioned(".T\n====\nx\n====");
    assert_eq!(block.caption.as_deref(), Some("Example 1. "));
    assert!(block.caption_parts.is_empty());
    let block = captioned(":example-caption: A &amp; <b>\n\n.T\n====\nx\n====");
    assert_eq!(block.caption.as_deref(), Some("A &amp; <b> 1. "));
    assert!(block.caption_parts.is_empty());
}

#[test]
fn block_captions_are_raw_or_substituted() {
    // Unquoted and double-quoted values are inserted raw, like Ruby.
    let block = captioned("[caption=\"<i>C</i> \"]\n.T\n====\nx\n====");
    assert_eq!(block.caption.as_deref(), Some("<i>C</i> "));
    assert_eq!(block.caption_parts, [raw("<i>C</i> ")]);
    // A single-quoted value keeps its substituted markup.
    let block = captioned("[caption='*Cap* &']\n.T\n====\nx\n====");
    assert_eq!(block.caption.as_deref(), Some("Cap &"));
    assert_eq!(
        block.caption_parts,
        [
            AttributePart::Open(Format::Strong),
            text("Cap"),
            AttributePart::Close(Format::Strong),
            text(" &"),
        ]
    );
    // A later raw value replaces a substituted one.
    let block = captioned("[caption='*a*']\n[caption=b<i>]\n.T\n====\nx\n====");
    assert_eq!(block.caption_parts, [raw("b<i>")]);
    // An empty caption is plain.
    let block = captioned(":example-caption: Ex\n\n[caption=]\n.T\n====\nx\n====");
    assert_eq!(block.caption.as_deref(), Some(""));
    assert!(block.caption_parts.is_empty());
}

#[test]
fn admonition_labels_take_substituted_captions() {
    let blocks = blocks("[NOTE,caption='*Hi* &']\nx\n\n[TIP,caption=<b>]\ny");
    let labels: Vec<_> = blocks
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Admonition { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        labels,
        [
            vec![
                AttributePart::Open(Format::Strong),
                text("Hi"),
                AttributePart::Close(Format::Strong),
                text(" &"),
            ],
            vec![raw("<b>")],
        ]
    );
}

#[test]
fn section_signifiers_keep_their_parts() {
    let headings: Vec<_> = blocks(
        ":doctype: book\n:sectnums:\n:chapter-signifier: pass:q[*Ch*]\n:appendix-caption: pass:q[_Ap_]\n\n== A\n\n=== A1\n\n[appendix]\n== B",
    )
    .into_iter()
    .filter(|b| matches!(b.kind, BlockKind::Heading { .. }))
    .collect();
    let captions: Vec<_> = headings.iter().map(|h| h.caption.as_deref()).collect();
    assert_eq!(captions, [Some("Ch 1. "), Some("1.1. "), Some("Ap A: ")]);
    assert_eq!(
        headings[0].caption_parts,
        [
            AttributePart::Open(Format::Strong),
            raw("Ch"),
            AttributePart::Close(Format::Strong),
            text(" 1. "),
        ]
    );
    assert!(headings[1].caption_parts.is_empty());
    assert_eq!(
        headings[2].caption_parts[0],
        AttributePart::Open(Format::Emphasis)
    );
}

#[test]
fn reference_labels_use_the_caption_parts() {
    // Ruby's `@caption.chomp '. '` keeps the caption's markup.
    let blocks = blocks(":xrefstyle: full\n\n<<e>>\n\n[#e,caption='*Cap*. ']\n.T\n====\nx\n====");
    let BlockKind::Paragraph { inlines } = &blocks[0].kind else {
        panic!()
    };
    let adoctra_core::Inline::CrossReference { children, .. } = &inlines[0] else {
        panic!("{inlines:?}")
    };
    let adoctra_core::Inline::Substituted { parts, .. } = &children[0] else {
        panic!("{children:?}")
    };
    assert_eq!(
        parts[..3],
        [
            AttributePart::Open(Format::Strong),
            text("Cap"),
            AttributePart::Close(Format::Strong),
        ]
    );
    let label: String = children
        .iter()
        .map(|c| match c {
            adoctra_core::Inline::Text { value, .. }
            | adoctra_core::Inline::Substituted { value, .. } => value.as_str(),
            _ => "",
        })
        .collect();
    assert_eq!(label, "Cap, \u{201c}T\u{201d}");
}
