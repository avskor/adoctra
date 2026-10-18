//! Delimited blocks with fences longer than the minimum (Ruby's
//! `is_delimited_block?`): only the identical line closes a block, so blocks
//! of one kind nest by fence length, and highlighting follows the parser.

use adoctra_core::{parse, tokens, Block, BlockKind, Inline, Severity, Span, TokenKind};

fn kinds(source: &str) -> Vec<(TokenKind, &str)> {
    tokens(source)
        .into_iter()
        .map(|t| (t.kind, &source[t.span.start..t.span.end]))
        .collect()
}

fn children(block: &Block) -> &[Block] {
    match &block.kind {
        BlockKind::Example { blocks }
        | BlockKind::Sidebar { blocks }
        | BlockKind::Quote { blocks, .. }
        | BlockKind::Open { blocks } => blocks,
        other => panic!("expected a container, got {other:?}"),
    }
}

fn text(block: &Block) -> String {
    match &block.kind {
        BlockKind::Paragraph { inlines } => inlines
            .iter()
            .map(|inline| match inline {
                Inline::Text { value, .. } => value.as_str(),
                _ => "",
            })
            .collect(),
        other => panic!("expected a paragraph, got {other:?}"),
    }
}

#[test]
fn containers_of_one_kind_nest_by_fence_length() {
    for (fence, inner) in [("======", "===="), ("*****", "****"), ("_____", "____")] {
        let source = format!("{fence}\nouter\n\n{inner}\ninner\n{inner}\n\nafter\n{fence}\n");
        let result = parse(&source);
        assert!(
            result.diagnostics.is_empty(),
            "{fence}: {:?}",
            result.diagnostics
        );
        let blocks = &result.document.blocks;
        assert_eq!(blocks.len(), 1, "{fence}");
        let outer = &blocks[0];
        assert_eq!(outer.span, Span::new(0, source.len() - 1), "{fence}");
        let kids = children(outer);
        assert_eq!(kids.len(), 3, "{fence}");
        assert_eq!(text(&kids[0]), "outer");
        assert_eq!(text(&kids[2]), "after");
        let start = source.find(&format!("\n{inner}\n")).unwrap() + 1;
        let end = source.rfind(&format!("\n{inner}\n")).unwrap() + 1 + inner.len();
        assert_eq!(kids[1].span, Span::new(start, end), "{fence}");
        assert_eq!(
            std::mem::discriminant(&kids[1].kind),
            std::mem::discriminant(&outer.kind)
        );
        assert_eq!(text(&children(&kids[1])[0]), "inner");
    }
}

#[test]
fn verbatim_blocks_keep_shorter_fences_as_text() {
    let source =
        "------\ncode\n----\nmore\n------\n\n.....\nlit\n....\n.....\n\n+++++\n<b>\n++++\n+++++";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert_eq!(blocks.len(), 3);
    match &blocks[0].kind {
        BlockKind::Listing { text, .. } => assert_eq!(text, "code\n----\nmore"),
        other => panic!("expected a listing, got {other:?}"),
    }
    assert_eq!(blocks[0].span, Span::new(0, 28));
    match &blocks[1].kind {
        BlockKind::Literal { text, .. } => assert_eq!(text, "lit\n...."),
        other => panic!("expected a literal, got {other:?}"),
    }
    match &blocks[2].kind {
        BlockKind::Passthrough { text, .. } => assert_eq!(text, "<b>\n++++"),
        other => panic!("expected a passthrough, got {other:?}"),
    }
    assert_eq!(blocks[2].span.end, source.len());
}

#[test]
fn verse_and_open_masquerades_close_only_at_their_own_fence() {
    let source = "[verse]\n_____\nline\n____\n_____\n\n[source]\n--\nx\n---\n--";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = result.document.blocks;
    assert_eq!(blocks.len(), 2);
    // The inner `____` is verse text (Ruby's constrained emphasis `<em>__</em>`).
    match &blocks[0].kind {
        BlockKind::Verse { inlines, .. } => {
            assert!(matches!(&inlines[0], Inline::Text { value, .. } if value == "line\n"));
            let Inline::Emphasis { span, .. } = &inlines[1] else {
                panic!("expected emphasis, got {:?}", inlines[1]);
            };
            assert_eq!(*span, Span::new(19, 23));
        }
        other => panic!("expected a verse, got {other:?}"),
    }
    assert_eq!(blocks[0].span, Span::new(8, 29));
    match &blocks[1].kind {
        BlockKind::Listing { text, .. } => assert_eq!(text, "x\n---"),
        other => panic!("expected a listing, got {other:?}"),
    }
}

