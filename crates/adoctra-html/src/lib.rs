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
    // A block's anchor (an explicit `[[id]]` or a section's auto-generated id)
    // renders as an `id` attribute on the block's element; `None` → no attribute.
    let id = id_attr(&block.id);
    match &block.kind {
        BlockKind::Heading { level, inlines } => {
            let h = (level + 1).min(6);
            out.push_str(&format!("<h{h}{id}>"));
            render_inlines(inlines, out);
            out.push_str(&format!("</h{h}>\n"));
        }
        BlockKind::Paragraph { inlines } => {
            out.push_str(&format!("<p{id}>"));
            render_inlines(inlines, out);
            out.push_str("</p>\n");
        }
        BlockKind::Listing { text } => {
            out.push_str(&format!("<pre class=\"listing\"{id}>"));
            out.push_str(&escape(text));
            out.push_str("</pre>\n");
        }
        BlockKind::List { ordered, items } => {
            let tag = if *ordered { "ol" } else { "ul" };
            out.push_str(&format!("<{tag}{id}>\n"));
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
            Inline::Text { value, .. } => out.push_str(&escape_text(value)),
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
            // A bare link (no explicit text) gets `class="bare"`, matching
            // asciidoctor; the href and display text are otherwise independent.
            Inline::Link {
                target,
                children,
                bare,
                ..
            } => {
                out.push_str("<a href=\"");
                out.push_str(&escape(target));
                if *bare {
                    out.push_str("\" class=\"bare");
                }
                out.push_str("\">");
                render_inlines(children, out);
                out.push_str("</a>");
            }
            // A cross-reference renders as an internal link `href="#id"`. The
            // display text (explicit reftext, or the core-generated `[id]`) is in
            // `children`; no `class="bare"` (that is a link-only concern).
            Inline::CrossReference {
                target, children, ..
            } => {
                out.push_str("<a href=\"#");
                out.push_str(&escape(target));
                out.push_str("\">");
                render_inlines(children, out);
                out.push_str("</a>");
            }
            // Resolved in core: emit the value, or fall back to the literal
            // `{name}` for a missing attribute (asciidoctor's default).
            Inline::AttributeRef { name, value, .. } => match value {
                Some(v) => out.push_str(&escape_text(v)),
                None => out.push_str(&escape(&format!("{{{name}}}"))),
            },
        }
    }
}

/// An ` id="…"` attribute (with a leading space) for a block that carries an
/// anchor, or the empty string. The id is escaped as an HTML attribute value.
fn id_attr(id: &Option<String>) -> String {
    match id {
        Some(id) => format!(" id=\"{}\"", escape(id)),
        None => String::new(),
    }
}

