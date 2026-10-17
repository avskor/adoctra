use adoctra_core::{parse, tokens, Block, BlockKind, Inline, Span, TokenKind};

fn heading(block: &Block) -> (u8, &[Inline]) {
    match &block.kind {
        BlockKind::Heading { level, inlines } => (*level, inlines),
        other => panic!("expected a heading, got {other:?}"),
    }
}

fn kinds(source: &str) -> Vec<(TokenKind, &str)> {
    tokens(source)
        .into_iter()
        .map(|t| (t.kind, &source[t.span.start..t.span.end]))
        .collect()
}

#[test]
fn body_setext_section_spans_both_lines_and_keeps_inline_spans() {
    let source = "Intro\n\nSection *One*\n-------------\n\nText";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert_eq!(blocks.len(), 3);
    assert!(!result.document.has_title);
    let (level, inlines) = heading(&blocks[1]);
    assert_eq!(level, 1);
    assert_eq!(blocks[1].span, Span::new(7, 34));
    assert_eq!(blocks[1].id.as_deref(), Some("_section_one"));
    assert!(matches!(&inlines[0], Inline::Text { value, .. } if value == "Section "));
    assert_eq!(inlines[0].span(), Span::new(7, 15));
    assert!(matches!(inlines[1], Inline::Strong { .. }));
    assert_eq!(
        &source[inlines[1].span().start..inlines[1].span().end],
        "*One*"
    );
    assert!(matches!(blocks[2].kind, BlockKind::Paragraph { .. }));
    let highlighted = kinds(source);
    assert!(highlighted.contains(&(TokenKind::Heading, "Section *One*")));
    assert!(highlighted.contains(&(TokenKind::Heading, "-------------")));
}

#[test]
fn underline_selects_level_and_wins_over_block_delimiters() {
    let source =
        "= Doc\n\nList\n----\n\nAb\n--\n\nPart\n====\n\nDeep\n~~~~\n\nNum\n^^^\n\nFive\n++++\n\nx";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    let levels: Vec<_> = blocks[1..7].iter().map(|b| heading(b).0).collect();
    assert_eq!(levels, [1, 1, 0, 2, 3, 4]);
    let ids: Vec<_> = blocks[1..7]
        .iter()
        .map(|b| b.id.as_deref().unwrap())
        .collect();
    assert_eq!(ids, ["_list", "_ab", "_part", "_deep", "_num", "_five"]);
    assert!(matches!(blocks[7].kind, BlockKind::Paragraph { .. }));
    let highlighted = kinds(source);
    for line in ["----", "--", "====", "++++"] {
        assert!(highlighted.contains(&(TokenKind::Heading, line)), "{line}");
    }
    assert!(!highlighted.iter().any(|(kind, _)| matches!(
        kind,
        TokenKind::ListingDelimiter
            | TokenKind::OpenDelimiter
            | TokenKind::ExampleDelimiter
            | TokenKind::PassthroughDelimiter
            | TokenKind::PassthroughText
    )));
}

#[test]
fn leveloffset_applies_and_atx_takes_precedence() {
    let result = parse(":leveloffset: 1\n\nSec\n---\n\n== Atx\n~~~~~~\n\nx");
    let blocks = &result.document.blocks;
    assert_eq!(heading(&blocks[0]).0, 2);
    assert_eq!(heading(&blocks[1]).0, 2);
    assert_eq!(blocks[1].span, Span::new(26, 32));
    assert!(matches!(blocks[2].kind, BlockKind::Paragraph { .. }));
}

#[test]
fn rejected_underlines_stay_paragraph_text() {
    for source in [
        "Title\n---",
        "Title\n---------",
        "Title\n--~--",
        "🙂!\n--",
        "Sec\n\n~~~",
        "Sec\n// comment\n~~~",
        "Sec\n ~~~",
        "Para\nTitle\n~~~~~",
        "Sec",
    ] {
        let result = parse(source);
        assert!(
            !result
                .document
                .blocks
                .iter()
                .any(|b| matches!(b.kind, BlockKind::Heading { .. })),
            "{source:?}: {:?}",
            result.document.blocks
        );
        assert!(!kinds(source).iter().any(|(k, _)| *k == TokenKind::Heading));
    }
}

