//! Listing captions (Ruby's `CAPTION_ATTRIBUTE_NAMES[:listing]`): titled
//! listing/source blocks number with `listing-caption` only when the document
//! sets it; an explicit `caption` applies even without it.

use adoctra_core::{parse, Block, BlockKind, Inline, Span};

fn captions(blocks: &[Block]) -> Vec<Option<&str>> {
    blocks
        .iter()
        .map(|block| block.caption.as_deref())
        .collect()
}

fn link_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text { value, .. } => out.push_str(value),
            Inline::AttributeRef {
                value: Some(value), ..
            } => out.push_str(value),
            Inline::CrossReference { children, .. } => out.push_str(&link_text(children)),
            _ => {}
        }
    }
    out
}

#[test]
fn listings_number_only_with_a_listing_caption() {
    let source = ":listing-caption: Listing\n\n.First\n----\none\n----\n\n\
                  [source,ruby]\n.Second\n----\ntwo\n----\n\n.Lit\n....\nlit\n....\n\n\
                  [listing]\n.Para\nthree\n\n.Fenced\n```js\nx\n```\n\n----\nuntitled\n----\n";
    let blocks = parse(source).document.into_blocks();
    assert!(matches!(blocks[2].kind, BlockKind::Literal { .. }));
    for index in [0, 1, 3, 4, 5] {
        assert!(matches!(blocks[index].kind, BlockKind::Listing { .. }));
    }
    // Literal blocks have no caption family; untitled listings never advance
    // the counter.
    assert_eq!(
        captions(&blocks),
        [
            Some("Listing 1. "),
            Some("Listing 2. "),
            None,
            Some("Listing 3. "),
            Some("Listing 4. "),
            None,
        ]
    );
    // The block span starts at the fence, after its title line.
    let fence = ":listing-caption: Listing\n\n.First\n".len();
    assert_eq!(
        blocks[0].span,
        Span::new(fence, fence + "----\none\n----".len())
    );
}

#[test]
fn listings_without_listing_caption_take_only_explicit_captions() {
    let source = ".Plain\n----\nx\n----\n\n[caption=\"Code A: \"]\n.Custom\n----\ny\n----\n\n\
                  :caption: Doc\n\n.Doc\n----\nz\n----\n\n:listing-caption: Listing\n\n\
                  .Counted\n----\nw\n----\n";
    let result = parse(source);
    let blocks: Vec<_> = result
        .document
        .into_blocks()
        .into_iter()
        .filter(|block| matches!(block.kind, BlockKind::Listing { .. }))
        .collect();
    // A document `caption` replaces the prefix without numbering, so the
    // first counted listing is still number 1 — like the explicit caption.
    assert_eq!(
        captions(&blocks),
        [None, Some("Code A: "), Some("Doc"), Some("Doc")]
    );
}

#[test]
fn listing_numbers_follow_attribute_changes_nesting_and_xrefstyle() {
    let source = ":listing-caption: Listing\n:xrefstyle: full\n\n\
                  See <<a>>, xref:b[xrefstyle=short] and xref:c[xrefstyle=basic].\n\n\
                  .Outer\n====\n[#a]\n.Inner\n----\ninner\n----\n====\n\n\
                  :listing-caption!:\n\n.Off\n----\noff\n----\n\n:listing-caption: Code\n\n\
                  [#b]\n.Second\n----\nb\n----\n\n[#c]\n.Third\n----\nc\n----\n\n\
                  Number {listing-number}.\n";
    let blocks = parse(source).document.into_blocks();
    let BlockKind::Example { blocks: inner } = &blocks[1].kind else {
        panic!("{:?}", blocks[1].kind);
    };
    // Children complete before their parent.
    assert_eq!(blocks[1].caption.as_deref(), Some("Example 1. "));
    assert_eq!(inner[0].caption.as_deref(), Some("Listing 1. "));
    assert_eq!(
        captions(&blocks[2..5]),
        [None, Some("Code 2. "), Some("Code 3. ")]
    );
    let BlockKind::Paragraph { inlines } = &blocks[0].kind else {
        panic!("{:?}", blocks[0].kind);
    };
    assert_eq!(
        link_text(inlines),
        // The reference converts before `:listing-caption!:`, so the short
        // style reads the prefix in effect there, not the block's own.
        "See Listing 1, \u{201c}Inner\u{201d}, Listing 2 and Third."
    );
    let BlockKind::Paragraph { inlines } = &blocks[5].kind else {
        panic!("{:?}", blocks[5].kind);
    };
    assert_eq!(link_text(inlines), "Number 3.");
}

#[test]
fn listing_captions_in_asciidoc_cells_count_on_the_root_document() {
    let source =
        ":listing-caption: Listing\n\n[cols=1a]\n|===\n|\n.Cell\n----\ncell\n----\n|===\n\n\
                  .After\n----\nafter\n----\n";
    let blocks = parse(source).document.into_blocks();
    assert_eq!(blocks[1].caption.as_deref(), Some("Listing 2. "));
    let BlockKind::Table(table) = &blocks[0].kind else {
        panic!("{:?}", blocks[0].kind);
    };
    let cell = &table.body[0][0];
    let adoctra_core::ast::CellContent::Blocks { blocks: cell, .. } = &cell.content else {
        panic!("{:?}", cell.content);
    };
    assert_eq!(cell[0].caption.as_deref(), Some("Listing 1. "));
}
