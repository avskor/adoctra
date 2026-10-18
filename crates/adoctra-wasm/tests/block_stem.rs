#[test]
fn editor_exports_block_math_with_spans_and_protected_tokens() {
    let source = ".Equation\n[latexmath#eq.red]\n++++\nЖ < x *y*\n++++";
    let json = adoctra_wasm::parse_json(source);
    assert!(
        json.contains(r#""name":"stem","id":"eq","roles":["red"],"style":"latexmath""#),
        "{json}"
    );
    let start = source.find("++++").unwrap();
    assert!(
        json.contains(&format!(
            r#""span":{{"source_id":0,"start":{start},"end":{}}},"name":"stem""#,
            source.len()
        )),
        "{json}"
    );
    assert!(json.contains(r#""value":"Ж < x *y*""#), "{json}");
    let tokens = adoctra_wasm::tokens_json(source);
    assert!(tokens.contains("stem-text"), "{tokens}");
    assert!(tokens.contains("stem-delimiter"), "{tokens}");
    assert!(!tokens.contains("\"strong\""), "{tokens}");
}
