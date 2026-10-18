//! Ruby's `:simple` content model of styled paragraph forms: the container
//! keeps the paragraph as its only child and is marked `simple`.

use adoctra_core::{parse, Block, BlockKind, Span};

fn children(block: &Block) -> &[Block] {
    match &block.kind {
        BlockKind::Admonition { blocks, .. }
        | BlockKind::Example { blocks }
        | BlockKind::Sidebar { blocks }
        | BlockKind::Open { blocks }
        | BlockKind::Quote { blocks, .. } => blocks,
        other => panic!("expected a container, got {other:?}"),
    }
}

#[test]
fn styled_paragraph_forms_are_simple_containers() {
    let source = "[NOTE]\na\n\nTIP: b\n\n[example]\nc\n\n[example%collapsible]\nd\n\n\
                  [sidebar]\ne\n\n[open]\nf\n\n[quote,A]\ng\n\n[abstract]\nh\n";
    let blocks = parse(source).document.blocks;
    assert_eq!(blocks.len(), 8);
    for block in &blocks {
        assert!(block.simple, "{:?}", block.kind);
        let [child] = children(block) else {
            panic!("one child expected: {:?}", block.kind);
        };
        assert!(matches!(child.kind, BlockKind::Paragraph { .. }));
        assert!(!child.simple);
    }
    // The label form's paragraph spans the text after the label.
    let child = &children(&blocks[1])[0];
    assert_eq!(child.span, Span::new(15, 16));
    assert_eq!(&source[child.span.start..child.span.end], "b");
}

#[test]
fn delimited_and_other_blocks_are_not_simple() {
    let source = "====\na\n====\n\n[NOTE]\n====\nb\n====\n\n****\nc\n****\n\n--\nd\n--\n\n\
                  [example]\n--\ne\n--\n\n____\nf\n____\n\npara\n\n[literal]\ng\n\n[verse]\nh\n";
    for block in parse(source).document.blocks {
        assert!(!block.simple, "{:?}", block.kind);
    }
}

#[test]
fn book_parts_follow_ruby_partintro_content_models() {
    let source = "= Book\n:doctype: book\n\n= A\n\n[partintro]\nintro\n\n== C1\n\nx\n\n\
                  = B\n\n[open]\nfirst\n\nsecond\n\n== C2\n\ny\n\n= P\n\n[open]\nonly\n\n== C3\n\nz\n";
    let blocks = parse(source).document.blocks;
    let intro = |part: &Block| -> Block {
        let BlockKind::Section { blocks, .. } = &part.kind else {
            panic!("expected a part, got {:?}", part.kind);
        };
        blocks[1].clone()
    };
    // A `[partintro]` paragraph becomes compound; a rebuilt `[open]`
    // paragraph moves into a compound intro; a lone one stays simple.
    let a = intro(&blocks[1]);
    assert!(!a.simple);
    assert_eq!(children(&a).len(), 1);
    let b = intro(&blocks[2]);
    assert!(!b.simple);
    assert_eq!(children(&b).len(), 2);
    let p = intro(&blocks[3]);
    assert!(p.simple);
    assert_eq!(p.attrs.style.as_deref(), Some("partintro"));
}
