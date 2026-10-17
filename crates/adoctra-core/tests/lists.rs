//! Lists parsed like Ruby's `parse_list`, `parse_description_list`,
//! `parse_list_item` and `read_lines_for_list_item`.
use adoctra_core::ast::{DescriptionListItem, ListItem};
use adoctra_core::{parse, Block, BlockKind, Inline, Severity, Span};

fn blocks(source: &str) -> Vec<Block> {
    parse(source).document.into_flat_blocks()
}

fn text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { value, .. } | Inline::Raw { value, .. } => value.clone(),
            Inline::Strong { children, .. } | Inline::CrossReference { children, .. } => {
                text(children)
            }
            Inline::Anchor { .. } => String::new(),
            other => panic!("unexpected inline {other:?}"),
        })
        .collect()
}

fn items(block: &Block) -> &[ListItem] {
    match &block.kind {
        BlockKind::List { items, .. } => items,
        other => panic!("not a list: {other:?}"),
    }
}

fn entries(block: &Block) -> &[DescriptionListItem] {
    match &block.kind {
        BlockKind::DescriptionList { items } => items,
        other => panic!("not a description list: {other:?}"),
    }
}

#[test]
fn blank_lines_keep_siblings_and_a_new_marker_nests() {
    let blocks = blocks("* a\nwrapped\n\n\n* b\n\n- nested\n\n* c\n");
    assert_eq!(blocks.len(), 1);
    let items = items(&blocks[0]);
    let texts: Vec<_> = items.iter().map(|i| text(&i.inlines)).collect();
    assert_eq!(texts, ["a wrapped", "b", "c"]);
    assert!(matches!(items[1].blocks[0].kind, BlockKind::List { .. }));
    assert_eq!(items[1].marker.as_deref(), Some("*"));
}

#[test]
fn list_lines_do_not_interrupt_a_paragraph() {
    let blocks = blocks("para\n* item\n");
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
}

#[test]
fn continuations_attach_blocks_with_their_metadata() {
    let source = "* item\n+\n[[ex]]\n.Title\n====\nexample\n====\n\n+\ndetached\n* two\n";
    let blocks = blocks(source);
    let items = items(&blocks[0]);
    assert_eq!(items.len(), 2);
    let example = &items[0].blocks[0];
    assert!(matches!(example.kind, BlockKind::Example { .. }));
    assert_eq!(example.id.as_deref(), Some("ex"));
    assert_eq!(example.caption.as_deref(), Some("Example 1. "));
    assert!(matches!(
        items[0].blocks[1].kind,
        BlockKind::Paragraph { .. }
    ));
    // The item spans its marker through its last block.
    let span = items[0].span;
    assert_eq!(
        &source[span.start..span.end],
        &source[..source.find("detached").unwrap() + 8]
    );
}

#[test]
fn ordered_lists_get_ruby_styles_and_index_warnings() {
    let result = parse(". a\n.. b\n\nc. x\nd. y\n\n1. one\n3. three\n");
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| (d.severity, d.message.as_str()))
        .collect();
    assert_eq!(
        warnings,
        [
            (Severity::Warning, "list item index: expected a, got c"),
            (Severity::Warning, "list item index: expected b, got d"),
            (Severity::Warning, "list item index: expected 2, got 3"),
        ]
    );
    let blocks = result.document.into_flat_blocks();
    assert_eq!(blocks[0].attrs.style.as_deref(), Some("arabic"));
    let nested = &items(&blocks[0])[0].blocks[0];
    assert_eq!(nested.attrs.style.as_deref(), Some("loweralpha"));
    let lettered = &items(nested)[0].blocks[0];
    assert_eq!(lettered.attrs.style.as_deref(), Some("loweralpha"));
    assert_eq!(items(lettered)[0].marker.as_deref(), Some("a."));
}

#[test]
fn checklists_set_the_option_and_item_state() {
    let blocks = blocks("* [x] done\n* [ ] todo\n* plain\n");
    assert!(blocks[0].attrs.options.contains(&"checklist".to_owned()));
    let items = items(&blocks[0]);
    let states: Vec<_> = items
        .iter()
        .map(|i| (i.checkbox, text(&i.inlines)))
        .collect();
    assert_eq!(
        states,
        [
            (Some(true), "done".into()),
            (Some(false), "todo".into()),
            (None, "plain".into())
        ]
    );
}

#[test]
fn description_lists_share_descriptions_between_terms() {
    let source = "CPU:: The *brain*\nA::\nB:: shared\n+\n----\ncode\n----\nDisk::\nStorage\n";
    let blocks = blocks(source);
    let entries = entries(&blocks[0]);
    assert_eq!(entries.len(), 3);
    let terms: Vec<Vec<String>> = entries
        .iter()
        .map(|e| e.terms.iter().map(|t| text(&t.inlines)).collect())
        .collect();
    assert_eq!(terms, [vec!["CPU"], vec!["A", "B"], vec!["Disk"]]);
    let cpu = &entries[0];
    assert_eq!(
        &source[cpu.terms[0].span.start..cpu.terms[0].span.end],
        "CPU"
    );
    let description = cpu.description.as_ref().unwrap();
    assert_eq!(text(&description.inlines), "The brain");
    let shared = entries[1].description.as_ref().unwrap();
    assert_eq!(text(&shared.inlines), "shared");
    assert!(matches!(shared.blocks[0].kind, BlockKind::Listing { .. }));
    // The description on the next line folds into its text.
    assert_eq!(
        text(&entries[2].description.as_ref().unwrap().inlines),
        "Storage"
    );
}

#[test]
fn description_delimiters_nest_and_a_lone_term_has_no_description() {
    let blocks = blocks("Outer:: one\nInner::: two\nOuter2::\n\nLast;;\n");
    let entries_outer = entries(&blocks[0]);
    assert_eq!(entries_outer.len(), 2);
    let inner = &entries_outer[0].description.as_ref().unwrap().blocks[0];
    assert_eq!(text(&entries(inner)[0].terms[0].inlines), "Inner");
    // A term still needing its description takes a list after a blank line
    // (Ruby's greedy description list); the last term has none.
    let nested = &entries_outer[1].description.as_ref().unwrap().blocks[0];
    assert!(entries(nested)[0].description.is_none());
    assert_eq!(blocks.len(), 1);
}

#[test]
fn term_anchors_take_the_rest_of_the_term_as_reftext() {
    let result = parse("[[cpu]]CPU:: The brain\n\nSee <<cpu>>.\n");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = result.document.into_flat_blocks();
    let BlockKind::Paragraph { inlines } = &blocks[1].kind else {
        panic!()
    };
    assert_eq!(text(inlines), "See CPU.");
}

#[test]
fn indented_paragraphs_are_literal_blocks() {
    let source = "  indented *text*\n    deeper\n\n* item\n+\n  attached\n";
    let blocks = blocks(source);
    assert_eq!(
        blocks[0].kind,
        BlockKind::Literal {
            text: "indented *text*\n  deeper".into(),
            callouts: Vec::new(),
        }
    );
    assert_eq!(blocks[0].span, Span::new(0, 28));
    let attached = &items(&blocks[1])[0].blocks[0];
    assert_eq!(
        attached.kind,
        BlockKind::Literal {
            text: "attached".into(),
            callouts: Vec::new(),
        }
    );
    // Adjacent to a term, an indented line is the description's text.
    let blocks = self::blocks("Term::\n  adjacent\n");
    let description = entries(&blocks[0])[0].description.as_ref().unwrap();
    assert_eq!(text(&description.inlines), "adjacent");
}
