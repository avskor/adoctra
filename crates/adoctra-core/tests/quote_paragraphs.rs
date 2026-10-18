//! Ruby's Markdown blockquote (`> `) and quoted paragraph (`"…"` with a
//! `-- credit` line) from its ordinary paragraph branch in `next_block`.

use adoctra_core::ast::Attribution;
use adoctra_core::{parse, tokens, Block, BlockKind, Inline, Severity, Span, TokenKind};

fn quote(block: &Block) -> (Option<&Attribution>, &[Block]) {
    match &block.kind {
        BlockKind::Quote {
            attribution,
            blocks,
        } => (attribution.as_ref(), blocks),
        other => panic!("expected a quote, got {other:?}"),
    }
}

fn text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { value, .. } | Inline::Raw { value, .. } => value.clone(),
            Inline::AttributeRef { value, .. } => value.clone().unwrap_or_default(),
            Inline::Strong { children, .. } => format!("<{}>", text(children)),
            Inline::CrossReference { children, .. } => format!("[{}]", text(children)),
            other => format!("{other:?}"),
        })
        .collect()
}

fn credit(block: &Block) -> (Option<String>, Option<String>) {
    let attribution = quote(block).0.expect("an attribution");
    (
        attribution.author.as_deref().map(text),
        attribution.citetitle.as_deref().map(text),
    )
}

fn paragraph(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines } => inlines,
        other => panic!("expected a paragraph, got {other:?}"),
    }
}

#[test]
fn markdown_blockquote_parses_its_lines_as_blocks() {
    let source = "> para\nlazy\n>\n> * item\n> ----\n> code\n> ----\n> -- Ann, *Work*\n";
    let blocks = parse(source).document.blocks;
    let [block] = blocks.as_slice() else {
        panic!("one block expected: {blocks:?}");
    };
    assert_eq!(block.span, Span::new(0, source.len() - 1));
    assert_eq!(block.attrs.style.as_deref(), Some("quote"));
    assert!(!block.simple);
    let (_, children) = quote(block);
    assert_eq!(children.len(), 3);
    // Marker-stripped lines keep their physical offsets; a lazy line has none.
    assert_eq!(
        &source[children[0].span.start..children[0].span.end],
        "para\nlazy"
    );
    assert_eq!(text(paragraph(&children[0])), "para lazy");
    assert!(matches!(children[1].kind, BlockKind::List { .. }));
    assert_eq!(
        &source[children[1].span.start..children[1].span.end],
        "* item"
    );
    let BlockKind::Listing { text: code, .. } = &children[2].kind else {
        panic!("expected a listing: {:?}", children[2].kind);
    };
    assert_eq!(code, "code");
    assert_eq!(credit(block), (Some("Ann".into()), Some("<Work>".into())));
    let attribution = quote(block).0.unwrap();
    let Inline::Text { span, .. } = &attribution.author.as_deref().unwrap()[0] else {
        panic!("expected text");
    };
    assert_eq!(&source[span.start..span.end], "Ann");
}

#[test]
fn nested_blockquotes_and_sections_follow_ruby() {
    // A nested quote needs a new paragraph; a section title is text.
    let source = "> a\n>\n> > b\n> > -- In\n>\n> == Not a section\n";
    let blocks = parse(source).document.blocks;
    let (attribution, children) = quote(&blocks[0]);
    assert!(attribution.is_none());
    assert_eq!(children.len(), 3);
    assert_eq!(credit(&children[1]), (Some("In".into()), None));
    let (_, inner) = quote(&children[1]);
    assert_eq!(&source[inner[0].span.start..inner[0].span.end], "b");
    assert!(matches!(children[2].kind, BlockKind::Paragraph { .. }));
}

#[test]
fn credit_lines_split_like_ruby() {
    let source = ":who: Ann, Book\n\n> a\n> -- A\n\n> b\n> -- , W\n\n> c\n> -- A, \n\n\
                  > d\n> -- A, W, more\n\n> e\n> -- {who}\n\n> f\n>\n>\n> -- blanks\n\n\
                  > -- only\n\n> g\n> --no space\n";
    let blocks = parse(source).document.blocks;
    assert_eq!(credit(&blocks[0]), (Some("A".into()), None));
    assert_eq!(credit(&blocks[1]), (Some("".into()), Some("W".into())));
    // Reader lines are right-stripped, so `A, ` keeps its comma.
    assert_eq!(credit(&blocks[2]), (Some("A,".into()), None));
    assert_eq!(
        credit(&blocks[3]),
        (Some("A".into()), Some("W, more".into()))
    );
    // A plain attribute value splits too, like Ruby's converted string.
    assert_eq!(
        credit(&blocks[4]),
        (Some("Ann".into()), Some("Book".into()))
    );
    // Blank marker lines before the credit are dropped.
    assert_eq!(quote(&blocks[5]).1.len(), 1);
    assert_eq!(credit(&blocks[5]), (Some("blanks".into()), None));
    let (_, children) = quote(&blocks[6]);
    assert!(children.is_empty());
    assert_eq!(credit(&blocks[6]), (Some("only".into()), None));
    assert!(quote(&blocks[7]).0.is_none());
    assert_eq!(text(paragraph(&quote(&blocks[7]).1[0])), "g --no space");
}

