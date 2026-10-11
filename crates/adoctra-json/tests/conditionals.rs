use adoctra_core::{parse, Backend};
use adoctra_json::JsonBackend;

#[test]
fn conditional_trivia_is_absent_from_the_semantic_backend() {
    let result = parse(":a:\n\nifdef::missing[]\n[[hidden]]\nHidden\nendif::[]\nifdef::a[*shown*]");
    assert!(result.diagnostics.is_empty());
    let json = String::from_utf8(JsonBackend.convert(&result.document).unwrap()).unwrap();
    assert!(
        !json.contains("hidden") && !json.contains("Hidden") && !json.contains("ifdef"),
        "{json}"
    );
    assert!(json.contains("strong") && json.contains("shown"), "{json}");
}
