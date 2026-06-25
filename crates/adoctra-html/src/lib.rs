//! HTML5 backend.

use adoctra_core::ast::{Block, BlockKind, Inline};
use adoctra_core::{Backend, Document, Result};

#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlBackend;

impl HtmlBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Backend for HtmlBackend {
    fn name(&self) -> &'static str {
        "html"
    }

    fn extension(&self) -> &'static str {
        "html"
    }

    fn convert(&self, doc: &Document) -> Result<Vec<u8>> {
        let mut out = String::new();
        out.push_str("<!DOCTYPE html>\n<html>\n<body>\n");
        for block in &doc.blocks {
            render_block(block, &mut out);
        }
        out.push_str("</body>\n</html>\n");
        Ok(out.into_bytes())
    }
}

fn render_block(block: &Block, out: &mut String) {
    match &block.kind {
        BlockKind::Heading { level, inlines } => {
            let h = (level + 1).min(6);
            out.push_str(&format!("<h{h}>"));
            render_inlines(inlines, out);
            out.push_str(&format!("</h{h}>\n"));
        }
        BlockKind::Paragraph { inlines } => {
            out.push_str("<p>");
            render_inlines(inlines, out);
            out.push_str("</p>\n");
        }
        BlockKind::Listing { text } => {
            out.push_str("<pre class=\"listing\">");
            out.push_str(&escape(text));
            out.push_str("</pre>\n");
        }
    }
}

fn render_inlines(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(t) => out.push_str(&escape(t)),
            Inline::Strong(children) => {
                out.push_str("<strong>");
                render_inlines(children, out);
                out.push_str("</strong>");
            }
            Inline::Emphasis(children) => {
                out.push_str("<em>");
                render_inlines(children, out);
                out.push_str("</em>");
            }
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_heading_and_paragraph() {
        let doc = adoctra_core::parse("= Title\n\nHi *there*").document;
        let html = String::from_utf8(HtmlBackend.convert(&doc).unwrap()).unwrap();
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<strong>there</strong>"));
    }
}
