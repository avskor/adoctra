//! Table of contents: Ruby's `toc` header normalization (`save_attributes`)
//! and the `toc::[]` block macro.
use adoctra_core::{
    parse, parse_with_options, tokens, Block, BlockKind, Inline, ParseOptions, TokenKind,
};

/// The effective header attributes of interest, `-` when unset.
fn toc_attributes(source: &str) -> [String; 5] {
    let doc = parse(source).document;
    [
        "toc",
        "toc-placement",
        "toc-position",
        "toc-class",
        "toc-title",
    ]
    .map(|name| doc.attributes.get(name).unwrap_or("-").to_string())
}

fn blocks(source: &str) -> Vec<Block> {
    parse(source).document.into_flat_blocks()
}

#[test]
fn defaults_and_header_normalization_follow_save_attributes() {
    // Without `toc`, only Ruby's defaults are present.
    assert_eq!(
        toc_attributes("= D"),
        ["-", "auto", "-", "-", "Table of Contents"]
    );
    // An empty `toc` keeps the automatic placement without a position.
    assert_eq!(
        toc_attributes("= D\n:toc:"),
        ["", "auto", "-", "-", "Table of Contents"]
    );
    for (value, position) in [
        ("left", "left"),
        ("<", "left"),
        ("right", "right"),
        (">", "right"),
        ("top", "top"),
        ("^", "top"),
        ("bottom", "bottom"),
        ("v", "bottom"),
    ] {
        assert_eq!(
            toc_attributes(&format!("= D\n:toc: {value}")),
            ["", "auto", position, "toc2", "Table of Contents"],
            "{value}"
        );
    }
    // `toc2` is an alias of `toc: left` and is removed.
    let doc = parse("= D\n:toc2:").document;
    assert_eq!(doc.attributes.get("toc2"), None);
    assert_eq!(doc.attributes.get("toc-position"), Some("left"));
    // A custom class is kept.
    assert_eq!(
        toc_attributes("= D\n:toc: right\n:toc-class: mine")[3],
        "mine"
    );
    for placement in ["preamble", "macro"] {
        assert_eq!(
            toc_attributes(&format!("= D\n:toc: {placement}")),
            ["", placement, "content", "-", "Table of Contents"]
        );
    }
    // An unknown value only keeps the automatic placement; an explicit
    // position wins over the `toc` value.
    assert_eq!(
        toc_attributes("= D\n:toc: foo"),
        ["", "auto", "-", "-", "Table of Contents"]
    );
    assert_eq!(
        toc_attributes("= D\n:toc: foo\n:toc-position: left"),
        ["", "auto", "left", "toc2", "Table of Contents"]
    );
    // An explicit placement names the position; an unset one means macro.
    assert_eq!(
        toc_attributes("= D\n:toc:\n:toc-placement: preamble")[1],
        "preamble"
    );
    assert_eq!(
        toc_attributes("= D\n:toc-placement!:\n:toc:")[..3],
        ["", "macro", "content"]
    );
    // Body entries are not normalized: the header state decides.
    assert_eq!(
        toc_attributes("= D\n\n:toc: left\n\ntext")[..3],
        ["-", "auto", "-"]
    );
    // References in the body see the normalized values.
    let doc = parse("= D\n:toc: left\n\n{toc-position} {toc-class}").document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[1].kind else {
        panic!("paragraph")
    };
    let text: String = inlines
        .iter()
        .map(|node| match node {
            Inline::Text { value, .. } => value.clone(),
            Inline::AttributeRef { value, .. } => value.clone().unwrap_or_default(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(text, "left toc2");
}

#[test]
fn derived_toc_values_bypass_host_locks() {
    let mut options = ParseOptions::default();
    options.attributes.set("toc", "left");
    options.attributes.set("toc-placement", "auto");
    let doc = parse_with_options("= D\n:toc: macro", &options).document;
    assert_eq!(doc.attributes.get("toc"), Some(""));
    assert_eq!(doc.attributes.get("toc-position"), Some("left"));
}

#[test]
fn toc_macro_is_a_block_with_merged_metadata() {
    let source = "= D\n\n.Old\n[[a]]\n[.r1,levels=3]\ntoc::[id=b,role=\"x y\",title=New,levels=1,foo=bar]\n\ntext\ntoc::[]";
    let found = blocks(source);
    let toc = &found[1];
    assert!(matches!(toc.kind, BlockKind::Toc));
    assert_eq!(
        &source[toc.span.start..toc.span.end],
        "toc::[id=b,role=\"x y\",title=New,levels=1,foo=bar]"
    );
    // The macro's fields replace the metadata lines, like Ruby's merge.
    assert_eq!(toc.id.as_deref(), Some("b"));
    assert_eq!(toc.roles, ["x", "y"]);
    assert_eq!(toc.attrs.named.get("levels").map(String::as_str), Some("1"));
    assert_eq!(toc.attrs.named.get("foo").map(String::as_str), Some("bar"));
    let title = toc.title.as_ref().expect("title");
    assert!(
        matches!(title.as_slice(), [Inline::Text { value, span }]
            if value == "New" && &source[span.start..span.end] == "New"),
        "{title:?}"
    );
    // Inside a paragraph the macro is text.
    assert!(matches!(found[2].kind, BlockKind::Paragraph { .. }));
    assert_eq!(found.len(), 3);
    // Metadata alone stays on the block.
    let found = blocks("[[a]]\n.T\ntoc::[]");
    assert_eq!(found[0].id.as_deref(), Some("a"));
    assert!(found[0].title.is_some());
    // Leading spaces or text after the brackets do not form the macro.
    for source in [" toc::[]", "toc::[] x", "toc::", "toc:[]"] {
        assert!(
            !blocks(source)
                .iter()
                .any(|block| matches!(block.kind, BlockKind::Toc)),
            "{source}"
        );
    }
}

#[test]
fn toc_macro_works_in_containers_and_list_continuations() {
    let found = blocks("====\ntoc::[levels=2]\n====\n\n* item\n+\ntoc::[id=t]");
    let BlockKind::Example { blocks } = &found[0].kind else {
        panic!("example")
    };
    assert!(matches!(blocks[0].kind, BlockKind::Toc));
    let BlockKind::List { items, .. } = &found[1].kind else {
        panic!("list")
    };
    assert!(matches!(items[0].blocks[0].kind, BlockKind::Toc));
    assert_eq!(items[0].blocks[0].id.as_deref(), Some("t"));
}

#[test]
fn toc_macro_lines_are_block_macro_tokens() {
    let source = "= D\n\ntoc::[levels=1]\n\ntext\ntoc::[]";
    let found: Vec<_> = tokens(source)
        .into_iter()
        .filter(|token| token.kind == TokenKind::BlockMacro)
        .map(|token| &source[token.span.start..token.span.end])
        .collect();
    assert_eq!(found, ["toc::[levels=1]"]);
    assert_eq!(TokenKind::BlockMacro.as_str(), "block-macro");
}
