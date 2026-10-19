//! An AsciiDoc cell's nested document keeps its title like Ruby's inner
//! `Document`: a level-0 heading first (no automatic id), a preamble before
//! its first section, and `notitle` by default, which its header may unset.
use adoctra_core::ast::CellContent;
use adoctra_core::{
    parse, parse_with_options, tokens, Attributes, Block, BlockKind, Inline, ParseOptions,
    TokenKind,
};

/// The first cell's blocks and title flag.
fn cell(source: &str, options: &ParseOptions) -> (Vec<Block>, bool) {
    let result = parse_with_options(source, options);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let table = result
        .document
        .flat_blocks()
        .into_iter()
        .find_map(|b| match &b.kind {
            BlockKind::Table(table) => Some(table.clone()),
            _ => None,
        })
        .expect("a table");
    match &table.body[0][0].content {
        CellContent::Blocks {
            blocks, has_title, ..
        } => (blocks.clone(), *has_title),
        other => panic!("{other:?}"),
    }
}

fn notitle(block: &Block) -> bool {
    block.conversion_attributes().get("notitle").is_some()
}

#[test]
fn nested_titles_are_headings_without_automatic_ids() {
    let options = ParseOptions::default();
    let (blocks, has_title) = cell("|===\na|\n= In *b*\n\nc\n|===", &options);
    assert!(has_title);
    let BlockKind::Heading { level: 0, inlines } = &blocks[0].kind else {
        panic!("{:?}", blocks[0])
    };
    assert!(matches!(&inlines[0], Inline::Text { value, .. } if value == "In "));
    assert!(matches!(&inlines[1], Inline::Strong { .. }));
    assert_eq!(blocks[0].id, None);
    // Hidden by default.
    assert!(notitle(&blocks[0]));
    let (blocks, has_title) = cell("|===\na|\nc\n|===", &options);
    assert!(!has_title);
    assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
}

#[test]
fn nested_headers_may_show_their_title() {
    let options = ParseOptions::default();
    for header in [":showtitle:\n", ":notitle!:\n"] {
        let (blocks, _) = cell(&format!("|===\na|\n= In\n{header}\nc\n|==="), &options);
        assert!(!notitle(&blocks[0]), "{header}");
    }
    let (blocks, _) = cell("|===\na|\n= In\n:showtitle!:\n\nc\n|===", &options);
    assert!(notitle(&blocks[0]));
    // The parent's host locks on these keys do not reach the inner document.
    let mut attributes = Attributes::new();
    attributes.set("showtitle", "");
    let options = ParseOptions {
        attributes,
        ..Default::default()
    };
    let (blocks, _) = cell("|===\na|\n= In\n\nc\n|===", &options);
    assert!(notitle(&blocks[0]));
    let (blocks, _) = cell("|===\na|\n= In\n:showtitle:\n\nc\n|===", &options);
    assert!(!notitle(&blocks[0]));
}

#[test]
fn nested_titles_open_a_preamble() {
    let options = ParseOptions::default();
    let (blocks, _) = cell("|===\na|\n= In\n\npre\n\n== S\n\nx\n|===", &options);
    assert!(
        matches!(blocks[1].kind, BlockKind::Preamble { .. }),
        "{blocks:?}"
    );
    assert!(matches!(blocks[2].kind, BlockKind::Section { .. }));
    let (blocks, _) = cell("|===\na|\npre\n\n== S\n\nx\n|===", &options);
    assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
}

#[test]
fn nested_titles_keep_their_tokens() {
    let source = "|===\na|\n= In *b*\n\nc\n|===";
    let heading = tokens(source)
        .into_iter()
        .find(|t| t.kind == TokenKind::Heading)
        .expect("a heading token");
    assert_eq!(&source[heading.span.start..heading.span.end], "= In *b*");
    // The registry does not know an automatic id for it.
    let result = parse("|===\na|\n= In\n\nc\n|===\n\n<<_in>>");
    assert_eq!(result.diagnostics.len(), 1);
}

#[test]
fn nested_documents_keep_their_header_attributes() {
    // Ruby's `convert_embedded` reads the inner document's header (its
    // automatic TOC); body entries are not part of it.
    let result = parse("|===\na|\n= In\n:toc: preamble\n\n:x: body\n\npre\n\n== S\n|===");
    let BlockKind::Table(table) = &result.document.blocks[0].kind else {
        panic!()
    };
    let CellContent::Blocks { attributes, .. } = &table.body[0][0].content else {
        panic!()
    };
    assert_eq!(attributes.get("toc"), Some(""));
    assert_eq!(attributes.get("toc-placement"), Some("preamble"));
    assert_eq!(attributes.get("x"), None);
    // The parent's TOC is not the inner document's.
    let result = parse(":toc:\n\n|===\na|\n== S\n|===");
    let BlockKind::Table(table) = &result.document.blocks[0].kind else {
        panic!()
    };
    let CellContent::Blocks { attributes, .. } = &table.body[0][0].content else {
        panic!()
    };
    assert_eq!(attributes.get("toc"), None);
}
