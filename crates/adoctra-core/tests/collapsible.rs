//! Collapsible examples (Ruby's `:example` branch of `next_block`) and the
//! `prewrap` default the HTML converter reads for verbatim blocks.

use adoctra_core::{parse, Block, BlockKind, Inline};

fn caption(block: &Block) -> Option<&str> {
    block.caption.as_deref()
}

fn text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { value, .. } => value.clone(),
            other => panic!("expected text, got {other:?}"),
        })
        .collect()
}

#[test]
fn collapsible_examples_are_never_numbered() {
    let source = "[%collapsible]\n.A\n====\na\n====\n\n.B\n====\nb\n====\n\n\
                  [example%collapsible,caption=\"X \"]\n.C\n--\nc\n--\n\n\
                  [collapsible-option=]\n.D\n====\nd\n====\n\n\
                  [options=\"open,collapsible\"]\n.E\n====\ne\n====\n\n.F\n====\nf\n====\n";
    let blocks = parse(source).document.blocks;
    assert_eq!(blocks.len(), 6);
    for block in &blocks {
        assert!(
            matches!(block.kind, BlockKind::Example { .. }),
            "{:?}",
            block.kind
        );
    }
    // Ruby sets `caption` to '' (overriding `caption=`), so the counter
    // advances only for the plain examples.
    let captions: Vec<_> = blocks.iter().map(caption).collect();
    assert_eq!(
        captions,
        [
            Some(""),
            Some("Example 1. "),
            Some(""),
            Some(""),
            Some(""),
            Some("Example 2. ")
        ]
    );
    assert_eq!(text(blocks[2].title.as_ref().unwrap()), "C");
}

#[test]
fn collapsible_examples_nest_and_number_their_children() {
    let source = "[%collapsible]\n.Outer\n====\n.Inner\n=====\nx\n=====\n====\n";
    let blocks = parse(source).document.blocks;
    assert_eq!(caption(&blocks[0]), Some(""));
    let BlockKind::Example { blocks: children } = &blocks[0].kind else {
        panic!("expected an example");
    };
    assert_eq!(caption(&children[0]), Some("Example 1. "));
    // An untitled collapsible example gets no caption at all.
    assert_eq!(
        caption(&parse("[%collapsible]\n====\nx\n====\n").document.blocks[0]),
        None
    );
}

#[test]
fn xrefstyle_uses_the_title_of_a_collapsible_example() {
    let source = ":xrefstyle: full\n\n[[t]]\n[%collapsible]\n.Target\n====\nx\n====\n\n<<t>>\n";
    let blocks = parse(source).document.blocks;
    let BlockKind::Paragraph { inlines } = &blocks[1].kind else {
        panic!("expected a paragraph, got {:?}", blocks[1].kind);
    };
    let Inline::CrossReference { children, .. } = &inlines[0] else {
        panic!("expected a reference, got {inlines:?}");
    };
    assert_eq!(text(children), "Target");
}

#[test]
fn prewrap_is_a_default_attribute_that_blocks_snapshot() {
    let result =
        parse("----\na\n----\n\n:prewrap!:\n\n----\nb\n----\n\n:prewrap:\n\n....\nc\n....\n");
    assert_eq!(result.document.attributes.get("prewrap"), Some(""));
    let blocks = &result.document.blocks;
    let prewrap: Vec<_> = blocks
        .iter()
        .map(|block| block.attribute_context.get("prewrap"))
        .collect();
    assert_eq!(prewrap, [Some(""), None, Some("")]);
    // A header unset reaches the header snapshot.
    let header = parse("= T\n:prewrap!:\n\n----\nx\n----\n").document;
    assert_eq!(header.attributes.get("prewrap"), None);
}
