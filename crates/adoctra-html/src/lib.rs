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
            Inline::Text { value, .. } => out.push_str(&escape(value)),
            Inline::Strong { children, .. } => {
                out.push_str("<strong>");
                render_inlines(children, out);
                out.push_str("</strong>");
            }
            Inline::Emphasis { children, .. } => {
                out.push_str("<em>");
                render_inlines(children, out);
                out.push_str("</em>");
            }
            // Resolved in core: emit the value, or fall back to the literal
            // `{name}` for a missing attribute (asciidoctor's default).
            Inline::AttributeRef { name, value, .. } => match value {
                Some(v) => out.push_str(&escape(v)),
                None => out.push_str(&escape(&format!("{{{name}}}"))),
            },
        }
    }
}

/// Output-encoding for HTML. Deliberately a *backend* concern: the shared model
/// keeps text literal, and each backend escapes for its own format.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(src: &str) -> String {
        let doc = adoctra_core::parse(src).document;
        String::from_utf8(HtmlBackend.convert(&doc).unwrap()).unwrap()
    }

    #[test]
    fn renders_heading_and_paragraph() {
        let html = render("= Title\n\nHi *there*");
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<strong>there</strong>"));
    }

    #[test]
    fn resolves_attribute_reference_to_its_value() {
        let html = render(":product: Adoctra\n\nUse {product}.");
        assert!(html.contains("<p>Use Adoctra.</p>"), "got: {html}");
    }

    #[test]
    fn missing_attribute_falls_back_to_literal() {
        let html = render("Use {nope} here.");
        assert!(html.contains("Use {nope} here."), "got: {html}");
    }

    #[test]
    fn text_is_html_escaped_by_the_backend() {
        // The model keeps text literal; escaping is the backend's job.
        let html = render("1 < 2 & 3");
        assert!(html.contains("1 &lt; 2 &amp; 3"), "got: {html}");
    }
}
