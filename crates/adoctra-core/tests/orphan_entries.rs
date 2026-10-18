//! Body attribute entries ride the next block of their collection, like Ruby's
//! `:attribute_entries` in the pending block attributes: an entry with no block
//! after it in a container, list item or table cell, or one before a skipped
//! comment block or dropped block macro, changes reading but not conversion.

use adoctra_core::{parse, Block, BlockKind, Inline, Span};

/// Plain text with unresolved references kept as `{name}` (the skip policy).
fn inline_text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|n| match n {
            Inline::Text { value, .. } => value.clone(),
            Inline::AttributeRef { name, value, .. } => {
                value.clone().unwrap_or_else(|| format!("{{{name}}}"))
            }
            _ => panic!("{n:?}"),
        })
        .collect()
}

fn text(block: &Block) -> String {
    let BlockKind::Paragraph { inlines } = &block.kind else {
        panic!("{block:?}");
    };
    inline_text(inlines)
}

fn last_text(source: &str) -> String {
    text(parse(source).document.blocks.last().unwrap())
}

#[test]
fn orphan_entries_in_containers_do_not_reach_conversion() {
    // Single-line and multiline entries, with metadata or an inactive
    // conditional after them, and in other container kinds.
    for source in [
        "====\n:x: one\n====\n\nAfter {x}.",
        "====\n:x: one \\\n====\n\nAfter {x}.",
        "====\ntext\n\n:x: one\n====\n\nAfter {x}.",
        "====\n:x: one\n[[a]]\n.Title\n====\n\nAfter {x}.",
        "====\n:x: one\nifdef::nope[]\ntext\nendif::[]\n====\n\nAfter {x}.",
        "--\n:x: one\n--\n\nAfter {x}.",
        "____\n:x: one\n____\n\nAfter {x}.",
    ] {
        assert_eq!(last_text(source), "After {x}.", "{source}");
    }
    // A list item's block takes the entry; the next item's orphan is dropped.
    let doc = parse("* item\n+\n:x: one\n\nIn {x}.\n\n* two\n+\n:y: two\n\n* three {y}").document;
    let BlockKind::List { items, .. } = &doc.blocks[0].kind else {
        panic!("{:?}", doc.blocks[0].kind);
    };
    assert_eq!(text(&items[0].blocks[0]), "In one.");
    assert_eq!(inline_text(&items[2].inlines), "three {y}");
}

#[test]
fn orphan_entries_keep_reading_state_and_spans() {
    let source = "====\n:x: one\n====\n\nifdef::x[]\nreading sees {x}\nendif::[]";
    let doc = parse(source).document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[1].kind else {
        panic!("{:?}", doc.blocks[1].kind);
    };
    let at = source.find("{x}").unwrap();
    let [_, Inline::AttributeRef { value, span, .. }] = inlines.as_slice() else {
        panic!("{inlines:?}");
    };
    assert_eq!(*value, None);
    assert_eq!((span.start, span.end), (at, at + 3));
    assert_eq!(doc.blocks[1].attribute_context.get("x"), Some("one"));
    // A later entry still replays; the orphan before it does not.
    assert_eq!(
        last_text("====\n:x: one\n====\n\n:y: two\n\n{x}{y}"),
        "{x}two"
    );
    // Counters restart from the replayed value.
    assert_eq!(
        last_text("n\n\n:n: 1\n\n====\n:n: 2\n====\n\n{counter:n}"),
        "2"
    );
}

#[test]
fn comment_blocks_and_dropped_macros_clear_pending_entries() {
    assert_eq!(
        last_text("p\n\n:x: one\n\n[comment]\nc\n\nAfter {x}."),
        "After {x}."
    );
    assert_eq!(
        last_text("p\n\n:x: one\n\n.Title\n[comment]\n--\nc\n--\n\nAfter {x}."),
        "After {x}."
    );
    // Comment lines and blocks are metadata: entries survive them.
    assert_eq!(
        last_text("p\n\n:x: one\n\n////\nc\n////\n\nKept {x}."),
        "Kept one."
    );
    assert_eq!(last_text("p\n\n:x: one\n// c\n\nKept {x}."), "Kept one.");
    let doc =
        parse(":attribute-missing: drop-line\n\np\n\n:x: one\n\nimage::{nope}[]\n\n{x}\nkept")
            .document;
    assert_eq!(text(doc.blocks.last().unwrap()), "kept");
}

#[test]
fn entries_carry_across_sections_but_not_out_of_table_cells() {
    let doc = parse("== S\n\n:x: one\n\n== Next\n\nAfter {x}.").document;
    let BlockKind::Section { blocks, .. } = &doc.blocks[1].kind else {
        panic!("{:?}", doc.blocks[1].kind);
    };
    assert_eq!(text(&blocks[1]), "After one.");
    let source = "[cols=1a]\n|===\na|\n:x: one\n\ntext {x}\n\n:y: two\n|{y}\n|===\n\n{x}{y}";
    let doc = parse(source).document;
    assert_eq!(text(doc.blocks.last().unwrap()), "{x}{y}");
    let BlockKind::Table(table) = &doc.blocks[0].kind else {
        panic!("{:?}", doc.blocks[0].kind);
    };
    let adoctra_core::ast::CellContent::Blocks { blocks, .. } = &table.body[0][0].content else {
        panic!("{:?}", table.body[0][0].content);
    };
    assert_eq!(text(&blocks[0]), "text one");
    let start = source.find("text").unwrap();
    assert_eq!(blocks[0].span, Span::new(start, start + "text {x}".len()));
}
