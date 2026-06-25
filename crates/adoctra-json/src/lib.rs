//! JSON (ASG) backend.
//!
//! Emits a JSON tree of the document; the natural place to target the official
//! AsciiDoc Abstract Syntax Graph and act as a TCK adapter. Hand-serialized to
//! stay dependency-free; switch to serde for the real ASG shape.

use adoctra_core::ast::{Block, BlockKind, Inline};
use adoctra_core::json::escape_into;
use adoctra_core::{Backend, Document, Result};

#[derive(Debug, Default, Clone, Copy)]
pub struct JsonBackend;

impl JsonBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Backend for JsonBackend {
    fn name(&self) -> &'static str {
        "json"
    }

    fn extension(&self) -> &'static str {
        "json"
    }

    fn convert(&self, doc: &Document) -> Result<Vec<u8>> {
        let mut s = String::new();
        s.push_str("{\"name\":\"document\",\"blocks\":[");
        for (i, block) in doc.blocks.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            write_block(block, &mut s);
        }
        s.push_str("]}");
        Ok(s.into_bytes())
    }
}

fn write_block(block: &Block, s: &mut String) {
    match &block.kind {
        BlockKind::Heading { level, inlines } => {
            s.push_str(&format!(
                "{{\"name\":\"heading\",\"level\":{level},\"inlines\":"
            ));
            write_inlines(inlines, s);
            s.push('}');
        }
        BlockKind::Paragraph { inlines } => {
            s.push_str("{\"name\":\"paragraph\",\"inlines\":");
            write_inlines(inlines, s);
            s.push('}');
        }
        BlockKind::Listing { text } => {
            s.push_str("{\"name\":\"listing\",\"text\":");
            escape_into(text, s);
            s.push('}');
        }
    }
}

fn write_inlines(inlines: &[Inline], s: &mut String) {
    s.push('[');
    for (i, inline) in inlines.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        match inline {
            Inline::Text { value, .. } => {
                s.push_str("{\"name\":\"text\",\"value\":");
                escape_into(value, s);
                s.push('}');
            }
            Inline::Strong { children, .. } => {
                s.push_str("{\"name\":\"strong\",\"inlines\":");
                write_inlines(children, s);
                s.push('}');
            }
            Inline::Emphasis { children, .. } => {
                s.push_str("{\"name\":\"emphasis\",\"inlines\":");
                write_inlines(children, s);
                s.push('}');
            }
            Inline::Monospace { children, .. } => {
                s.push_str("{\"name\":\"monospace\",\"inlines\":");
                write_inlines(children, s);
                s.push('}');
            }
            Inline::Mark { children, .. } => {
                s.push_str("{\"name\":\"mark\",\"inlines\":");
                write_inlines(children, s);
                s.push('}');
            }
            Inline::Superscript { children, .. } => {
                s.push_str("{\"name\":\"superscript\",\"inlines\":");
                write_inlines(children, s);
                s.push('}');
            }
            Inline::Subscript { children, .. } => {
                s.push_str("{\"name\":\"subscript\",\"inlines\":");
                write_inlines(children, s);
                s.push('}');
            }
            // Attribute reference resolved in core: serialize as plain text
            // (the value, or the literal `{name}` when the attribute was
            // missing), which is what the ASG carries after resolution.
            Inline::AttributeRef { name, value, .. } => {
                s.push_str("{\"name\":\"text\",\"value\":");
                match value {
                    Some(v) => escape_into(v, s),
                    None => escape_into(&format!("{{{name}}}"), s),
                }
                s.push('}');
            }
        }
    }
    s.push(']');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(src: &str) -> String {
        let doc = adoctra_core::parse(src).document;
        String::from_utf8(JsonBackend.convert(&doc).unwrap()).unwrap()
    }

    #[test]
    fn emits_json() {
        let json = render("= Title");
        assert!(json.starts_with("{\"name\":\"document\""));
        assert!(json.contains("\"heading\""));
    }

    #[test]
    fn strong_and_emphasis_nest() {
        let json = render("a *b _c_*");
        assert!(json.contains("\"name\":\"strong\""));
        assert!(json.contains("\"name\":\"emphasis\""));
    }

    #[test]
    fn new_quotes_serialize_by_name() {
        let json = render("A `code`, #hot#, x^2^, y~1~.");
        assert!(json.contains("\"name\":\"monospace\""), "got: {json}");
        assert!(json.contains("\"name\":\"mark\""), "got: {json}");
        assert!(json.contains("\"name\":\"superscript\""), "got: {json}");
        assert!(json.contains("\"name\":\"subscript\""), "got: {json}");
    }

    #[test]
    fn monospace_nests_inline_children() {
        let json = render("`*b*`");
        assert!(json.contains("\"name\":\"monospace\""), "got: {json}");
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");
    }

    #[test]
    fn resolved_attribute_serializes_as_text() {
        // After resolution the ASG carries plain text, not an attribute node.
        let json = render(":v: 9\n\nVersion {v}.");
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"9\"}"),
            "got: {json}"
        );
        assert!(!json.contains("attribute"), "got: {json}");
    }
}
