//! The editor parses without extension processors (a JS host cannot
//! register Rust processors): a diagram block stays a listing with its style
//! and source, so the frontend can render it (e.g. with mermaid.js), and its
//! block macro stays a paragraph, exactly like Asciidoctor without
//! asciidoctor-diagram.

#[test]
fn editor_keeps_diagram_blocks_as_styled_listings() {
    let source = "[mermaid, flow, svg]\n----\ngraph TD; A-->B\n----\n\nplantuml::a.puml[]\n";
    let json = adoctra_wasm::parse_json(source);
    let start = source.find("----").unwrap();
    let end = source.find("\n\n").unwrap();
    assert!(
        json.contains(&format!(
            r#""span":{{"source_id":0,"start":{start},"end":{end}}},"name":"listing""#
        )),
        "{json}"
    );
    assert!(json.contains(r#""style":"mermaid""#), "{json}");
    assert!(json.contains(r#""text":"graph TD; A-->B""#), "{json}");
    assert!(json.contains(r#""name":"paragraph""#), "{json}");
    let tokens = adoctra_wasm::tokens_json(source);
    assert!(tokens.contains("listing-delimiter"), "{tokens}");
    assert!(!tokens.contains("block-macro"), "{tokens}");
}
