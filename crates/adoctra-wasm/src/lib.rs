//! WASM bindings for an AsciiDoc editor.
//!
//! Exposes only what an editor needs — never the converters:
//!   * [`parse_json`]  — error-tolerant parse → JSON `{ document, diagnostics }`
//!   * [`tokens_json`] — highlight tokens → JSON array
//!
//! The JSON producers below are pure Rust (no wasm-bindgen), so they compile
//! and unit-test on the native target. The `#[wasm_bindgen]` wrappers are
//! gated to `wasm32`; build them with:
//!
//! ```sh
//! rustup target add wasm32-unknown-unknown
//! wasm-pack build crates/adoctra-wasm --target web   # or --target bundler
//! ```
//!
//! On the JS side: `JSON.parse(parse_json(src))` gives `{ document, diagnostics }`,
//! and diagnostics/tokens carry byte spans (`{ start, end }`) that map directly
//! onto CodeMirror/Monaco ranges.

use adoctra_core::ast::{Block, BlockKind, Inline, Span};
use adoctra_core::diagnostic::{Diagnostic, ParseResult};
use adoctra_core::json::escape_into as write_str;
use adoctra_core::{Token, TokenKind};

/// Parse `src` and return JSON `{ "document": {...}, "diagnostics": [...] }`.
pub fn parse_json(src: &str) -> String {
    let result = adoctra_core::parse(src);
    serialize_parse_result(&result)
}

/// Tokenize `src` and return a JSON array of `{ "span": {...}, "kind": "..." }`.
pub fn tokens_json(src: &str) -> String {
    let toks = adoctra_core::tokens(src);
    serialize_tokens(&toks)
}

// ---- pure serialization (native-testable) --------------------------------

fn serialize_parse_result(r: &ParseResult) -> String {
    let mut s = String::from("{\"document\":");
    write_document(&r.document, &mut s);
    s.push_str(",\"diagnostics\":[");
    for (i, d) in r.diagnostics.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_diagnostic(d, &mut s);
    }
    s.push_str("]}");
    s
}

fn write_document(doc: &adoctra_core::Document, s: &mut String) {
    s.push_str("{\"attributes\":{");
    for (i, (k, v)) in doc.attributes.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_str(k, s);
        s.push(':');
        write_str(v, s);
    }
    s.push_str("},\"blocks\":[");
    for (i, b) in doc.blocks.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_block(b, s);
    }
    s.push_str("]}");
}

fn write_block(b: &Block, s: &mut String) {
    s.push('{');
    write_span_field(&b.span, s);
    s.push(',');
    match &b.kind {
        BlockKind::Heading { level, inlines } => {
            s.push_str(&format!(
                "\"name\":\"heading\",\"level\":{level},\"inlines\":"
            ));
            write_inlines(inlines, s);
        }
        BlockKind::Paragraph { inlines } => {
            s.push_str("\"name\":\"paragraph\",\"inlines\":");
            write_inlines(inlines, s);
        }
        BlockKind::Listing { text } => {
            s.push_str("\"name\":\"listing\",\"text\":");
            write_str(text, s);
        }
    }
    s.push('}');
}

fn write_inlines(inlines: &[Inline], s: &mut String) {
    s.push('[');
    for (i, inline) in inlines.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        // Every inline now carries a span, so the editor can map highlight /
        // hover ranges straight off the AST.
        s.push('{');
        write_span_field(&inline.span(), s);
        s.push(',');
        match inline {
            Inline::Text { value, .. } => {
                s.push_str("\"name\":\"text\",\"value\":");
                write_str(value, s);
            }
            Inline::Strong { children, .. } => {
                s.push_str("\"name\":\"strong\",\"inlines\":");
                write_inlines(children, s);
            }
            Inline::Emphasis { children, .. } => {
                s.push_str("\"name\":\"emphasis\",\"inlines\":");
                write_inlines(children, s);
            }
            // The editor gets the structured reference (name + resolved value),
            // unlike the backends which only need the resolved text.
            Inline::AttributeRef { name, value, .. } => {
                s.push_str("\"name\":\"attribute-ref\",\"attribute\":");
                write_str(name, s);
                s.push_str(",\"value\":");
                match value {
                    Some(v) => write_str(v, s),
                    None => s.push_str("null"),
                }
            }
        }
        s.push('}');
    }
    s.push(']');
}