/// Output-encoding for HTML. Deliberately a *backend* concern: the shared model
/// keeps text literal, and each backend escapes for its own format. Used for
/// verbatim text (listing blocks) and link targets, where typographic
/// characters are not remapped.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Output-encoding for resolved *inline text*. Beyond the usual `&<>` escaping,
/// the typographic characters core produces for replacements (`(C)` -> `©`,
/// `--` -> `—`, `...` -> `…`, the arrows, …) are emitted as numeric character
/// references, matching asciidoctor's HTML byte-for-byte. Choosing entities
/// over the literal UTF-8 char is an HTML-encoding decision, so it lives here in
/// the backend rather than in the format-neutral core.
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{a9}' => out.push_str("&#169;"),
            '\u{ae}' => out.push_str("&#174;"),
            '\u{2122}' => out.push_str("&#8482;"),
            '\u{2009}' => out.push_str("&#8201;"),
            '\u{2014}' => out.push_str("&#8212;"),
            '\u{200b}' => out.push_str("&#8203;"),
            '\u{2026}' => out.push_str("&#8230;"),
            '\u{2019}' => out.push_str("&#8217;"),
            '\u{2192}' => out.push_str("&#8594;"),
            '\u{21d2}' => out.push_str("&#8658;"),
            '\u{2190}' => out.push_str("&#8592;"),
            '\u{21d0}' => out.push_str("&#8656;"),
            _ => out.push(ch),
        }
    }
    out
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
    fn renders_link_macro_with_text() {
        let html = render("See link:https://example.org[the site].");
        assert!(
            html.contains("<a href=\"https://example.org\">the site</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn bare_autolink_gets_the_bare_class() {
        let html = render("Bare https://example.org now.");
        assert!(
            html.contains("<a href=\"https://example.org\" class=\"bare\">https://example.org</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn link_text_keeps_inline_markup() {
        let html = render("link:https://h[*bold* text]");
        assert!(
            html.contains("<a href=\"https://h\"><strong>bold</strong> text</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn mailto_macro_renders_scheme_in_href() {
        let html = render("Mail mailto:me@x.io[me].");
        assert!(
            html.contains("<a href=\"mailto:me@x.io\">me</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn xref_with_text_renders_internal_link() {
        let html = render("See <<sec,Section Two>>.");
        assert!(
            html.contains("<a href=\"#sec\">Section Two</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn textless_xref_to_unknown_id_renders_bracketed_id() {
        // An *unresolved* textless `<<id>>` falls back to `[id]` (still inside
        // the anchor), matching asciidoctor.
        let html = render("See <<sec>>.");
        assert!(html.contains("<a href=\"#sec\">[sec]</a>"), "got: {html}");
    }

    #[test]
    fn xref_macro_renders_internal_link() {
        let html = render("See xref:sec[Mac].");
        assert!(html.contains("<a href=\"#sec\">Mac</a>"), "got: {html}");
    }

    #[test]
    fn section_heading_renders_with_auto_id() {
        let html = render("== Hello World\n\nbody");
        assert!(
            html.contains("<h2 id=\"_hello_world\">Hello World</h2>"),
            "got: {html}"
        );
    }

    #[test]
    fn doctitle_has_no_id() {
        // The level-0 document title is not a section, so it carries no id.
        let html = render("= Doc Title\n\nbody");
        assert!(html.contains("<h1>Doc Title</h1>"), "got: {html}");
    }

    #[test]
    fn resolved_xref_renders_section_title() {
        let html = render("== Section Two\n\nSee <<_section_two>>.");
        assert!(
            html.contains("<a href=\"#_section_two\">Section Two</a>"),
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
    fn explicit_numbered_list_renders_as_ol() {
        // `1.`/`2.` reuse the ordered-list node, so they render like `.`.
        let html = render("1. first\n2. second");
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

    #[test]
    fn typographic_replacements_render_as_numeric_entities() {
        // The HTML backend encodes the resolved typographic chars as numeric
        // character references, matching asciidoctor.
        let html = render("(C) (R) (TM)");
        assert!(html.contains("<p>&#169; &#174; &#8482;</p>"), "got: {html}");

        let html = render("A -- B and word--word");
        assert!(
            html.contains("<p>A&#8201;&#8212;&#8201;B and word&#8212;&#8203;word</p>"),
            "got: {html}"
        );

        let html = render("Wait... it's done");
        assert!(
            html.contains("<p>Wait&#8230;&#8203; it&#8217;s done</p>"),
            "got: {html}"
        );

        // Arrows, including the `<=>` -> `<` + `⇒` precedence (the `<` then
        // escapes to `&lt;`).
        let html = render("-> => <- <= and <=>");
        assert!(
            html.contains("<p>&#8594; &#8658; &#8592; &#8656; and &lt;&#8658;</p>"),
            "got: {html}"
        );
    }

    #[test]
    fn escaped_replacement_renders_literally() {
        let html = render("escaped \\(C) and \\-> here");
        assert!(
            html.contains("<p>escaped (C) and -&gt; here</p>"),
            "got: {html}"
        );
    }

    #[test]
    fn anchored_paragraph_renders_id_attribute() {
        let html = render("[[pa]]\nHello.");
        assert!(html.contains("<p id=\"pa\">Hello.</p>"), "got: {html}");
    }

    #[test]
    fn xref_to_anchor_reftext_renders_reftext() {
        let html = render("[[pa,The Para]]\nHi.\n\nSee <<pa>>.");
        assert!(html.contains("<a href=\"#pa\">The Para</a>"), "got: {html}");
    }

    #[test]
    fn explicit_anchor_on_section_overrides_auto_id() {
        // `[[custom]]` replaces the auto id `_real_title` on the <h2>.
        let html = render("[[custom]]\n== Real Title\n\nbody");
        assert!(
            html.contains("<h2 id=\"custom\">Real Title</h2>"),
            "got: {html}"
        );
    }
}
