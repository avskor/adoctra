use adoctra_core::{parse, Backend};
use adoctra_json::JsonBackend;

#[test]
fn block_math_exports_notation_metadata_and_neutral_content() {
    let result = parse(".Equation\n[latexmath#eq.red,subs=\"+quotes\"]\n++++\nЖ < x *y*\n++++");
    let json = String::from_utf8(JsonBackend::new().convert(&result.document).unwrap()).unwrap();
    assert!(
        json.contains(r#""name":"stem","id":"eq","roles":["red"],"style":"latexmath""#),
        "{json}"
    );
    assert!(
        json.contains(r#""title":[{"name":"text","value":"Equation"}]"#),
        "{json}"
    );
    assert!(json.contains(r#""value":"Ж < x y""#), "{json}");
    assert!(
        json.contains(r#"{"kind":"open","format":"strong"}"#),
        "{json}"
    );
    assert!(!json.contains("stemblock"));
}
