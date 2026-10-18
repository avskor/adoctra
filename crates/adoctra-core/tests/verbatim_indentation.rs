//! Verbatim paragraph forms of listing/source (strict verbatim paragraphs),
//! the source defaults of Ruby's `:listing`/`:source` branch
//! (`source-language`, `source-linenums-option`, `source-indent`) and
//! `build_block`'s `indent`/`tabsize` adjustment of verbatim content.

use adoctra_core::{parse, tokens, Block, BlockKind, Inline, Span, TokenKind};

fn listing(block: &Block) -> &str {
    match &block.kind {
        BlockKind::Listing { text, .. } => text,
        other => panic!("expected a listing, got {other:?}"),
    }
}

fn literal(block: &Block) -> &str {
    match &block.kind {
        BlockKind::Literal { text, .. } => text,
        other => panic!("expected a literal block, got {other:?}"),
    }
}

fn named<'a>(block: &'a Block, key: &str) -> Option<&'a str> {
    block.attrs.named.get(key).map(String::as_str)
}

/// The flat visible text of converted inlines.
fn flat(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text { value, .. } => out.push_str(value),
            Inline::Substituted { value, .. } => out.push_str(value),
            Inline::AttributeRef { value, .. } => out.push_str(value.as_deref().unwrap_or("")),
            Inline::Strong { children, .. } | Inline::Emphasis { children, .. } => {
                out.push_str(&flat(children));
            }
            other => panic!("unexpected inline {other:?}"),
        }
    }
    out
}

#[test]
fn listing_and_source_paragraphs_are_verbatim_listings() {
    let source = "[source,ruby]\nputs *1*\n// kept\n----\n:attr: x\n* item\nimage::a.png[]\n\n[listing]\nplain\n+\nafter";
    let blocks = parse(source).document.blocks;
    assert_eq!(blocks.len(), 3, "{blocks:?}");
    assert_eq!(
        listing(&blocks[0]),
        "puts *1*\n// kept\n----\n:attr: x\n* item\nimage::a.png[]"
    );
    assert_eq!(blocks[0].attrs.style.as_deref(), Some("source"));
    assert_eq!(named(&blocks[0], "language"), Some("ruby"));
    let start = "[source,ruby]\n".len();
    assert_eq!(
        blocks[0].span,
        Span::new(start, source.find("[]").unwrap() + 2)
    );
    // `[listing]` stays a plain listing and ends at a list continuation.
    assert_eq!(listing(&blocks[1]), "plain");
    assert_eq!(blocks[1].attrs.style.as_deref(), Some("listing"));
    assert!(matches!(blocks[2].kind, BlockKind::Paragraph { .. }));
}

#[test]
fn verbatim_paragraphs_in_list_items_stop_at_siblings() {
    let doc = parse("* item\n+\n[source]\nputs 1\n  puts 2\n* next").document;
    let BlockKind::List { items, .. } = &doc.blocks[0].kind else {
        panic!("{doc:?}")
    };
    assert_eq!(items.len(), 2);
    assert_eq!(listing(&items[0].blocks[0]), "puts 1\n  puts 2");
}

#[test]
fn listing_masquerades_on_open_blocks() {
    let source = "[listing]\n--\n  a\n--";
    let blocks = parse(source).document.blocks;
    assert_eq!(listing(&blocks[0]), "  a");
    assert_eq!(blocks[0].span, Span::new(10, source.len()));
    let kinds: Vec<_> = tokens(source)
        .into_iter()
        .map(|t| (t.kind, &source[t.span.start..t.span.end]))
        .collect();
    assert!(
        kinds.contains(&(TokenKind::OpenDelimiter, "--")),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&(TokenKind::ListingText, "  a")),
        "{kinds:?}"
    );
}

