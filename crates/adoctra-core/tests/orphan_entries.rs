//! Body attribute entries ride the next block of their collection, like Ruby's
//! `:attribute_entries` in the pending block attributes: an entry with no block
//! after it in a container, list item or table cell, or one before a skipped
//! comment block or dropped block macro, changes reading but not conversion.
//! Converters read `Block::conversion_attributes`, which follows conversion.

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

#[test]
fn conversion_context_excludes_reading_only_effects() {
    // An orphan unset changes reading (the heading's snapshot) but not the
    // attributes the heading converts with.
    let doc = parse(":sectanchors:\n\n====\n:sectanchors!:\n====\n\n== Sec").document;
    let BlockKind::Section { blocks, .. } = &doc.blocks[1].kind else {
        panic!("{:?}", doc.blocks[1].kind);
    };
    let heading = &blocks[0];
    assert_eq!(heading.attribute_context.get("sectanchors"), None);
    assert_eq!(heading.conversion_attributes().get("sectanchors"), Some(""));
    // A section title's expression runs while reading only.
    let doc = parse(":sectlinks:\n\n== A {set:sectlinks!}\n\n== B").document;
    let BlockKind::Section { blocks, .. } = &doc.blocks[1].kind else {
        panic!("{:?}", doc.blocks[1].kind);
    };
    assert_eq!(blocks[0].attribute_context.get("sectlinks"), None);
    assert_eq!(blocks[0].conversion_attributes().get("sectlinks"), Some(""));
}

#[test]
fn conversion_context_follows_replay_and_earlier_conversions() {
    let doc = parse("Para {set:prewrap!}\n\n----\nx\n----\n\n:prewrap:\n\n----\ny\n----").document;
    // Taken on entry: the paragraph's own expression comes after.
    assert_eq!(
        doc.blocks[0].conversion_attributes().get("prewrap"),
        Some("")
    );
    assert_eq!(doc.blocks[1].conversion_attributes().get("prewrap"), None);
    assert_eq!(
        doc.blocks[2].conversion_attributes().get("prewrap"),
        Some("")
    );
    // Reading never executed the paragraph's expression.
    assert_eq!(doc.blocks[1].attribute_context.get("prewrap"), Some(""));
    // Blocks converting with equal attributes share one snapshot.
    let doc = parse("a\n\nb\n\n:x: 1\n\nc").document;
    let [a, b, c] = &doc.blocks[..] else {
        panic!("{:?}", doc.blocks);
    };
    let shared = |x: &Block, y: &Block| {
        std::sync::Arc::ptr_eq(
            x.conversion_context.as_ref().unwrap(),
            y.conversion_context.as_ref().unwrap(),
        )
    };
    assert!(shared(a, b));
    assert!(!shared(b, c));
    assert_eq!(c.conversion_attributes().get("x"), Some("1"));
}

#[test]
fn conversion_context_inside_asciidoc_cells_stays_in_the_cell() {
    // The cell inherits the parent's attributes locked, so its unset is
    // ignored; its own declaration converts inside the cell only.
    let source = ":sectanchors:\n\n[cols=1a]\n|===\n|\n:sectanchors!:\n:local: 1\n\n== In cell\n|===\n\n== After";
    let doc = parse(source).document;
    let BlockKind::Table(table) = &doc.blocks[0].kind else {
        panic!("{:?}", doc.blocks[0].kind);
    };
    let adoctra_core::ast::CellContent::Blocks { blocks, .. } = &table.body[0][0].content else {
        panic!("{:?}", table.body[0][0].content);
    };
    let BlockKind::Section { blocks: inner, .. } = &blocks[0].kind else {
        panic!("{:?}", blocks[0].kind);
    };
    let inner = inner[0].conversion_attributes();
    assert_eq!(inner.get("sectanchors"), Some(""));
    assert_eq!(inner.get("local"), Some("1"));
    let BlockKind::Section { blocks: after, .. } = &doc.blocks[1].kind else {
        panic!("{:?}", doc.blocks[1].kind);
    };
    let after = after[0].conversion_attributes();
    assert_eq!(after.get("sectanchors"), Some(""));
    assert_eq!(after.get("local"), None);
}
