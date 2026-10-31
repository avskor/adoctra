//! Deep nesting needs no call stack per level in the core: blocks are read
//! on the parser's heap stack, the AST passes walk an explicit stack of
//! owned frames and a document drops its tree iteratively. Each test parses,
//! checks and drops the result on a 1 MiB thread (about a wasm32 module's
//! stack), far deeper than a recursive walk of frames would allow.

use adoctra_core::{parse, Block, BlockKind, Span};

fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
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
fn deeply_nested_containers_parse_and_drop_on_a_small_stack() {
    const DEPTH: usize = 3000;
    for fence in ['=', '*', '_'] {
        on_small_stack(move || {
            let source: String = (0..DEPTH)
                .map(|k| format!("{}\n", fence.to_string().repeat(4 + k)))
                .chain(["core\n".to_owned()])
                .chain(
                    (0..DEPTH)
                        .rev()
                        .map(|k| format!("{}\n", fence.to_string().repeat(4 + k))),
                )
                .collect();
            let result = parse(&source);
            assert!(result.diagnostics.is_empty(), "{fence}");
            let root = &result.document.blocks[0];
            // Every container plus the innermost paragraph.
            assert_eq!(depth(root), DEPTH + 1, "{fence}");
            assert_eq!(root.span, Span::new(0, source.len() - 1), "{fence}");
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
            // The tokens and the drop of the result run here too.
            assert!(!adoctra_core::tokens(&source).is_empty());
        });
    }
}

#[test]
fn deeply_nested_lists_and_blockquotes_parse_and_drop_on_a_small_stack() {
    // Deep lists read in time growing faster than the square of their
    // depth (each level copies the nested lines, like Ruby).
    on_small_stack(|| {
        let list: String = (0..200)
            .map(|k| format!("{} item {k}\n", "*".repeat(k + 1)))
            .collect();
        assert_eq!(depth(&parse(&list).document.blocks[0]), 200);

        // A long flat list continues in place, not one frame per item.
        let flat: String = (0..5_000).map(|k| format!("* item {k}\n")).collect();
        match &parse(&flat).document.blocks[0].kind {
            BlockKind::List { items, .. } => assert_eq!(items.len(), 5_000),
            other => panic!("expected a list, got {other:?}"),
        }

        let quote = format!("{}core\n", "> ".repeat(1000));
        assert_eq!(depth(&parse(&quote).document.blocks[0]), 1001);
    });
}