#[test]
fn listings_without_a_style_become_source_with_a_language() {
    let source = ":source-language: ruby\n\n----\na\n----\n\n[listing]\n----\nb\n----\n\n[#x.r]\n----\nc\n----\n\n[foo]\n----\nd\n----\n\n[source]\n....\ne\n....\n\n[source,c]\n----\nf\n----";
    let blocks = parse(source).document.blocks;
    let summary: Vec<_> = blocks
        .iter()
        .map(|b| (b.attrs.style.as_deref(), named(b, "language")))
        .collect();
    assert_eq!(
        summary,
        [
            (Some("source"), Some("ruby")),
            (Some("listing"), None),
            (Some("source"), Some("ruby")),
            (Some("foo"), None),
            (Some("source"), Some("ruby")),
            (Some("source"), Some("c")),
        ]
    );
    assert_eq!(blocks[2].id.as_deref(), Some("x"));
    // A second positional is a language without a document default, and the
    // third positional of a listing turned source is `linenums`.
    let blocks = parse("[,java,on]\n----\nj\n----\n\n[,]\n----\nk\n----")
        .document
        .blocks;
    assert_eq!(blocks[0].attrs.style.as_deref(), Some("source"));
    assert_eq!(named(&blocks[0], "language"), Some("java"));
    assert_eq!(named(&blocks[0], "linenums"), Some("on"));
    assert_eq!(blocks[1].attrs.style, None);
    assert_eq!(named(&blocks[1], "language"), None);
    // An empty document value is a language too.
    let blocks = parse(":source-language:\n\n----\nx\n----").document.blocks;
    assert_eq!(named(&blocks[0], "language"), Some(""));
}

#[test]
fn source_language_follows_body_assignments() {
    let source = "----\na\n----\n\n:source-language: py\n\n----\nb\n----\n\n:source-language!:\n\n[source]\n----\nc\n----";
    let blocks = parse(source).document.blocks;
    let languages: Vec<_> = blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::Listing { .. }))
        .map(|b| named(b, "language"))
        .collect();
    assert_eq!(languages, [None, Some("py"), None]);
}

#[test]
fn fenced_code_takes_the_default_language_over_metadata() {
    let source =
        ":source-language: ruby\n\n[source,language=c]\n```\na\n```\n\n[language=c]\n```go\nb\n```";
    let blocks = parse(source).document.blocks;
    assert_eq!(named(&blocks[0], "language"), Some("ruby"));
    assert_eq!(named(&blocks[1], "language"), Some("go"));
    // Without a default the metadata language stays.
    let blocks = parse("[language=c]\n```\na\n```").document.blocks;
    assert_eq!(named(&blocks[0], "language"), Some("c"));
}

#[test]
fn linenums_come_from_the_option_or_the_document() {
    let blocks = parse("[source%linenums,ruby]\n----\na\n----\n\n[source,ruby,x]\n----\nb\n----\n\n[listing%linenums]\n----\nc\n----\n\n[source,ruby]\n----\nd\n----").document.blocks;
    let linenums: Vec<_> = blocks.iter().map(|b| named(b, "linenums")).collect();
    assert_eq!(linenums, [Some(""), Some("x"), None, None]);
    let source = ":source-linenums-option:\n\n[source]\n----\na\n----\n\n```\nb\n```\n\n[source,ruby,linenums=no]\n----\nc\n----";
    let blocks = parse(source).document.blocks;
    let linenums: Vec<_> = blocks.iter().map(|b| named(b, "linenums")).collect();
    assert_eq!(linenums, [Some(""), Some(""), Some("no")]);
}

#[test]
fn source_indent_applies_to_source_blocks_only() {
    let source = ":source-language: ruby\n:source-indent: 2\n\n----\n    a\n      b\n----\n\n[source,indent=0]\n----\n  c\n----\n\n[listing]\n----\n    d\n----\n\n```\n\te\n```";
    let blocks = parse(source).document.blocks;
    assert_eq!(listing(&blocks[0]), "  a\n    b");
    assert_eq!(named(&blocks[0], "indent"), Some("2"));
    assert_eq!(listing(&blocks[1]), "c");
    assert_eq!(listing(&blocks[2]), "    d");
    assert_eq!(named(&blocks[2], "indent"), None);
    // Without `tabsize` a tab is one indent character.
    assert_eq!(listing(&blocks[3]), "  e");
}