#[test]
fn credit_lines_execute_while_reading() {
    // The credit converts while reading; like Ruby, conversion restarts
    // from the header attributes.
    let source = ":n: 0\n\n> x\n> -- {counter:n} <<t>>\n\n{n}\n\n[[t]]\nT\n";
    let result = parse(source);
    let blocks = &result.document.blocks;
    assert_eq!(credit(&blocks[0]), (Some("1 [[t]]".into()), None));
    assert_eq!(text(paragraph(&blocks[1])), "0");
}

#[test]
fn quoted_paragraphs_are_simple_quotes() {
    let source = ".T\n[#q.r]\n\"one\ntwo\"\n// note\n-- Ann, Work\n";
    let result = parse(source);
    let [block] = result.document.blocks.as_slice() else {
        panic!("one block expected");
    };
    assert!(block.simple);
    assert_eq!(block.id.as_deref(), Some("q"));
    assert_eq!(block.roles, ["r"]);
    assert_eq!(block.attrs.style.as_deref(), Some("quote"));
    let (_, children) = quote(block);
    let [child] = children else {
        panic!("one paragraph expected");
    };
    assert_eq!(&source[child.span.start..child.span.end], "one\ntwo");
    assert_eq!(credit(block), (Some("Ann".into()), Some("Work".into())));
    assert!(result
        .comments
        .iter()
        .any(|span| &source[span.start..span.end] == "// note"));
}

#[test]
fn quote_forms_need_an_unstyled_paragraph() {
    let source = "[quote, Q]\n> styled\n\n[NOTE]\n> note\n\n[foo, A, B]\n> unknown\n\n\
                  [normal]\n\"n\"\n-- N\n\n* item\n> folded\n\n\"open\nend\n-- A\n\n\
                  >no space\n";
    let blocks = parse(source).document.blocks;
    // A paragraph style keeps the markers as text.
    let (attribution, children) = quote(&blocks[0]);
    assert_eq!(text(attribution.unwrap().author.as_deref().unwrap()), "Q");
    assert_eq!(text(paragraph(&children[0])), "> styled");
    assert!(matches!(blocks[1].kind, BlockKind::Admonition { .. }));
    // An unknown style is replaced; its positional fields do not rekey.
    assert_eq!(blocks[2].attrs.style.as_deref(), Some("quote"));
    assert!(quote(&blocks[2]).0.is_none());
    assert_eq!(blocks[2].attrs.positional, ["A", "B"]);
    assert_eq!(credit(&blocks[3]), (Some("N".into()), None));
    let BlockKind::List { items, .. } = &blocks[4].kind else {
        panic!("expected a list");
    };
    assert_eq!(text(&items[0].inlines), "item > folded");
    assert!(matches!(blocks[5].kind, BlockKind::Paragraph { .. }));
    assert!(matches!(blocks[6].kind, BlockKind::Paragraph { .. }));
}

#[test]
fn blockquote_anchors_catalog_twice_like_ruby() {
    let source = "> see [[a,Ref]]\n\n<<a>>\n";
    let result = parse(source);
    let BlockKind::Paragraph { inlines } = &result.document.blocks[1].kind else {
        panic!("expected a paragraph");
    };
    assert_eq!(text(inlines), "[Ref]");
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(
        &source[warnings[0].span.start..warnings[0].span.end],
        "[[a,Ref]]"
    );
}

#[test]
fn blockquote_markers_are_quote_delimiter_tokens() {
    let source = "> *a*\n>\n> > b\n>\n> ----\n> x\n> ----\n";
    let found: Vec<_> = tokens(source)
        .into_iter()
        .map(|t| (t.kind, &source[t.span.start..t.span.end], t.span.start))
        .collect();
    let markers: Vec<_> = found
        .iter()
        .filter(|(kind, ..)| *kind == TokenKind::QuoteDelimiter)
        .map(|(_, _, start)| *start)
        .collect();
    assert_eq!(markers, [0, 6, 8, 10, 14, 16, 23, 27]);
    assert!(found.contains(&(TokenKind::Strong, "*a*", 2)));
    assert!(found.contains(&(TokenKind::ListingDelimiter, "----", 18)));
    assert!(found.contains(&(TokenKind::ListingText, "x", 25)));
    assert!(found.contains(&(TokenKind::ListingDelimiter, "----", 29)));
}

#[test]
fn reading_attributions_label_their_references() {
    // Attribute-line and credit attributions convert while reading: an
    // earlier section lends its title, a later one shows `[id]`.
    let source = "== Sec\n\n[quote, '<<_sec>> <<_later>>']\n____\nx\n____\n\n\
                  > y\n> -- <<_sec>>, <<_later>>\n\n== Later\n";
    let blocks = parse(source).document.blocks;
    let BlockKind::Section { blocks, .. } = &blocks[0].kind else {
        panic!("expected a section");
    };
    assert_eq!(credit(&blocks[1]), (Some("[Sec] [[_later]]".into()), None));
    assert_eq!(
        credit(&blocks[2]),
        (Some("[Sec]".into()), Some("[[_later]]".into()))
    );
}
