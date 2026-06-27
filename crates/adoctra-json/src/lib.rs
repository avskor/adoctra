//! JSON (ASG) backend.
//!
//! Emits a JSON tree of the document; the natural place to target the official
//! AsciiDoc Abstract Syntax Graph and act as a TCK adapter. Hand-serialized to
//! stay dependency-free; switch to serde for the real ASG shape.

use adoctra_core::ast::{Block, BlockKind, Inline};
use adoctra_core::json::escape_into;
use adoctra_core::{Backend, Document, Result};
use std::collections::BTreeMap;

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
            s.push_str(&format!("{{\"name\":\"heading\",\"level\":{level},"));
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            s.push_str(",\"inlines\":");
            write_inlines(inlines, s);
            s.push('}');
        }
        BlockKind::Paragraph { inlines } => {
            s.push_str("{\"name\":\"paragraph\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            s.push_str(",\"inlines\":");
            write_inlines(inlines, s);
            s.push('}');
        }
        BlockKind::Listing { text } => {
            s.push_str("{\"name\":\"listing\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            s.push_str(",\"text\":");
            escape_into(text, s);
            s.push('}');
        }
        BlockKind::Literal { text } => {
            s.push_str("{\"name\":\"literal\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            s.push_str(",\"text\":");
            escape_into(text, s);
            s.push('}');
        }
        BlockKind::List { ordered, items } => {
            s.push_str(&format!("{{\"name\":\"list\",\"ordered\":{ordered},"));
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            s.push_str(",\"items\":[");
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
        BlockKind::Admonition { kind, blocks } => {
            s.push_str(&format!(
                "{{\"name\":\"admonition\",\"admonition-type\":\"{}\",",
                kind.as_str()
            ));
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            s.push_str(",\"blocks\":[");
            for (i, child) in blocks.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push_str("]}");
        }
        BlockKind::Example { blocks } => {
            s.push_str("{\"name\":\"example\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            s.push_str(",\"blocks\":[");
            for (i, child) in blocks.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push_str("]}");
        }
    }
}

/// Emit the `"id":<string|null>` field for a block's anchor.
fn write_id(id: &Option<String>, s: &mut String) {
    s.push_str("\"id\":");
    match id {
        Some(id) => escape_into(id, s),
        None => s.push_str("null"),
    }
}

/// Emit the leading-comma `,"roles":[…]` field for a block's `[.role]` shorthand
/// roles (an empty array when there are none).
fn write_roles(roles: &[String], s: &mut String) {
    s.push_str(",\"roles\":[");
    for (i, role) in roles.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        escape_into(role, s);
    }
    s.push(']');
}

/// Emit the leading-comma `,"style":<string|null>` field — the block style (the
/// first positional attribute), or `null`.
fn write_style(style: &Option<String>, s: &mut String) {
    s.push_str(",\"style\":");
    match style {
        Some(v) => escape_into(v, s),
        None => s.push_str("null"),
    }
}

/// Emit the leading-comma `,"attributes":{…}` field — named `name=value` pairs.
/// Keys come from a `BTreeMap`, so the order is deterministic.
fn write_attributes(named: &BTreeMap<String, String>, s: &mut String) {
    s.push_str(",\"attributes\":{");
    for (i, (k, v)) in named.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        escape_into(k, s);
        s.push(':');
        escape_into(v, s);
    }
    s.push('}');
}

/// Emit the leading-comma `,"options":[…]` field — `%option` flags in source
/// order (an empty array when there are none).
fn write_options(options: &[String], s: &mut String) {
    s.push_str(",\"options\":[");
    for (i, opt) in options.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        escape_into(opt, s);
    }
    s.push(']');
}

