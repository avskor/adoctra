//! Callouts: Ruby's verbatim `callouts` substitution (`catalog_callouts`,
//! `sub_callouts`) and callout lists (`parse_callout_list`).
use adoctra_core::ast::Callout;
use adoctra_core::{parse, tokens, Block, BlockKind, Severity, TokenKind};

fn callouts(block: &Block) -> (&str, &[Callout]) {
    match &block.kind {
        BlockKind::Listing { text, callouts, .. } | BlockKind::Literal { text, callouts, .. } => {
            (text, callouts)
        }
        other => panic!("not verbatim: {other:?}"),
    }
}

#[test]
fn marks_are_cataloged_with_guards_ids_and_source_spans() {
    let source = "----\nputs 1 # <1>\nputs 2 // <2> <3>\nx = \\<4>\ny <!--5-->\nz <.>\n----\n<1> one\n<2> two\n<.> three\n<5> five\n";
    let result = parse(source);
    let blocks = result.document.into_flat_blocks();
    let (text, marks) = callouts(&blocks[0]);
    // An escaped mark loses its backslash and stays text.
    assert_eq!(
        text,
        "puts 1 # <1>\nputs 2 // <2> <3>\nx = <4>\ny <!--5-->\nz <.>"
    );
    let found: Vec<_> = marks
        .iter()
        .map(|c| {
            (
                c.number.as_str(),
                c.guard.as_deref(),
                c.xml,
                c.id.as_str(),
                &text[c.start..c.end],
                &source[c.span.start..c.span.end],
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("1", Some("# "), false, "CO1-1", "# <1>", "# <1>"),
            ("2", Some("// "), false, "CO1-2", "// <2>", "// <2>"),
            ("3", None, false, "CO1-3", "<3>", "<3>"),
            ("5", None, true, "CO1-4", "<!--5-->", "<!--5-->"),
            ("1", None, false, "CO1-5", "<.>", "<.>"),
        ]
    );
    let BlockKind::CalloutList { items } = &blocks[1].kind else {
        panic!()
    };
    let coids: Vec<_> = items.iter().map(|i| i.coids.join(" ")).collect();
    assert_eq!(coids, ["CO1-1 CO1-5", "CO1-2", "CO1-3", ""]);
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| (d.severity, d.message.as_str()))
        .collect();
    assert_eq!(
        warnings,
        [
            (
                Severity::Warning,
                "callout list item index: expected 3, got 1"
            ),
            (
                Severity::Warning,
                "callout list item index: expected 4, got 5"
            ),
            (Severity::Warning, "no callout found for <4>"),
        ]
    );
}

#[test]
fn literal_paragraphs_and_line_comment_attribute() {
    let blocks = parse(
        "  indented <1>\n\n[source,erlang,line-comment=%]\n----\nx. % <1>\ny. <!--2-->\n----\n",
    )
    .document
    .into_flat_blocks();
    assert_eq!(callouts(&blocks[0]).1.len(), 1);
    let (_, marks) = callouts(&blocks[1]);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].guard.as_deref(), Some("% "));
}

#[test]
fn callout_lists_take_blocks_and_advance_the_catalog() {
    let result = parse("----\na <1>\n----\n<1> first\n+\n----\nb <1>\n----\n\n<1> next list\n");
    let messages: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    // After a blank line the next item continues the same list (Ruby).
    assert_eq!(
        messages,
        [
            "callout list item index: expected 2, got 1",
            "no callout found for <2>"
        ]
    );
    let blocks = result.document.into_flat_blocks();
    let BlockKind::CalloutList { items } = &blocks[1].kind else {
        panic!()
    };
    // Like Ruby, a listing attached to the item registers in the same list
    // before the item's marks are looked up.
    assert_eq!(items[0].coids, ["CO1-1", "CO1-2"]);
    assert_eq!(callouts(&items[0].item.blocks[0]).1[0].id, "CO1-2");
    assert!(items[1].coids.is_empty());
}

#[test]
fn marks_and_list_numbers_are_highlighted() {
    let source = "----\nx // <1>\n----\n<1> text\n";
    let tokens = tokens(source);
    let found: Vec<_> = tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::Callout | TokenKind::ListMarker))
        .map(|t| (t.kind, &source[t.span.start..t.span.end]))
        .collect();
    assert_eq!(
        found,
        [
            (TokenKind::Callout, "// <1>"),
            (TokenKind::ListMarker, "<1>")
        ]
    );
}
