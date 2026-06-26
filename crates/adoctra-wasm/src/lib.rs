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
        BlockKind::List { ordered, items } => {
            s.push_str(&format!(
                "\"name\":\"list\",\"ordered\":{ordered},\"items\":["
            ));
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                // Each item carries its own span for the editor.
                s.push('{');
                write_span_field(&item.span, s);
                s.push_str(",\"inlines\":");
                write_inlines(&item.inlines, s);
                // Nested blocks (e.g. a sublist); each carries its own spans.
                s.push_str(",\"blocks\":[");
                for (j, child) in item.blocks.iter().enumerate() {
                    if j > 0 {
                        s.push(',');
                    }
                    write_block(child, s);
                }
                s.push(']');
                s.push('}');
            }
            s.push(']');
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
            Inline::Monospace { children, .. } => {
                s.push_str("\"name\":\"monospace\",\"inlines\":");
                write_inlines(children, s);
            }
            Inline::Mark { children, .. } => {
                s.push_str("\"name\":\"mark\",\"inlines\":");
                write_inlines(children, s);
            }
            Inline::Superscript { children, .. } => {
                s.push_str("\"name\":\"superscript\",\"inlines\":");
                write_inlines(children, s);
            }
            Inline::Subscript { children, .. } => {
                s.push_str("\"name\":\"subscript\",\"inlines\":");
                write_inlines(children, s);
            }
            // The editor gets the resolved href, the `bare` flag and the display
            // text (each child carries its own span, like any other inline).
            Inline::Link {
                target,
                children,
                bare,
                ..
            } => {
                s.push_str("\"name\":\"link\",\"target\":");
                write_str(target, s);
                s.push_str(&format!(",\"bare\":{bare},\"inlines\":"));
                write_inlines(children, s);
            }
            // The editor gets the raw anchor id (`target`) so it can resolve or
            // validate the reference; the display text is in `inlines`.
            Inline::CrossReference {
                target, children, ..
            } => {
                s.push_str("\"name\":\"xref\",\"target\":");
                write_str(target, s);
                s.push_str(",\"inlines\":");
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
    fn new_quotes_are_structured_nodes_with_spans() {
        let json = parse_json("ab `cd`");
        // the monospace span runs from byte 3 to byte 7 ("`cd`")
        assert!(
            json.contains("\"span\":{\"start\":3,\"end\":7},\"name\":\"monospace\""),
            "got: {json}"
        );
    }

    #[test]
    fn all_new_quote_names_appear() {
        let json = parse_json("`c` #m# x^2^ y~1~");
        for name in ["monospace", "mark", "superscript", "subscript"] {
            assert!(
                json.contains(&format!("\"name\":\"{name}\"")),
                "missing {name} in {json}"
            );
        }
    }

    #[test]
    fn link_is_a_structured_node_with_span_and_target() {
        let json = parse_json("See link:https://h[the site]");
        // The link runs from byte 4 ("link:") through the closing bracket (28).
        assert!(
            json.contains("\"span\":{\"start\":4,\"end\":28},\"name\":\"link\""),
            "got: {json}"
        );
        assert!(json.contains("\"target\":\"https://h\""), "got: {json}");
        assert!(json.contains("\"bare\":false"), "got: {json}");
    }

    #[test]
    fn bare_autolink_is_structured_and_bare() {
        let json = parse_json("Bare https://example.org now");
        assert!(json.contains("\"name\":\"link\""), "got: {json}");
        assert!(json.contains("\"bare\":true"), "got: {json}");
        assert!(
            json.contains("\"target\":\"https://example.org\""),
            "got: {json}"
        );
    }

    #[test]
    fn xref_is_a_structured_node_with_span_and_target() {
        // "<<sec,Two>>" spans bytes 0..11; the editor gets the raw `target`.
        let json = parse_json("<<sec,Two>>");
        assert!(
            json.contains("\"span\":{\"start\":0,\"end\":11},\"name\":\"xref\""),
            "got: {json}"
        );
        assert!(json.contains("\"target\":\"sec\""), "got: {json}");
    }

    #[test]
    fn textless_xref_carries_target_and_bracketed_text() {
        let json = parse_json("<<sec>>");
        assert!(json.contains("\"name\":\"xref\""), "got: {json}");
        assert!(json.contains("\"target\":\"sec\""), "got: {json}");
        assert!(
            json.contains("\"name\":\"text\",\"value\":\"[sec]\""),
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

    #[test]
    fn list_block_serializes_items_with_spans() {
        let json = parse_json("* one\n* two");
        assert!(json.contains("\"name\":\"list\""), "got: {json}");
        assert!(json.contains("\"ordered\":false"), "got: {json}");
        // Each item carries its own span (the whole item line).
        assert!(
            json.contains("\"span\":{\"start\":0,\"end\":5}"),
            "got: {json}"
        );
        assert!(
            json.contains("\"span\":{\"start\":6,\"end\":11}"),
            "got: {json}"
        );
    }

    #[test]
    fn ordered_list_marks_ordered_true() {
        let json = parse_json(". a\n. b");
        assert!(json.contains("\"ordered\":true"), "got: {json}");
    }

    #[test]
    fn explicit_numbered_list_marks_ordered_true() {
        // `1. a` reuses the ordered-list node; spans still anchor to real bytes.
        let json = parse_json("1. a\n2. b");
        assert!(json.contains("\"ordered\":true"), "got: {json}");
        // Item content "a" sits at bytes 3..4 of "1. a".
        assert!(
            json.contains("\"span\":{\"start\":3,\"end\":4},\"name\":\"text\""),
            "got: {json}"
        );
    }

    #[test]
    fn list_item_inlines_carry_spans() {
        // The item content "one" sits at bytes 2..5 of "* one".
        let json = parse_json("* one");
        assert!(
            json.contains("\"span\":{\"start\":2,\"end\":5},\"name\":\"text\""),
            "got: {json}"
        );
    }

    #[test]
    fn list_marker_is_tokenized() {
        let json = tokens_json("* one");
        assert!(json.contains("\"kind\":\"list-marker\""), "got: {json}");
    }

    #[test]
    fn nested_list_serializes_with_spans() {
        // "* one\n** a": item "one" spans 0..10 and carries a nested list block
        // (span 6..10) in its "blocks"; the editor can map every level.
        let json = parse_json("* one\n** a");
        assert!(
            json.contains("\"blocks\":[{\"span\":{\"start\":6,\"end\":10},\"name\":\"list\""),
            "got: {json}"
        );
        // The nested item's principal text sits at bytes 9..10.
        assert!(
            json.contains("\"span\":{\"start\":9,\"end\":10},\"name\":\"text\""),
            "got: {json}"
        );
    }

    #[test]
    fn continuation_block_serializes_in_item_blocks() {
        // `+` attaches a paragraph; it serializes as a spanned block inside the
        // item's "blocks", so the editor can map the attached content too.
        let json = parse_json("* one\n+\nattached");
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
        assert!(
            json.contains("\"name\":\"text\",\"value\":\"attached\""),
            "got: {json}"
        );
    }

    #[test]
    fn multi_line_item_span_covers_both_lines() {
        // The wrapped item "* one\n  two" spans 0..11 (both source lines), so the
        // editor maps the whole item even though its text folds to one line.
        let json = parse_json("* one\n  two");
        assert!(
            json.contains("\"span\":{\"start\":0,\"end\":11}"),
            "got: {json}"
        );
    }

    #[test]
    fn typographic_replacements_resolve_in_text_value() {
        // The editor receives resolved text (the literal Unicode char), while the
        // span still covers the original `(C)` source bytes (0..3).
        let json = parse_json("(C) it's");
        assert!(
            json.contains("\"name\":\"text\",\"value\":\"\u{a9} it\u{2019}s\""),
            "got: {json}"
        );
        assert!(
            json.contains("\"span\":{\"start\":0,\"end\":8}"),
            "got: {json}"
        );
    }
}
