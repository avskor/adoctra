//! Thematic and page breaks (Ruby's layout breaks in `next_block` with
//! Markdown syntax) and Markdown fenced code (`:fenced_code`).

use adoctra_core::{parse, tokens, Block, BlockKind, Inline, Severity, Span, TokenKind};

fn kinds(source: &str) -> Vec<(TokenKind, &str)> {
    tokens(source)
        .into_iter()
        .map(|t| (t.kind, &source[t.span.start..t.span.end]))
        .collect()
}

fn listing(block: &Block) -> &str {
    match &block.kind {
        BlockKind::Listing { text, .. } => text,
        other => panic!("expected a listing, got {other:?}"),
    }
}

fn language(block: &Block) -> Option<&str> {
    block.attrs.named.get("language").map(String::as_str)
}

#[test]
fn layout_break_lines_follow_ruby() {
    for line in [
        "'''", "''''", "---", "- - -", "-  -  -", "***", "* * *", "___", "_ _ _", " ---", "   ***",
    ] {
        let source = format!("{line}\n");
        let blocks = parse(&source).document.into_blocks();
        assert_eq!(blocks.len(), 1, "{line:?}");
        assert!(
            matches!(blocks[0].kind, BlockKind::ThematicBreak),
            "{line:?}: {:?}",
            blocks[0].kind
        );
        assert_eq!(blocks[0].span, Span::new(0, line.len()), "{line:?}");
    }
    for line in ["<<<", "<<<<<"] {
        let blocks = parse(line).document.into_blocks();
        assert!(matches!(blocks[0].kind, BlockKind::PageBreak), "{line:?}");
        assert_eq!(blocks[0].span, Span::new(0, line.len()));
    }
    // Not breaks: four-space indentation is a literal paragraph, uneven
    // spacing a list item, other text a paragraph.
    assert!(matches!(
        parse("    ---").document.blocks[0].kind,
        BlockKind::Literal { .. }
    ));
    assert!(matches!(
        parse("- -  -").document.blocks[0].kind,
        BlockKind::List { .. }
    ));
    for line in ["''", "'''x", "<<<x", "--", "-- -", "*-*", "'''\u{a0}"] {
        let blocks = parse(line).document.into_blocks();
        assert!(
            !matches!(
                blocks[0].kind,
                BlockKind::ThematicBreak | BlockKind::PageBreak
            ),
            "{line:?}: {:?}",
            blocks[0].kind
        );
    }
}

#[test]
fn breaks_start_blocks_but_never_interrupt_paragraphs() {
    let blocks = parse("para\n'''\nmore\n\n'''\nafter")
        .document
        .into_blocks();
    assert_eq!(blocks.len(), 3);
    assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
    assert_eq!(blocks[0].span, Span::new(0, 13));
    assert!(matches!(blocks[1].kind, BlockKind::ThematicBreak));
    assert!(matches!(blocks[2].kind, BlockKind::Paragraph { .. }));

    // A list item's text-only first block keeps the line; after a
    // continuation it is a break, as in a container.
    let source = "* item\n'''\n\n* other\n+\n'''\n\n====\n<<<\n====";
    let blocks = parse(source).document.into_blocks();
    let BlockKind::List { items, .. } = &blocks[0].kind else {
        panic!("expected a list");
    };
    assert!(items[0].blocks.is_empty());
    assert!(matches!(items[1].blocks[0].kind, BlockKind::ThematicBreak));
    assert_eq!(items[1].blocks[0].span, Span::new(22, 25));
    let BlockKind::Example { blocks: kids } = &blocks[1].kind else {
        panic!("expected an example");
    };
    assert!(matches!(kids[0].kind, BlockKind::PageBreak));

    // A verbatim style keeps the line as paragraph text.
    let blocks = parse("[literal]\n'''").document.into_blocks();
    assert!(matches!(&blocks[0].kind, BlockKind::Literal { text, .. } if text == "'''"));
}

