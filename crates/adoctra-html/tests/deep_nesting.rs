//! The HTML backend writes nested blocks without recursion
//! (`adoctra_core::segments`): a deeply nested document converts on a 1 MiB
//! thread.

use adoctra_core::{parse, Backend};
use adoctra_html::HtmlBackend;

/// Nested example blocks and a nested list, deeper than a recursive renderer
/// could write on a 1 MiB stack (about a wasm32 module's).
fn nested_examples(depth: usize) -> String {
    (0..depth)
        .map(|k| format!("{}\n", "=".repeat(4 + k)))
        .chain(["core\n".to_owned()])
        .chain((0..depth).rev().map(|k| format!("{}\n", "=".repeat(4 + k))))
        .collect()
}

fn nested_list(depth: usize) -> String {
    (0..depth)
        .map(|k| format!("{} item {k}\n", "*".repeat(k + 1)))
        .collect()
}

fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deeply_nested_blocks_convert_on_a_small_stack() {
    on_small_stack(|| {
        let html = HtmlBackend::embedded()
            .convert(&parse(&nested_examples(3000)).document)
            .unwrap();
        let html = String::from_utf8(html).unwrap();
        assert_eq!(html.matches("<div class=\"exampleblock\">").count(), 3000);
        assert_eq!(html.matches("</div>\n</div>").count(), 3000);
        assert!(html.contains("<p>core</p>"));

        let html = HtmlBackend::embedded()
            .convert(&parse(&nested_list(200)).document)
            .unwrap();
        let html = String::from_utf8(html).unwrap();
        assert_eq!(html.matches("<ul>").count(), 200);
        assert_eq!(html.matches("</ul>").count(), 200);
        assert!(html.contains("<li>item 199</li>"));
    });
}
