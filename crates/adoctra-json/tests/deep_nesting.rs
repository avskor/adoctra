//! The JSON backend writes nested blocks without recursion
//! (`adoctra_core::segments`): a deeply nested document converts on a 1 MiB
//! thread.

use adoctra_core::{parse, Backend};
use adoctra_json::JsonBackend;

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
fn deeply_nested_blocks_serialize_on_a_small_stack() {
    on_small_stack(|| {
        let json = JsonBackend
            .convert(&parse(&nested_examples(3000)).document)
            .unwrap();
        let json = String::from_utf8(json).unwrap();
        assert_eq!(json.matches(r#""name":"example""#).count(), 3000);
        assert!(json.ends_with("]}"), "{}", &json[json.len() - 40..]);

        let json = JsonBackend
            .convert(&parse(&nested_list(200)).document)
            .unwrap();
        let json = String::from_utf8(json).unwrap();
        assert_eq!(json.matches(r#""name":"list""#).count(), 200);
    });
}
