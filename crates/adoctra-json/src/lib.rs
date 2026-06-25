//! JSON (ASG) backend.
//!
//! Emits a JSON tree of the document; the natural place to target the official
//! AsciiDoc Abstract Syntax Graph and act as a TCK adapter. Hand-serialized to
//! stay dependency-free; switch to serde for the real ASG shape.

use adoctra_core::ast::{Block, BlockKind, Inline};
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
            write_json_string(text, s);
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
            Inline::Text(t) => {
                s.push_str("{\"name\":\"text\",\"value\":");
                write_json_string(t, s);
                s.push('}');
            }
            Inline::Strong(c) => {
                s.push_str("{\"name\":\"strong\",\"inlines\":");
                write_inlines(c, s);
                s.push('}');
            }
            Inline::Emphasis(c) => {
                s.push_str("{\"name\":\"emphasis\",\"inlines\":");
                write_inlines(c, s);
                s.push('}');
            }
        }
    }
    s.push(']');
}

fn write_json_string(value: &str, s: &mut String) {
    s.push('"');
    for c in value.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            '\r' => s.push_str("\\r"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_json() {
        let doc = adoctra_core::parse("= Title").document;
        let json = String::from_utf8(JsonBackend.convert(&doc).unwrap()).unwrap();
        assert!(json.starts_with("{\"name\":\"document\""));
        assert!(json.contains("\"heading\""));
    }
}