/// Emit the leading-comma `,"title":<array|null>` field — the block's `.Title`
/// inlines (so formatting is preserved), or `null` when the block has no title.
fn write_title(title: &Option<Vec<Inline>>, s: &mut String) {
    s.push_str(",\"title\":");
    match title {
        Some(inlines) => write_inlines(inlines, s),
        None => s.push_str("null"),
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
            // An inline anchor: an empty `<a id>` target. Its reftext is resolve
            // metadata (the display text of a textless xref), not content, so —
            // like a block anchor's reftext — only the id is serialized.
            Inline::Anchor { id, .. } => {
                s.push_str("{\"name\":\"anchor\",\"id\":");
                escape_into(id, s);
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
        // No attribute-ref *node* leaks through (it resolved to text). The
        // block-level `"attributes"` field is a separate, legitimate concern.
        assert!(!json.contains("\"name\":\"attribute"), "got: {json}");
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

    #[test]
    fn anchored_paragraph_serializes_its_id() {
        let json = render("[[pa]]\nHi.");
        assert!(
            json.contains("\"name\":\"paragraph\",\"id\":\"pa\""),
            "got: {json}"
        );
    }

    #[test]
    fn plain_paragraph_serializes_null_id() {
        let json = render("Hi.");
        assert!(
            json.contains("\"name\":\"paragraph\",\"id\":null"),
            "got: {json}"
        );
    }

    #[test]
    fn inline_anchor_serializes_with_id() {
        // The inline `[[point]]` node carries its id; the reftext is resolve-only.
        let json = render("x [[point]]y");
        assert!(
            json.contains("{\"name\":\"anchor\",\"id\":\"point\"}"),
            "got: {json}"
        );
    }

    #[test]
    fn shorthand_id_and_roles_serialize() {
        let json = render("[#intro.lead]\nHi.");
        assert!(
            json.contains("\"name\":\"paragraph\",\"id\":\"intro\",\"roles\":[\"lead\"]"),
            "got: {json}"
        );
    }

    #[test]
    fn multiple_roles_serialize_as_array() {
        let json = render("[.a.b]\nHi.");
        assert!(json.contains("\"roles\":[\"a\",\"b\"]"), "got: {json}");
    }

    #[test]
    fn plain_paragraph_serializes_empty_roles() {
        let json = render("Hi.");
        assert!(json.contains("\"id\":null,\"roles\":[]"), "got: {json}");
    }

    #[test]
    fn source_style_and_language_serialize() {
        let json = render("[source,ruby]\n----\nx\n----");
        assert!(
            json.contains(
                "\"style\":\"source\",\"attributes\":{\"language\":\"ruby\"},\"options\":[]"
            ),
            "got: {json}"
        );
    }

    #[test]
    fn options_serialize_as_array() {
        let json = render("[%hardbreaks]\nHi.");
        assert!(json.contains("\"options\":[\"hardbreaks\"]"), "got: {json}");
    }

    #[test]
    fn plain_paragraph_serializes_empty_attrs() {
        let json = render("Hi.");
        assert!(
            json.contains("\"roles\":[],\"style\":null,\"attributes\":{},\"options\":[]"),
            "got: {json}"
        );
    }

    #[test]
    fn delimited_literal_serializes_by_name() {
        let json = render("....\nverbatim *not bold*\n....");
        assert!(json.contains("\"name\":\"literal\""), "got: {json}");
        assert!(
            json.contains("\"text\":\"verbatim *not bold*\""),
            "got: {json}"
        );
    }

    #[test]
    fn literal_style_paragraph_serializes_by_name() {
        // `[literal]` paragraph: a literal node carrying the verbatim text and the
        // `literal` style.
        let json = render("[literal]\na *b* c");
        assert!(json.contains("\"name\":\"literal\""), "got: {json}");
        assert!(json.contains("\"style\":\"literal\""), "got: {json}");
        assert!(json.contains("\"text\":\"a *b* c\""), "got: {json}");
    }

    #[test]
    fn note_style_serializes_as_admonition() {
        let json = render("[NOTE]\nThis is a *note*.");
        assert!(json.contains("\"name\":\"admonition\""), "got: {json}");
        assert!(json.contains("\"admonition-type\":\"note\""), "got: {json}");
        // The body is a nested paragraph that keeps its inline markup (unlike a
        // verbatim literal).
        assert!(json.contains("\"blocks\":["), "got: {json}");
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");
    }

    #[test]
    fn label_form_serializes_as_admonition() {
        let json = render("TIP: be *bold*");
        assert!(json.contains("\"name\":\"admonition\""), "got: {json}");
        assert!(json.contains("\"admonition-type\":\"tip\""), "got: {json}");
        assert!(json.contains("\"blocks\":["), "got: {json}");
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");
    }

    #[test]
    fn delimited_admonition_serializes_block_body() {
        // `[NOTE]` on a `====` block serializes as an admonition whose body is the
        // nested child blocks (a paragraph and a listing here).
        let json = render("[NOTE]\n====\nIntro.\n\n----\ncode\n----\n====");
        assert!(json.contains("\"name\":\"admonition\""), "got: {json}");
        assert!(json.contains("\"admonition-type\":\"note\""), "got: {json}");
        assert!(json.contains("\"blocks\":["), "got: {json}");
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
        assert!(json.contains("\"name\":\"listing\""), "got: {json}");
    }

    #[test]
    fn example_block_serializes_nested_blocks() {
        let json = render("====\nHi *there*.\n====");
        assert!(json.contains("\"name\":\"example\""), "got: {json}");
        // The child paragraph (with its own inline markup) is nested under blocks.
        assert!(json.contains("\"blocks\":["), "got: {json}");
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");
    }

    #[test]
    fn block_title_serializes_as_inlines() {
        // The title is the block's parsed inlines, so formatting is preserved.
        let json = render(".A *bold* caption\nBody.");
        assert!(
            json.contains("\"title\":[{\"name\":\"text\",\"value\":\"A \"},{\"name\":\"strong\""),
            "got: {json}"
        );
    }

    #[test]
    fn untitled_block_serializes_null_title() {
        let json = render("Body.");
        assert!(json.contains("\"title\":null"), "got: {json}");
    }

    #[test]
    fn example_block_title_serializes_on_the_container() {
        // The `.Title` stays on the example container, its body under `blocks`.
        let json = render(".Demo\n====\nhi\n====");
        assert!(json.contains("\"name\":\"example\""), "got: {json}");
        assert!(
            json.contains("\"title\":[{\"name\":\"text\",\"value\":\"Demo\"}]"),
            "got: {json}"
        );
    }
}
