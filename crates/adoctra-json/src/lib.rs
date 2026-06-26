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
        BlockKind::Heading { level, id, inlines } => {
            s.push_str(&format!("{{\"name\":\"heading\",\"level\":{level},\"id\":"));
            match id {
                Some(id) => escape_into(id, s),
                None => s.push_str("null"),
            }
            s.push_str(",\"inlines\":");
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
        BlockKind::List { ordered, items } => {
            s.push_str(&format!(
                "{{\"name\":\"list\",\"ordered\":{ordered},\"items\":["
            ));
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str("{\"inlines\":");
                write_inlines(&item.inlines, s);
                // Nested blocks (e.g. a sublist) under this item.
                s.push_str(",\"blocks\":[");
                for (j, child) in item.blocks.iter().enumerate() {
                    if j > 0 {
                        s.push(',');
                    }
                    write_block(child, s);
                }
                s.push_str("]}");
            }
            s.push_str("]}");
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
            Inline::Link {
                target,
                children,
                bare,
                ..
            } => {
                s.push_str("{\"name\":\"link\",\"target\":");
                escape_into(target, s);
                s.push_str(&format!(",\"bare\":{bare},\"inlines\":"));
                write_inlines(children, s);
                s.push('}');
            }
            Inline::CrossReference {
                target, children, ..
            } => {
                s.push_str("{\"name\":\"xref\",\"target\":");
                escape_into(target, s);
                s.push_str(",\"inlines\":");
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
    fn doctitle_heading_has_null_id() {
        // The level-0 document title is not a section.
        let json = render("= Title");
        assert!(json.contains("\"level\":0,\"id\":null"), "got: {json}");
    }

    #[test]
    fn section_heading_serializes_auto_id() {
        let json = render("== Hello World\n\nbody");
        assert!(json.contains("\"id\":\"_hello_world\""), "got: {json}");
    }

    #[test]
    fn resolved_xref_serializes_section_title() {
        let json = render("== Section Two\n\nSee <<_section_two>>.");
        assert!(json.contains("\"name\":\"xref\""), "got: {json}");
        assert!(json.contains("\"target\":\"_section_two\""), "got: {json}");
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"Section Two\"}"),
            "got: {json}"
        );
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
    fn link_serializes_with_target_and_bare_flag() {
        let json = render("See link:https://example.org[the site].");
        assert!(json.contains("\"name\":\"link\""), "got: {json}");
        assert!(
            json.contains("\"target\":\"https://example.org\""),
            "got: {json}"
        );
        assert!(json.contains("\"bare\":false"), "got: {json}");
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"the site\"}"),
            "got: {json}"
        );
    }

    #[test]
    fn bare_autolink_sets_bare_true() {
        let json = render("Bare https://example.org now.");
        assert!(json.contains("\"name\":\"link\""), "got: {json}");
        assert!(json.contains("\"bare\":true"), "got: {json}");
        assert!(
            json.contains("\"target\":\"https://example.org\""),
            "got: {json}"
        );
    }

    #[test]
    fn xref_serializes_with_target_and_inlines() {
        let json = render("See <<sec,Section Two>>.");
        assert!(json.contains("\"name\":\"xref\""), "got: {json}");
        assert!(json.contains("\"target\":\"sec\""), "got: {json}");
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"Section Two\"}"),
            "got: {json}"
        );
    }

    #[test]
    fn textless_xref_serializes_bracketed_id() {
        let json = render("See <<sec>>.");
        assert!(json.contains("\"name\":\"xref\""), "got: {json}");
        assert!(json.contains("\"target\":\"sec\""), "got: {json}");
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"[sec]\"}"),
            "got: {json}"
        );
    }

    #[test]
    fn xref_macro_serializes_like_angle_form() {
        let json = render("See xref:sec[Mac].");
        assert!(json.contains("\"name\":\"xref\""), "got: {json}");
        assert!(json.contains("\"target\":\"sec\""), "got: {json}");
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"Mac\"}"),
            "got: {json}"
        );
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

    #[test]
    fn unordered_list_serializes_by_name() {
        let json = render("* one\n* two");
        assert!(json.contains("\"name\":\"list\""), "got: {json}");
        assert!(json.contains("\"ordered\":false"), "got: {json}");
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"one\"}"),
            "got: {json}"
        );
    }

    #[test]
    fn ordered_list_sets_ordered_true() {
        let json = render(". a\n. b");
        assert!(json.contains("\"name\":\"list\""), "got: {json}");
        assert!(json.contains("\"ordered\":true"), "got: {json}");
    }

    #[test]
    fn explicit_numbered_list_sets_ordered_true() {
        // `1.`/`2.` reuse the ordered-list node, serialized like `.`.
        let json = render("1. a\n2. b");
        assert!(json.contains("\"name\":\"list\""), "got: {json}");
        assert!(json.contains("\"ordered\":true"), "got: {json}");
    }

    #[test]
    fn list_item_nests_inline_children() {
        let json = render("* a *b*");
        assert!(json.contains("\"name\":\"list\""), "got: {json}");
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");
    }

    #[test]
    fn nested_list_serializes_under_item_blocks() {
        let json = render("* one\n** a");
        // The parent item carries the sublist in its "blocks".
        assert!(
            json.contains("\"blocks\":[{\"name\":\"list\""),
            "got: {json}"
        );
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"a\"}"),
            "got: {json}"
        );
        // The leaf item has an empty "blocks".
        assert!(json.contains("\"blocks\":[]}"), "got: {json}");
    }

    #[test]
    fn continuation_paragraph_serializes_in_item_blocks() {
        // `+` attaches a paragraph as a block inside the item's "blocks".
        let json = render("* one\n+\nattached\n* two");
        assert!(
            json.contains("\"blocks\":[{\"name\":\"paragraph\""),
            "got: {json}"
        );
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"attached\"}"),
            "got: {json}"
        );
    }

    #[test]
    fn multi_line_item_text_serializes_folded() {
        // A wrapped line folds into the item's text — its newline and indent
        // become spaces (the resolved content), while the source is two lines.
        let json = render("* one\n  two");
        assert!(json.contains("\"name\":\"list\""), "got: {json}");
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"one   two\"}"),
            "got: {json}"
        );
    }

    #[test]
    fn typographic_replacements_emit_the_literal_char() {
        // Unlike HTML (which uses numeric entities), the JSON backend renders the
        // resolved Unicode character verbatim — encoding is a per-backend choice.
        let json = render("(C) and it's");
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"\u{a9} and it\u{2019}s\"}"),
            "got: {json}"
        );
        assert!(!json.contains("&#169;"), "should not HTML-encode: {json}");
    }
}
