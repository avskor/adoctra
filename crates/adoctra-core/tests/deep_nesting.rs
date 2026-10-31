//! Deep nesting is read on the parser's heap stack (no recursion per level):
//! a document nested deeper than a call stack of frames would allow still
//! parses on a 1 MiB thread, about a wasm32 module's stack. The AST passes
//! after reading still recurse (the next step), which bounds the depth here.

use adoctra_core::{parse, Block, BlockKind, Span};

const DEPTH: usize = 200;

fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

/// The single child block of a container or the nested list of an item.
fn child(block: &Block) -> Option<&Block> {
    match &block.kind {
        BlockKind::Example { blocks }
        | BlockKind::Sidebar { blocks }
        | BlockKind::Quote { blocks, .. }
        | BlockKind::Open { blocks } => blocks.first(),
        BlockKind::List { items, .. } => items[0].blocks.first(),
        _ => None,
    }
}

fn depth(block: &Block) -> usize {
    let mut depth = 0;
    let mut current = Some(block);
    while let Some(block) = current {
        depth += 1;
        current = child(block);
    }
    depth
}

#[test]
fn deeply_nested_containers_parse_on_a_small_stack() {
    for fence in ['=', '*', '_'] {
        let source: String = (0..DEPTH)
            .map(|k| format!("{}\n", fence.to_string().repeat(4 + k)))
            .chain(["core\n".to_owned()])
            .chain(
                (0..DEPTH)
                    .rev()
                    .map(|k| format!("{}\n", fence.to_string().repeat(4 + k))),
            )
            .collect();
        let len = source.len();
        let result = on_small_stack(move || parse(&source));
        assert!(result.diagnostics.is_empty(), "{fence}");
        let root = &result.document.blocks[0];
        // Every container plus the innermost paragraph.
        assert_eq!(depth(root), DEPTH + 1, "{fence}");
        assert_eq!(root.span, Span::new(0, len - 1), "{fence}");
        // The innermost container spans its own fences and the paragraph.
        let mut inner = root;
        while let Some(next) = child(inner).filter(|b| child(b).is_some()) {
            inner = next;
        }
        let fence_len = 4 + DEPTH - 1;
        assert_eq!(
            inner.span.end - inner.span.start,
            2 * fence_len + "\ncore\n".len(),
            "{fence}"
        );
    }
}

#[test]
fn deeply_nested_lists_and_blockquotes_parse_on_a_small_stack() {
    let list: String = (0..DEPTH)
        .map(|k| format!("{} item {k}\n", "*".repeat(k + 1)))
        .collect();
    let result = on_small_stack(move || parse(&list));
    assert_eq!(depth(&result.document.blocks[0]), DEPTH);

    // A long flat list continues in place, not one frame per item.
    let flat: String = (0..5_000).map(|k| format!("* item {k}\n")).collect();
    let result = on_small_stack(move || parse(&flat));
    match &result.document.blocks[0].kind {
        BlockKind::List { items, .. } => assert_eq!(items.len(), 5_000),
        other => panic!("expected a list, got {other:?}"),
    }

    let quote = format!("{}core\n", "> ".repeat(DEPTH));
    let result = on_small_stack(move || parse(&quote));
    assert_eq!(depth(&result.document.blocks[0]), DEPTH + 1);
}
