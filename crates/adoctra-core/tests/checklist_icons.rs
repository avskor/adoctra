//! Checklist marks, like Ruby's `convert_ulist`: the document `icons` mode
//! where the checklist converts, so a body entry before it applies.

use adoctra_core::{parse, Block, BlockKind};

/// Every list in document order (nested lists after their parent): whether
/// it is ordered and its captured `icons` mode.
fn lists(source: &str) -> Vec<(bool, Option<String>)> {
    fn walk(block: &Block, out: &mut Vec<(bool, Option<String>)>) {
        if let BlockKind::List {
            ordered,
            items,
            icons,
        } = &block.kind
        {
            out.push((*ordered, icons.clone()));
            for item in items {
                for child in &item.blocks {
                    walk(child, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    for block in parse(source).document.flat_blocks() {
        walk(block, &mut out);
    }
    out
}

#[test]
fn checklists_capture_the_icons_mode_where_they_convert() {
    let source = "* [x] a\n\n:icons: font\n\n* [ ] b\n** [x] nested\n\n:icons!:\n\n* [x] c\n";
    assert_eq!(
        lists(source),
        [
            (false, None),
            (false, Some("font".to_owned())),
            (false, Some("font".to_owned())),
            (false, None),
        ]
    );
}

#[test]
fn other_lists_capture_no_icons_mode() {
    let source = ":icons: font\n\n* plain\n\n. [x] ordered\n";
    assert_eq!(lists(source), [(false, None), (true, None)]);
}