#[test]
fn indented_and_short_fences_are_not_delimiters() {
    // Ruby checks the unindented line: an indented fence is a literal paragraph.
    let blocks = parse(" ----\nindented").document.blocks;
    assert_eq!(blocks.len(), 1);
    match &blocks[0].kind {
        BlockKind::Literal { text, .. } => assert_eq!(text, " ----\nindented"),
        other => panic!("expected a literal paragraph, got {other:?}"),
    }
    // A three-character run is never a fence; a mixed run is not uniform.
    for line in ["===", "***", "====-"] {
        let blocks = parse(&format!("{line}\ntext")).document.blocks;
        assert!(
            matches!(blocks[0].kind, BlockKind::Paragraph { .. }),
            "{line}: {:?}",
            blocks[0].kind
        );
    }
}

#[test]
fn unterminated_long_fences_report_the_missing_fence() {
    let result = parse("======\nbody\n====\nnested");
    let messages: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| (d.severity, d.span, d.message.as_str()))
        .collect();
    assert_eq!(
        messages,
        [
            (
                Severity::Error,
                Span::new(12, 16),
                "unterminated example block (missing closing `====`)"
            ),
            (
                Severity::Error,
                Span::new(0, 6),
                "unterminated example block (missing closing `======`)"
            ),
        ]
    );
    let outer = &result.document.blocks[0];
    assert_eq!(outer.span, Span::new(0, 23));
    assert!(matches!(children(outer)[1].kind, BlockKind::Example { .. }));

    let result = parse("-----\ncode");
    assert_eq!(
        result.diagnostics[0].message,
        "unterminated listing block (missing closing `-----`)"
    );
    assert_eq!(result.diagnostics[0].span, Span::new(0, 5));
}

#[test]
fn styled_paragraphs_end_at_fences_and_attribute_lines_unless_verbatim() {
    // Ruby's `read_paragraph_lines` (StartOfBlockProc) for styled paragraphs.
    let blocks = parse("[quote]\nq1\n=====\nexample\n=====\n\n[quote]\nq2\n[.r]\nnext")
        .document
        .blocks;
    assert_eq!(blocks.len(), 4);
    assert!(matches!(blocks[0].kind, BlockKind::Quote { .. }));
    assert_eq!(blocks[0].span, Span::new(8, 10));
    assert!(matches!(blocks[1].kind, BlockKind::Example { .. }));
    assert!(matches!(blocks[2].kind, BlockKind::Quote { .. }));
    assert_eq!(blocks[3].roles, ["r"]);
    assert_eq!(text(&blocks[3]), "next");

    // A verbatim paragraph ends only at a blank line or a list continuation.
    let source = "[literal]\npara\n------\nmore\n====\n\nafter";
    let blocks = parse(source).document.blocks;
    assert_eq!(blocks.len(), 2);
    match &blocks[0].kind {
        BlockKind::Literal { text, .. } => assert_eq!(text, "para\n------\nmore\n===="),
        other => panic!("expected a literal paragraph, got {other:?}"),
    }
}

#[test]
fn long_fences_work_in_list_continuations_and_table_cells() {
    let source = "* item\n+\n======\n====\nx\n====\n======\n";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let BlockKind::List { items, .. } = &result.document.blocks[0].kind else {
        panic!("expected a list");
    };
    let outer = &items[0].blocks[0];
    assert_eq!(outer.span, Span::new(9, source.len() - 1));
    assert!(matches!(children(outer)[0].kind, BlockKind::Example { .. }));

    let source = "|===\na|\n*****\n****\ncell\n****\n*****\n|===";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let tokens = kinds(source);
    let fences: Vec<_> = tokens
        .iter()
        .filter(|(kind, _)| *kind == TokenKind::SidebarDelimiter)
        .map(|(_, text)| *text)
        .collect();
    assert_eq!(fences, ["*****", "****", "****", "*****"]);
}

#[test]
fn highlighting_follows_the_parser_for_long_fences() {
    let source = "======\n*a*\n====\n*b*\n====\n======\n\n------\n*code*\n----\n------\n\n[source]\n--\n*x*\n--";
    let tokens = kinds(source);
    let delimiters: Vec<_> = tokens
        .iter()
        .filter(|(kind, _)| {
            matches!(
                kind,
                TokenKind::ExampleDelimiter
                    | TokenKind::ListingDelimiter
                    | TokenKind::OpenDelimiter
            )
        })
        .copied()
        .collect();
    assert_eq!(
        delimiters,
        [
            (TokenKind::ExampleDelimiter, "======"),
            (TokenKind::ExampleDelimiter, "===="),
            (TokenKind::ExampleDelimiter, "===="),
            (TokenKind::ExampleDelimiter, "======"),
            (TokenKind::ListingDelimiter, "------"),
            (TokenKind::ListingDelimiter, "------"),
            (TokenKind::OpenDelimiter, "--"),
            (TokenKind::OpenDelimiter, "--"),
        ]
    );
    // Container bodies keep inline tokens; verbatim bodies (including a
    // `[source]` open block) are text, and a shorter fence inside is text.
    let strong: Vec<_> = tokens
        .iter()
        .filter(|(kind, _)| *kind == TokenKind::Strong)
        .map(|(_, text)| *text)
        .collect();
    assert_eq!(strong, ["*a*", "*b*"]);
    let listing: Vec<_> = tokens
        .iter()
        .filter(|(kind, _)| *kind == TokenKind::ListingText)
        .map(|(_, text)| *text)
        .collect();
    assert_eq!(listing, ["*code*", "----", "*x*"]);
}

