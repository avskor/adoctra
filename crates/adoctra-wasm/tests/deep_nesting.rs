//! The editor serialization writes nested blocks without recursion
//! (`adoctra_core::segments`): a deeply nested buffer serializes on a 1 MiB
//! thread, about a wasm32 module's stack.

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
fn deeply_nested_buffer_serializes_on_a_small_stack() {
    on_small_stack(|| {
        let source = nested_examples(3000);
        let json = adoctra_wasm::parse_json(&source);
        assert_eq!(json.matches(r#""name":"example""#).count(), 3000);
        // The outermost block spans the whole buffer but its last newline.
        assert!(
            json.contains(&format!(
                r#"{{"span":{{"source_id":0,"start":0,"end":{}}},"name":"example""#,
                source.len() - 1
            )),
            "{}",
            &json[..200]
        );
        assert!(!adoctra_wasm::tokens_json(&source).is_empty());

        let json = adoctra_wasm::parse_json(&nested_list(200));
        assert_eq!(json.matches(r#""name":"list""#).count(), 200);
    });
}