fn write_diagnostic(d: &Diagnostic, s: &mut String) {
    s.push('{');
    write_span_field(&d.span, s);
    s.push_str(&format!(
        ",\"severity\":\"{}\",\"message\":",
        d.severity.as_str()
    ));
    write_str(&d.message, s);
    s.push('}');
}

fn serialize_tokens(toks: &[Token]) -> String {
    let mut s = String::from("[");
    for (i, t) in toks.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_token(t, s_mut(&mut s));
    }
    s.push(']');
    s
}

// tiny helper so the closure-free loop above reads cleanly
fn s_mut(s: &mut String) -> &mut String {
    s
}

fn write_token(t: &Token, s: &mut String) {
    s.push('{');
    write_span_field(&t.span, s);
    s.push_str(&format!(",\"kind\":\"{}\"}}", kind_str(t.kind)));
}

fn kind_str(k: TokenKind) -> &'static str {
    k.as_str()
}

fn write_span_field(span: &Span, s: &mut String) {
    s.push_str(&format!(
        "\"span\":{{\"start\":{},\"end\":{}}}",
        span.start, span.end
    ));
}

// ---- wasm glue (only compiled for wasm32) ---------------------------------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(start)]
    pub fn init() {
        console_error_panic_hook::set_once();
    }

    /// Parse AsciiDoc → JSON `{ document, diagnostics }`.
    #[wasm_bindgen]
    pub fn parse_json(src: &str) -> String {
        super::parse_json(src)
    }

    /// Tokenize AsciiDoc → JSON array of highlight tokens.
    #[wasm_bindgen]
    pub fn tokens_json(src: &str) -> String {
        super::tokens_json(src)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_has_document_and_diagnostics() {
        let json = parse_json("= Title\n\ntext");
        assert!(json.contains("\"document\""));
        assert!(json.contains("\"diagnostics\":[]"));
        assert!(json.contains("\"heading\""));
        assert!(json.contains("\"span\":{\"start\":0,\"end\":7}"));
    }

    #[test]
    fn parse_json_reports_diagnostic() {
        let json = parse_json("----\nunterminated");
        assert!(json.contains("\"severity\":\"error\""));
    }

    #[test]
    fn tokens_json_is_array() {
        let json = tokens_json("= Title");
        assert!(json.starts_with('['));
        assert!(json.contains("\"kind\":\"heading\""));
    }

    #[test]
    fn inline_nodes_carry_spans() {
        // The editor needs a span on every inline, not just on blocks.
        let json = parse_json("ab *cd*");
        // the strong runs from byte 3 to byte 7 ("*cd*")
        assert!(
            json.contains("\"span\":{\"start\":3,\"end\":7},\"name\":\"strong\""),
            "got: {json}"
        );
    }

    #[test]
    fn attribute_reference_is_a_structured_node() {
        // Unlike the backends, the editor keeps the reference structured.
        let json = parse_json(":v: 9\n\nVersion {v}");
        assert!(json.contains("\"name\":\"attribute-ref\""), "got: {json}");
        assert!(json.contains("\"attribute\":\"v\""), "got: {json}");
        assert!(json.contains("\"value\":\"9\""), "got: {json}");
    }

    #[test]
    fn missing_attribute_reference_has_null_value() {
        let json = parse_json("Ref {nope}");
        assert!(json.contains("\"name\":\"attribute-ref\""), "got: {json}");
        assert!(json.contains("\"value\":null"), "got: {json}");
    }
}