#[test]
fn listing_and_literal_fences_masquerade_like_ruby() {
    // Ruby's DELIMITED_BLOCKS: `----` accepts literal/source, `....`
    // listing/source; the fence still decides where the body ends.
    let source = "[literal]\n------\nx\n----\n------\n\n[listing]\n.....\ny\n....\n.....\n\n[source,ruby]\n....\nputs 1\n....";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert_eq!(blocks.len(), 3);
    match &blocks[0].kind {
        BlockKind::Literal { text, .. } => assert_eq!(text, "x\n----"),
        other => panic!("expected a literal, got {other:?}"),
    }
    assert_eq!(blocks[0].span, Span::new(10, 30));
    match &blocks[1].kind {
        BlockKind::Listing { text, .. } => assert_eq!(text, "y\n...."),
        other => panic!("expected a listing, got {other:?}"),
    }
    match &blocks[2].kind {
        BlockKind::Listing { text, .. } => assert_eq!(text, "puts 1"),
        other => panic!("expected a source listing, got {other:?}"),
    }
    assert_eq!(blocks[2].attrs.style.as_deref(), Some("source"));
    assert_eq!(
        blocks[2].attrs.named.get("language").map(String::as_str),
        Some("ruby")
    );
    let result = parse("[literal]\n----\ncode");
    assert_eq!(
        result.diagnostics[0].message,
        "unterminated literal block (missing closing `----`)"
    );

    // Highlighting uses the masqueraded kind for fences and body.
    let tokens = kinds(source);
    let verbatim: Vec<_> = tokens
        .iter()
        .filter(|(kind, _)| {
            matches!(
                kind,
                TokenKind::LiteralDelimiter
                    | TokenKind::LiteralText
                    | TokenKind::ListingDelimiter
                    | TokenKind::ListingText
            )
        })
        .copied()
        .collect();
    assert_eq!(
        verbatim,
        [
            (TokenKind::LiteralDelimiter, "------"),
            (TokenKind::LiteralText, "x"),
            (TokenKind::LiteralText, "----"),
            (TokenKind::LiteralDelimiter, "------"),
            (TokenKind::ListingDelimiter, "....."),
            (TokenKind::ListingText, "y"),
            (TokenKind::ListingText, "...."),
            (TokenKind::ListingDelimiter, "....."),
            (TokenKind::ListingDelimiter, "...."),
            (TokenKind::ListingText, "puts 1"),
            (TokenKind::ListingDelimiter, "...."),
        ]
    );
}

#[test]
fn long_fences_keep_physical_spans_through_includes() {
    use adoctra_core::{
        parse_resource, tokens_resource, MemoryResources, ParseOptions, SourceId, SourceMap,
    };
    let mut resources = MemoryResources::new();
    resources
        .insert("/docs/main.adoc", "======\ninclude::part.adoc[]\n======\n")
        .unwrap();
    resources
        .insert(
            "/docs/part.adoc",
            "====\ninner\n====\n\n-----\ncode\n----\n-----\n",
        )
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let outer = &result.document.blocks[0];
    let kids = children(outer);
    assert_eq!(kids.len(), 2);
    assert!(matches!(kids[0].kind, BlockKind::Example { .. }));
    assert_eq!(kids[0].span.source_id, SourceId(1));
    assert_eq!(sources.slice(kids[0].span), Some("====\ninner\n===="));
    match &kids[1].kind {
        BlockKind::Listing { text, .. } => assert_eq!(text, "code\n----"),
        other => panic!("expected a listing, got {other:?}"),
    }
    let fences: Vec<_> = tokens_resource(&mut sources, &options, &resources)
        .unwrap()
        .into_iter()
        .filter(|t| {
            matches!(
                t.kind,
                TokenKind::ExampleDelimiter | TokenKind::ListingDelimiter | TokenKind::ListingText
            )
        })
        .map(|t| {
            (
                t.kind,
                t.span.source_id,
                sources.slice(t.span).unwrap().to_owned(),
            )
        })
        .collect();
    let main = SourceId(0);
    let part = SourceId(1);
    assert_eq!(
        fences,
        [
            // Tokens are ordered by source, then offset.
            (TokenKind::ExampleDelimiter, main, "======".to_owned()),
            (TokenKind::ExampleDelimiter, main, "======".to_owned()),
            (TokenKind::ExampleDelimiter, part, "====".to_owned()),
            (TokenKind::ExampleDelimiter, part, "====".to_owned()),
            (TokenKind::ListingDelimiter, part, "-----".to_owned()),
            (TokenKind::ListingText, part, "code".to_owned()),
            (TokenKind::ListingText, part, "----".to_owned()),
            (TokenKind::ListingDelimiter, part, "-----".to_owned()),
        ]
    );
}
