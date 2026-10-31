/// The editor receives one diagnostic stream in Ruby's reporting order
/// (see adoctra-core's diagnostic_order tests), each with its byte span.
#[test]
fn editor_diagnostics_follow_ruby_reporting_order_with_spans() {
    let source = ":attribute-missing: warn\n== {g}\n****\nendif::[]\n";
    let json = adoctra_wasm::parse_json(source);
    let entry = |needle: &str, severity: &str, message: &str| {
        let start = source.find(needle).unwrap();
        format!(
            r#"{{"span":{{"source_id":0,"start":{start},"end":{}}},"severity":"{severity}","message":"{message}"}}"#,
            start + needle.len()
        )
    };
    let expected = [
        // The section title converts while reading (its automatic id).
        entry(
            "{g}",
            "warning",
            "skipping reference to missing attribute: g",
        ),
        // The sidebar's read-ahead meets the directive, then the end.
        entry("endif::[]", "error", "unmatched endif directive"),
        entry(
            "****",
            "error",
            "unterminated sidebar block (missing closing `****`)",
        ),
    ]
    .join(",");
    assert!(
        json.contains(&format!(r#""diagnostics":[{expected}]"#)),
        "{json}"
    );
}