#[test]
fn break_metadata_is_kept_and_its_title_is_not_executed() {
    // (An explicit id with a `{` in the title would execute it while reading,
    // like Ruby's anchored titles; an unanchored one waits for conversion.)
    let source = ":n: 1\n\n[.r%o]\n.T {counter:n}\n'''\n\n{n}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    let hr = &blocks[0];
    assert!(matches!(hr.kind, BlockKind::ThematicBreak));
    assert_eq!(
        parse("[#hr]\n'''").document.blocks[0].id.as_deref(),
        Some("hr")
    );
    assert_eq!(hr.roles, ["r"]);
    assert_eq!(hr.attrs.options, ["o"]);
    // Ruby never converts a break's title: the counter keeps its value.
    let title = hr.title.as_ref().unwrap();
    assert!(matches!(&title[0], Inline::Text { value, .. } if value == "T {counter:n}"));
    let BlockKind::Paragraph { inlines } = &blocks[1].kind else {
        panic!("expected a paragraph");
    };
    let text: String = inlines
        .iter()
        .map(|i| match i {
            Inline::Text { value, .. } => value.clone(),
            Inline::AttributeRef { value, .. } => value.clone().unwrap_or_default(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(text, "1");
    // A block title before a break is consumed by it, not carried on.
    let blocks = parse(".T\n'''\n\npara").document.into_blocks();
    assert!(blocks[1].title.is_none());
}

#[test]
fn breaks_are_highlighted_from_the_parser() {
    let source = "'''\n\n<<<\n\npara\n'''\n\n* x\n+\n - - -";
    assert_eq!(
        kinds(source)
            .into_iter()
            .filter(|(k, _)| matches!(k, TokenKind::ThematicBreak | TokenKind::PageBreak))
            .collect::<Vec<_>>(),
        [
            (TokenKind::ThematicBreak, "'''"),
            (TokenKind::PageBreak, "<<<"),
            (TokenKind::ThematicBreak, " - - -"),
        ]
    );
}

#[test]
fn fenced_code_is_a_source_listing() {
    let source = "```\nplain <b>\n```\n\n```ruby\nputs 1 # <1>\n```\n<1> one\n\n``` js , linenums\nx\n```\n\n```,x\ny\n```";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert_eq!(blocks.len(), 5);
    for block in [&blocks[0], &blocks[1], &blocks[3], &blocks[4]] {
        assert_eq!(block.attrs.style.as_deref(), Some("source"));
    }
    assert_eq!(listing(&blocks[0]), "plain <b>");
    assert_eq!(blocks[0].span, Span::new(0, 17));
    assert_eq!(language(&blocks[0]), None);
    assert_eq!(language(&blocks[1]), Some("ruby"));
    match &blocks[1].kind {
        BlockKind::Listing { callouts, .. } => assert_eq!(callouts.len(), 1),
        other => panic!("expected a listing, got {other:?}"),
    }
    assert!(matches!(blocks[2].kind, BlockKind::CalloutList { .. }));
    assert_eq!(language(&blocks[3]), Some("js"));
    assert_eq!(
        blocks[3].attrs.named.get("linenums").map(String::as_str),
        Some("")
    );
    // A leading comma keeps the whole text as the language, like Ruby.
    assert_eq!(language(&blocks[4]), Some(",x"));
    assert_eq!(
        blocks[4].attrs.named.get("linenums").map(String::as_str),
        Some("")
    );
}

#[test]
fn fenced_code_closes_at_three_backticks_and_ignores_styles() {
    let source = "[.r#f]\n.Title\n```py\n````\n```py\nz\n```\n\n````\nnot fenced\n````";
    let blocks = parse(source).document.into_blocks();
    assert_eq!(listing(&blocks[0]), "````\n```py\nz");
    assert_eq!(blocks[0].id.as_deref(), Some("f"));
    assert_eq!(blocks[0].roles, ["r"]);
    assert_eq!(language(&blocks[0]), Some("py"));
    assert!(matches!(blocks[1].kind, BlockKind::Paragraph { .. }));

    // Ruby's fenced code ignores the metadata style and its `source` rekeying,
    // but keeps an explicit named language.
    let blocks = parse(
        "[literal]\n```\na\n```\n\n[source,python]\n```\nb\n```\n\n[language=go]\n```\nc\n```",
    )
    .document
    .into_blocks();
    assert_eq!(listing(&blocks[0]), "a");
    assert_eq!(blocks[0].attrs.style.as_deref(), Some("source"));
    assert_eq!(language(&blocks[1]), None);
    assert_eq!(blocks[1].attrs.positional, ["python"]);
    assert_eq!(language(&blocks[2]), Some("go"));

    let result = parse("```\nunterminated");
    assert_eq!(listing(&result.document.blocks[0]), "unterminated");
    assert_eq!(result.diagnostics[0].severity, Severity::Error);
    assert_eq!(
        result.diagnostics[0].message,
        "unterminated listing block (missing closing `````)"
    );
    assert_eq!(result.diagnostics[0].span, Span::new(0, 3));
}

#[test]
fn fenced_code_works_in_list_continuations_and_is_highlighted() {
    let source = "* item\n+\n```sh\nls *x*\n```";
    let blocks = parse(source).document.into_blocks();
    let BlockKind::List { items, .. } = &blocks[0].kind else {
        panic!("expected a list");
    };
    assert_eq!(listing(&items[0].blocks[0]), "ls *x*");
    assert_eq!(language(&items[0].blocks[0]), Some("sh"));
    let verbatim: Vec<_> = kinds(source)
        .into_iter()
        .filter(|(k, _)| {
            matches!(
                k,
                TokenKind::ListingDelimiter | TokenKind::ListingText | TokenKind::Strong
            )
        })
        .collect();
    assert_eq!(
        verbatim,
        [
            (TokenKind::ListingDelimiter, "```sh"),
            (TokenKind::ListingText, "ls *x*"),
            (TokenKind::ListingDelimiter, "```"),
        ]
    );
}

#[test]
fn verbatim_and_raw_bodies_drop_blank_edge_lines_like_ruby() {
    // Ruby's `Block#content` for verbatim/raw blocks, with default subs too.
    let source = "----\n\ncode <1>\n\nmore\n \t\n----\n<1> x\n\n....\n\nlit\n\n....\n\n++++\n\n<b>\n\n++++\n\n```\n\nx\n\n```";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    match &blocks[0].kind {
        BlockKind::Listing { text, callouts, .. } => {
            assert_eq!(text, "code <1>\n\nmore");
            assert_eq!(callouts.len(), 1);
            assert_eq!(&source[callouts[0].span.start..callouts[0].span.end], "<1>");
        }
        other => panic!("expected a listing, got {other:?}"),
    }
    assert!(matches!(&blocks[2].kind, BlockKind::Literal { text, .. } if text == "lit"));
    assert!(matches!(&blocks[3].kind, BlockKind::Passthrough { text, .. } if text == "<b>"));
    assert_eq!(listing(&blocks[4]), "x");
}
