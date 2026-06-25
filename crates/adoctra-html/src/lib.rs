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
        BlockKind::List { ordered, items } => {
            let tag = if *ordered { "ol" } else { "ul" };
            out.push_str(&format!("<{tag}>\n"));
            for item in items {
                out.push_str("<li>");
                render_inlines(&item.inlines, out);
                // Nested blocks (a sublist, or a `+`-attached paragraph/listing)
                // render inside the <li>, after the item's principal text. Each
                // starts on its own line, matching asciidoctor's structure and
                // keeping the principal text from running into the block's text.
                for child in &item.blocks {
                    out.push('\n');
                    render_block(child, out);
                }
                out.push_str("</li>\n");
            }
            out.push_str(&format!("</{tag}>\n"));
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
            Inline::Monospace { children, .. } => {
                out.push_str("<code>");
                render_inlines(children, out);
                out.push_str("</code>");
            }
            Inline::Mark { children, .. } => {
                out.push_str("<mark>");
                render_inlines(children, out);
                out.push_str("</mark>");
            }
            Inline::Superscript { children, .. } => {
                out.push_str("<sup>");
                render_inlines(children, out);
                out.push_str("</sup>");
            }
            Inline::Subscript { children, .. } => {
                out.push_str("<sub>");
                render_inlines(children, out);
                out.push_str("</sub>");
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
    fn renders_monospace_mark_super_and_subscript() {
        let html = render("A `code`, #hot#, E=mc^2^, H~2~O.");
        assert!(html.contains("<code>code</code>"), "got: {html}");
        assert!(html.contains("<mark>hot</mark>"), "got: {html}");
        assert!(html.contains("mc<sup>2</sup>"), "got: {html}");
        assert!(html.contains("H<sub>2</sub>O"), "got: {html}");
    }

    #[test]
    fn monospace_renders_nested_markup() {
        let html = render("Run `*bold* code`.");
        assert!(
            html.contains("<code><strong>bold</strong> code</code>"),
            "got: {html}"
        );
    }

    #[test]
    fn text_is_html_escaped_by_the_backend() {
        // The model keeps text literal; escaping is the backend's job.
        let html = render("1 < 2 & 3");
        assert!(html.contains("1 &lt; 2 &amp; 3"), "got: {html}");
    }

    #[test]
    fn renders_unordered_list() {
        let html = render("* one\n* two");
        assert!(html.contains("<ul>"), "got: {html}");
        assert!(html.contains("<li>one</li>"), "got: {html}");
        assert!(html.contains("<li>two</li>"), "got: {html}");
        assert!(html.contains("</ul>"), "got: {html}");
    }

    #[test]
    fn dash_list_also_renders_ul() {
        let html = render("- a\n- b");
        assert!(html.contains("<ul>"), "got: {html}");
    }

    #[test]
    fn renders_ordered_list() {
        let html = render(". first\n. second");
        assert!(html.contains("<ol>"), "got: {html}");
        assert!(html.contains("<li>first</li>"), "got: {html}");
        assert!(html.contains("</ol>"), "got: {html}");
    }

    #[test]
    fn list_item_keeps_inline_markup() {
        let html = render("* a *b*");
        assert!(
            html.contains("<li>a <strong>b</strong></li>"),
            "got: {html}"
        );
    }

    #[test]
    fn nested_list_renders_inside_parent_li() {
        let html = render("* one\n** a\n** b\n* two");
        // Two <ul> levels; the nested one opens inside the first item.
        assert_eq!(html.matches("<ul>").count(), 2, "got: {html}");
        // A nested block opens on its own line, after the item's principal text.
        assert!(html.contains("<li>one\n<ul>"), "got: {html}");
        assert!(html.contains("<li>a</li>"), "got: {html}");
        assert!(html.contains("<li>b</li>"), "got: {html}");
        assert!(html.contains("<li>two</li>"), "got: {html}");
    }

    #[test]
    fn nested_ordered_under_unordered() {
        // A unique marker nests across kinds: `. b` becomes an <ol> in the <li>.
        let html = render("* a\n. b");
        assert!(html.contains("<ul>"), "got: {html}");
        assert!(html.contains("<ol>"), "got: {html}");
        assert!(html.contains("<li>a\n<ol>"), "got: {html}");
        assert!(html.contains("<li>b</li>"), "got: {html}");
    }

    #[test]
    fn continuation_paragraph_renders_inside_li() {
        // `+` attaches a paragraph block; it renders inside the <li>, after the
        // item's principal text (the same recursion that renders a sublist).
        let html = render("* one\n+\nattached\n* two");
        assert!(html.contains("<li>one\n<p>attached</p>"), "got: {html}");
        assert!(html.contains("<li>two</li>"), "got: {html}");
    }

    #[test]
    fn continuation_listing_renders_inside_li() {
        let html = render("* one\n+\n----\ncode\n----");
        assert!(
            html.contains("<li>one\n<pre class=\"listing\">code</pre>"),
            "got: {html}"
        );
    }

    #[test]
    fn multi_line_item_text_folds_into_one_li() {
        // A wrapped line folds into the item's text (one <li>, no extra item).
        let html = render("* one\n  two");
        assert_eq!(html.matches("<li>").count(), 1, "got: {html}");
        assert!(html.contains("two</li>"), "got: {html}");
    }
}