#[test]
fn indent_adjusts_listing_literal_and_verse_forms() {
    let source = "[indent=2]\n----\n    a\n      b\n----\n\n[literal,indent=1]\n   c\n     d\n\n[literal,indent=0]\n----\n  e\n----\n\n[listing,indent=abc]\n--\n  f\n   g\n--\n\n[indent=-1]\n....\n  h\n....\n\n[verse,indent=1]\n____\n   *i*\n     j\n____\n\n[verse,indent=0]\n  k\n\n[verse,indent=2]\n--\nl\n--";
    let blocks = parse(source).document.blocks;
    assert_eq!(listing(&blocks[0]), "  a\n    b");
    assert_eq!(literal(&blocks[1]), " c\n   d");
    assert_eq!(literal(&blocks[2]), "e");
    assert_eq!(listing(&blocks[3]), "f\n g");
    assert_eq!(literal(&blocks[4]), "  h");
    let verses: Vec<_> = blocks[5..]
        .iter()
        .map(|b| match &b.kind {
            BlockKind::Verse { inlines, .. } => flat(inlines),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(verses, [" i\n   j", "k", "  l"]);
}

#[test]
fn indented_literal_paragraphs_ignore_indent_and_tabsize() {
    let blocks = parse(":tabsize: 4\n\n[indent=4]\n  \ta\n   b")
        .document
        .blocks;
    assert_eq!(literal(&blocks[0]), "a\nb");
}

#[test]
fn tabsize_expands_tabs_by_character_columns() {
    let source = ":tabsize: 4\n\n----\n\tЯ\tX\na\tb🙂\tc\n----\n\n[tabsize=2]\n....\n\tx\n....\n\n[tabsize=0]\n----\n\ty\n----\n\n[verse]\n\tv\tw";
    let blocks = parse(source).document.blocks;
    assert_eq!(listing(&blocks[0]), "    Я   X\na   b🙂  c");
    assert_eq!(literal(&blocks[1]), "  x");
    assert_eq!(listing(&blocks[2]), "\ty");
    let BlockKind::Verse { inlines, .. } = &blocks[3].kind else {
        panic!()
    };
    assert_eq!(flat(inlines), "    v   w");
}

#[test]
fn adjusted_verse_and_subs_keep_physical_spans() {
    let source = ":tabsize: 4\n:n: v\n\n[verse,indent=1]\n  {n}\n\t*b*";
    let doc = parse(source).document;
    let BlockKind::Verse { inlines, .. } = &doc.blocks[0].kind else {
        panic!("{doc:?}")
    };
    assert_eq!(flat(inlines), " v\n   b");
    let reference = inlines
        .iter()
        .find_map(|i| match i {
            Inline::AttributeRef { span, .. } => Some(*span),
            _ => None,
        })
        .unwrap();
    assert_eq!(&source[reference.start..reference.end], "{n}");
    let strong = inlines
        .iter()
        .find_map(|i| match i {
            Inline::Strong { span, .. } => Some(*span),
            _ => None,
        })
        .unwrap();
    assert_eq!(&source[strong.start..strong.end], "*b*");

    let source = ":n: v\n\n[subs=+attributes,indent=2]\n----\n    {n}\n----";
    let doc = parse(source).document;
    let BlockKind::Listing {
        inlines: Some(inlines),
        ..
    } = &doc.blocks[0].kind
    else {
        panic!("{doc:?}")
    };
    // The new indent is zero-width: the converted text spans the reference.
    let [Inline::Substituted { value, span, .. }] = inlines.as_slice() else {
        panic!("{inlines:?}")
    };
    assert_eq!(value, "  v");
    assert_eq!(&source[span.start..span.end], "{n}");
}

#[test]
fn callouts_keep_their_source_spans_after_adjustment() {
    let source = ":tabsize: 4\n\n[source,ruby,indent=1]\n----\n    a # <1>\n\tb\t<2>\n----\n<1> one\n<2> two";
    let doc = parse(source).document;
    let BlockKind::Listing { text, callouts, .. } = &doc.blocks[0].kind else {
        panic!("{doc:?}")
    };
    assert_eq!(text, " a # <1>\n b   <2>");
    let marks: Vec<_> = callouts
        .iter()
        .map(|c| (&text[c.start..c.end], &source[c.span.start..c.span.end]))
        .collect();
    assert_eq!(marks, [("# <1>", "# <1>"), ("<2>", "<2>")]);
}

#[test]
fn asciidoc_cells_inherit_tabsize() {
    let source = ":tabsize: 2\n\n|===\na|\n----\n\tcell\n----\n|===";
    let doc = parse(source).document;
    let mut texts = Vec::new();
    for block in doc.flat_blocks() {
        if let BlockKind::Table(table) = &block.kind {
            for row in &table.body {
                for cell in row {
                    if let adoctra_core::ast::CellContent::Blocks { blocks, .. } = &cell.content {
                        texts.extend(blocks.iter().map(|b| listing(b).to_owned()));
                    }
                }
            }
        }
    }
    assert_eq!(texts, ["  cell"]);
}

#[test]
fn verbatim_paragraph_forms_have_no_inline_tokens() {
    for style in ["listing", "source", "literal"] {
        let source = format!("[{style}]\n*x* {{y}}\n:a: b");
        let kinds: Vec<_> = tokens(&source).into_iter().map(|t| t.kind).collect();
        assert_eq!(kinds, [TokenKind::BlockAttributeList], "{style}");
    }
}