#[test]
fn containers_and_list_items_only_accept_discrete_setext_titles() {
    let result = parse("====\nInner\n~~~~~\n\n[float]\nIn\n--\n====\n\n* item\n+\nSec\n~~~");
    let BlockKind::Example { blocks } = &result.document.blocks[0].kind else {
        panic!("{:?}", result.document.blocks)
    };
    assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
    assert_eq!(heading(&blocks[1]).0, 1);
    assert_eq!(blocks[1].attrs.style.as_deref(), Some("float"));
    assert_eq!(blocks[1].id.as_deref(), Some("_in"));
    assert!(!result
        .document
        .blocks
        .iter()
        .skip(1)
        .any(|b| matches!(b.kind, BlockKind::Heading { .. })));
}

#[test]
fn anchors_ids_and_block_titles_follow_section_rules() {
    let source = ".Caption\n[[meta]]\nSec\n---\n\nBody\n\nNamed [[sid, Label]]\n--------------------\n\n<<sid>> <<meta>>";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert_eq!(blocks[0].id.as_deref(), Some("meta"));
    assert!(blocks[0].title.is_none());
    assert!(
        blocks[1].title.is_some(),
        "the block title is carried to Body"
    );
    assert_eq!(blocks[2].id.as_deref(), Some("sid"));
    let (_, inlines) = heading(&blocks[2]);
    assert!(matches!(inlines, [Inline::Text { value, .. }] if value == "Named"));
    let BlockKind::Paragraph { inlines } = &blocks[3].kind else {
        panic!()
    };
    let labels: Vec<_> = inlines
        .iter()
        .filter_map(|i| match i {
            Inline::CrossReference { children, .. } => Some(match &children[..] {
                [Inline::Text { value, .. }] => value.as_str(),
                other => panic!("{other:?}"),
            }),
            _ => None,
        })
        .collect();
    assert_eq!(labels, ["Label", "Sec"]);
}

#[test]
fn unicode_crlf_and_eof_titles_use_character_lengths() {
    let source = "Яблоко\r\n------\r\n\r\nЯ🙂 \t\r\n~~\r\n";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert_eq!(heading(&blocks[0]).0, 1);
    assert_eq!(
        &source[blocks[0].span.start..blocks[0].span.end],
        "Яблоко\r\n------"
    );
    assert_eq!(blocks[0].id.as_deref(), Some("_яблоко"));
    let (level, inlines) = heading(&blocks[1]);
    assert_eq!(level, 2);
    assert_eq!(
        &source[inlines[0].span().start..inlines[0].span().end],
        "Я🙂"
    );
    let highlighted = kinds(source);
    assert!(highlighted.contains(&(TokenKind::Heading, "Яблоко")));
    assert!(highlighted.contains(&(TokenKind::Heading, "~~")));
    // An underline at end of input still completes the title.
    assert_eq!(heading(&parse("Sec\n---").document.blocks[0]).0, 1);
}

#[test]
fn comment_style_peeks_the_underline_without_preprocessing() {
    // The comment style does not prevent a section title.
    let result = parse("[comment]\nSec\n---\n\nx");
    assert_eq!(heading(&result.document.blocks[0]).0, 1);
    // Its underline is read raw: the directive is comment text, not a condition.
    let result = parse(":a:\n\n[comment]\nSec\nifdef::a[]\n---\nendif::[]\n\nx");
    assert_eq!(
        result.document.blocks.len(),
        1,
        "{:?}",
        result.document.blocks
    );
    assert!(matches!(
        result.document.blocks[0].kind,
        BlockKind::Paragraph { .. }
    ));
    assert!(result.conditional_directives.is_empty());
}
