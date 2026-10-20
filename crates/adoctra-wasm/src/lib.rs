//! WASM bindings for an AsciiDoc editor.
//!
//! Exposes only what an editor needs — never the converters:
//!   * [`parse_json`]  — error-tolerant parse → JSON `{ document, diagnostics, tokens }`
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
//! On the JS side: `JSON.parse(parse_json(src))` gives `{ document, diagnostics, tokens }`,
//! and diagnostics/tokens carry byte spans (`{ source_id, start, end }`). The
//! host maps source ids to buffers and converts UTF-8 byte offsets into its
//! editor's coordinate system (for example UTF-16 positions).

mod resources;
pub use resources::ResourceWorkspace;

use adoctra_core::ast::{Attribution, Block, BlockKind, Inline, Span};
use adoctra_core::diagnostic::{Diagnostic, ParseResult};
use adoctra_core::json::escape_into as write_str;
use adoctra_core::{Token, TokenKind};
use std::collections::BTreeMap;

/// Parse `src` once and return document, diagnostics and source highlight tokens.
pub fn parse_json(src: &str) -> String {
    parse_json_with_source(src, 0)
}

/// Parse an editor buffer using the source id assigned by the host application.
pub fn parse_json_with_source(src: &str, source_id: usize) -> String {
    let options = adoctra_core::ParseOptions {
        source_id: adoctra_core::SourceId(source_id),
        ..Default::default()
    };
    let result = adoctra_core::parse_with_options(src, &options);
    serialize_parse_result(&result)
}

/// Tokenize `src` and return a JSON array of `{ "span": {...}, "kind": "..." }`.
pub fn tokens_json(src: &str) -> String {
    tokens_json_with_source(src, 0)
}

pub fn tokens_json_with_source(src: &str, source_id: usize) -> String {
    let options = adoctra_core::ParseOptions {
        source_id: adoctra_core::SourceId(source_id),
        ..Default::default()
    };
    let toks = adoctra_core::tokens_with_options(src, &options);
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
    s.push(']');
    s.push_str(",\"tokens\":");
    s.push_str(&serialize_tokens(&r.tokens));
    for (name, spans) in [
        ("comments", &r.comments),
        ("conditional_directives", &r.conditional_directives),
        ("inactive", &r.inactive),
        ("include_directives", &r.include_directives),
    ] {
        if spans.is_empty() {
            continue;
        }
        s.push(',');
        write_str(name, &mut s);
        s.push_str(":[");
        for (i, span) in spans.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('{');
            write_span_field(span, &mut s);
            s.push('}');
        }
        s.push(']');
    }
    if !r.provenance.is_empty() {
        s.push_str(",\"provenance\":[");
        for (i, fragments) in r.provenance.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('[');
            for (j, span) in fragments.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                write_span(span, &mut s);
            }
            s.push(']');
        }
        s.push(']');
    }
    s.push('}');
    s
}

/// A manual page's names and the NAME section its header read, with the
/// range of its name and purpose lines (`null` when they do not conform).
fn write_manpage(manpage: &adoctra_core::Manpage, s: &mut String) {
    s.push_str(",\"manpage\":{\"names\":[");
    for (i, name) in manpage.names.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_str(name, s);
    }
    s.push_str("],\"name_section\":");
    match &manpage.name_section {
        Some(section) => {
            s.push_str("{\"heading\":");
            write_block(&section.heading, s);
            s.push_str(",\"body\":");
            match &section.body {
                Some(body) => write_span(body, s),
                None => s.push_str("null"),
            }
            s.push('}');
        }
        None => s.push_str("null"),
    }
    s.push('}');
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
    s.push('}');
    adoctra_core::json::write_attribute_parts_map(&doc.attributes, s);
    s.push_str(",\"blocks\":[");
    for (i, b) in doc.blocks.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_block(b, s);
    }
    s.push(']');
    adoctra_core::json::write_footnote_definitions(&doc.footnotes, s, Some(write_span_field));
    if let Some(header) = &doc.header {
        s.push_str(",\"header\":");
        write_header(header, s);
    }
    if let Some(manpage) = &doc.manpage {
        write_manpage(manpage, s);
    }
    s.push('}');
}

// Header declarations share the core model; effective overrides remain attributes.
fn write_header(header: &adoctra_core::DocumentHeader, s: &mut String) {
    s.push('{');
    write_span_field(&header.span, s);
    s.push_str(",\"title_span\":");
    write_span(&header.title_span, s);
    s.push(',');
    s.push_str("\"authors\":[");
    for (i, author) in header.authors.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('{');
        write_span_field(&author.span, s);
        s.push(',');
        for (j, (key, value)) in [
            ("name", Some(author.name.as_str())),
            ("firstname", Some(author.firstname.as_str())),
            ("middlename", author.middlename.as_deref()),
            ("lastname", author.lastname.as_deref()),
            ("initials", Some(author.initials.as_str())),
            ("email", author.email.as_deref()),
        ]
        .into_iter()
        .enumerate()
        {
            if j > 0 {
                s.push(',');
            }
            write_str(key, s);
            s.push(':');
            if let Some(value) = value {
                write_str(value, s);
            } else {
                s.push_str("null");
            }
        }
        s.push('}');
    }
    s.push_str("],\"revision\":");
    if let Some(revision) = &header.revision {
        s.push('{');
        write_span_field(&revision.span, s);
        s.push(',');
        for (i, (key, value)) in [
            ("number", &revision.number),
            ("date", &revision.date),
            ("remark", &revision.remark),
        ]
        .into_iter()
        .enumerate()
        {
            if i > 0 {
                s.push(',');
            }
            write_str(key, s);
            s.push(':');
            if let Some(value) = value {
                write_str(value, s);
            } else {
                s.push_str("null");
            }
        }
        s.push('}');
    } else {
        s.push_str("null");
    }
    s.push_str(",\"title\":");
    write_str(&header.title, s);
    s.push('}');
}

/// A table: its resolved presentation (`width` percentage, `frame`, `grid`,
/// `stripes`), the columns and the head/body/foot rows of cells. A cell has
/// `paragraphs` (arrays of inlines; one run in the head), the `blocks` of an
/// AsciiDoc cell or verbatim `literal` text.
fn write_table(table: &adoctra_core::ast::Table, s: &mut String) {
    s.push_str(&format!(",\"width\":{},\"frame\":", table.width));
    write_str(&table.frame, s);
    s.push_str(",\"grid\":");
    write_str(&table.grid, s);
    s.push_str(",\"stripes\":");
    match &table.stripes {
        Some(stripes) => write_str(stripes, s),
        None => s.push_str("null"),
    }
    s.push_str(",\"columns\":[");
    for (i, column) in table.columns.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"halign\":\"{}\",\"valign\":\"{}\",\"style\":{},\"width\":",
            column.halign.as_str(),
            column.valign.as_str(),
            column
                .style
                .map_or("null".to_owned(), |style| format!("\"{}\"", style.as_str()))
        ));
        write_str(&column.width, s);
        s.push_str(&format!(",\"autowidth\":{}}}", column.autowidth));
    }
    s.push(']');
    for (name, rows) in [
        ("head", &table.head),
        ("body", &table.body),
        ("foot", &table.foot),
    ] {
        s.push_str(&format!(",\"{name}\":["));
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('[');
            for (j, cell) in row.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                write_cell(cell, s);
            }
            s.push(']');
        }
        s.push(']');
    }
}

fn write_cell(cell: &adoctra_core::ast::TableCell, s: &mut String) {
    let number = |n: Option<u32>| n.map_or("null".to_owned(), |n| n.to_string());
    s.push('{');
    write_span_field(&cell.span, s);
    s.push(',');
    s.push_str(&format!(
        "\"style\":{},\"halign\":\"{}\",\"valign\":\"{}\",\"colspan\":{},\"rowspan\":{},\"background\":",
        cell.style
            .map_or("null".to_owned(), |style| format!("\"{}\"", style.as_str())),
        cell.halign.as_str(),
        cell.valign.as_str(),
        number(cell.colspan),
        number(cell.rowspan)
    ));
    match &cell.background {
        Some(color) => write_str(color, s),
        None => s.push_str("null"),
    }
    match &cell.content {
        adoctra_core::ast::CellContent::Paragraphs(paragraphs) => {
            s.push_str(",\"paragraphs\":[");
            for (i, inlines) in paragraphs.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_inlines(inlines, s);
            }
            s.push(']');
        }
        adoctra_core::ast::CellContent::Blocks {
            blocks, footnotes, ..
        } => {
            s.push_str(",\"blocks\":[");
            for (i, block) in blocks.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_block(block, s);
            }
            s.push(']');
            adoctra_core::json::write_footnote_definitions(footnotes, s, Some(write_span_field));
        }
        adoctra_core::ast::CellContent::Literal(text) => {
            s.push_str(",\"literal\":");
            write_str(text, s);
        }
    }
    s.push('}');
}

/// A callout icon captured where its block converts: the `icons` mode and
/// the image icon (empty unless icons are images).
fn write_icon(icons: &Option<String>, src: &str, s: &mut String) {
    s.push_str(",\"icons\":");
    match icons {
        Some(icons) => write_str(icons, s),
        None => s.push_str("null"),
    }
    s.push_str(",\"src\":");
    write_str(src, s);
}

/// A verbatim block's callout marks (omitted when there are none): number,
/// guard, `<!--N-->` form, Ruby's id and the captured icon.
fn write_callouts(callouts: &[adoctra_core::ast::Callout], s: &mut String) {
    if callouts.is_empty() {
        return;
    }
    s.push_str(",\"callouts\":[");
    for (i, callout) in callouts.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('{');
        write_span_field(&callout.span, s);
        s.push(',');
        s.push_str("\"number\":");
        write_str(&callout.number, s);
        s.push_str(",\"id\":");
        write_str(&callout.id, s);
        s.push_str(",\"guard\":");
        match &callout.guard {
            Some(guard) => write_str(guard, s),
            None => s.push_str("null"),
        }
        s.push_str(&format!(",\"xml\":{}", callout.xml));
        write_icon(&callout.icons, &callout.src, s);
        s.push('}');
    }
    s.push(']');
}

/// A list item (or a description) with its span, text, nested blocks and, in
/// a checklist, its `checkbox` state.
fn write_list_item(item: &adoctra_core::ast::ListItem, s: &mut String) {
    // Each item carries its own span for the editor.
    s.push('{');
    write_span_field(&item.span, s);
    s.push_str(",\"inlines\":");
    write_inlines(&item.inlines, s);
    if let Some(checked) = item.checkbox {
        s.push_str(&format!(",\"checkbox\":{checked}"));
    }
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

fn write_block(b: &Block, s: &mut String) {
    s.push('{');
    write_span_field(&b.span, s);
    s.push(',');
    match &b.kind {
        BlockKind::Heading { level, inlines } => {
            s.push_str(&format!("\"name\":\"heading\",\"level\":{level},"));
            // The editor gets each block's anchor (the section auto-id, an
            // explicit `[[id]]`, or null), so it can resolve/validate `<<id>>`
            // targets and build a TOC.
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            s.push_str(",\"inlines\":");
            write_inlines(inlines, s);
        }
        BlockKind::Paragraph { inlines } => {
            s.push_str("\"name\":\"paragraph\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            s.push_str(",\"inlines\":");
            write_inlines(inlines, s);
        }
        BlockKind::Listing {
            text,
            callouts,
            inlines,
        } => {
            s.push_str("\"name\":\"listing\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            s.push_str(",\"text\":");
            write_str(text, s);
            write_callouts(callouts, s);
            write_substituted(inlines, s);
        }
        BlockKind::Literal {
            text,
            callouts,
            inlines,
        } => {
            s.push_str("\"name\":\"literal\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            s.push_str(",\"text\":");
            write_str(text, s);
            write_callouts(callouts, s);
            write_substituted(inlines, s);
        }
        BlockKind::Stem { style, inlines } => {
            s.push_str("\"name\":\"stem\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&Some(style.as_str().into()), s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            s.push_str(",\"inlines\":");
            write_inlines(inlines, s);
        }
        BlockKind::Passthrough { text, inlines } => {
            s.push_str("\"name\":\"pass\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            s.push_str(",\"text\":");
            write_str(text, s);
            write_substituted(inlines, s);
        }
        BlockKind::ThematicBreak | BlockKind::PageBreak => {
            // The ASG `break` block with its variant; it has no content.
            s.push_str("\"name\":\"break\",\"variant\":");
            s.push_str(if matches!(b.kind, BlockKind::PageBreak) {
                "\"page\","
            } else {
                "\"thematic\","
            });
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
        }
        BlockKind::Toc => {
            // The `toc::[]` macro; the editor builds the outline from sections.
            s.push_str("\"name\":\"toc\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
        }
        BlockKind::Image {
            target,
            src,
            alt,
            svg,
        } => {
            // The resolved image macro; its line span covers target and list.
            s.push_str("\"name\":\"image\",\"target\":");
            write_str(target, s);
            s.push_str(",\"src\":");
            write_str(src, s);
            s.push_str(",\"alt\":");
            write_str(&adoctra_core::attributes::parts_text(alt), s);
            adoctra_core::json::write_svg(svg.as_ref(), s);
            s.push(',');
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
        }
        BlockKind::Table(table) => {
            s.push_str("\"name\":\"table\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            write_table(table, s);
        }
        BlockKind::Video {
            target,
            src,
            poster,
            embedded,
        } => {
            // The resolved video macro; its line span covers target and list.
            s.push_str("\"name\":\"video\",\"target\":");
            write_str(target, s);
            s.push_str(",\"src\":");
            write_str(src, s);
            s.push_str(",\"poster\":");
            match poster {
                Some(poster) => write_str(poster, s),
                None => s.push_str("null"),
            }
            s.push_str(&format!(",\"embedded\":{embedded},"));
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
        }
        BlockKind::Audio { target, src } => {
            s.push_str("\"name\":\"audio\",\"target\":");
            write_str(target, s);
            s.push_str(",\"src\":");
            write_str(src, s);
            s.push(',');
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
        }
        BlockKind::List { ordered, items } => {
            s.push_str(&format!("\"name\":\"list\",\"ordered\":{ordered},"));
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            s.push_str(",\"items\":[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_list_item(item, s);
            }
            s.push(']');
        }
        BlockKind::CalloutList { items } => {
            s.push_str("\"name\":\"colist\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            s.push_str(",\"items\":[");
            for (i, entry) in items.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_list_item(&entry.item, s);
                s.pop();
                s.push_str(",\"coids\":[");
                for (j, id) in entry.coids.iter().enumerate() {
                    if j > 0 {
                        s.push(',');
                    }
                    write_str(id, s);
                }
                s.push(']');
                write_icon(&entry.icons, &entry.src, s);
                s.push('}');
            }
            s.push(']');
        }
        BlockKind::DescriptionList { items } => {
            s.push_str("\"name\":\"dlist\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            s.push_str(",\"items\":[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push('{');
                write_span_field(&item.span, s);
                s.push_str(",\"terms\":[");
                for (j, term) in item.terms.iter().enumerate() {
                    if j > 0 {
                        s.push(',');
                    }
                    s.push('{');
                    write_span_field(&term.span, s);
                    s.push_str(",\"inlines\":");
                    write_inlines(&term.inlines, s);
                    s.push('}');
                }
                s.push_str("],\"description\":");
                match &item.description {
                    Some(description) => write_list_item(description, s),
                    None => s.push_str("null"),
                }
                s.push('}');
            }
            s.push(']');
        }
        BlockKind::Admonition {
            kind,
            label,
            blocks,
            icons,
            src,
        } => {
            s.push_str(&format!(
                "\"name\":\"admonition\",\"admonition-type\":\"{}\",\"label\":",
                kind.as_str()
            ));
            write_str(&adoctra_core::attributes::parts_text(label), s);
            s.push(',');
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            // Children carry their own spans; recurse so the editor sees the tree.
            write_simple(b.simple, s);
            write_icon(icons, src, s);
            s.push_str(",\"blocks\":[");
            for (j, child) in blocks.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push(']');
        }
        BlockKind::Example { blocks } => {
            s.push_str("\"name\":\"example\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            // Children carry their own spans; recurse so the editor sees the tree.
            write_simple(b.simple, s);
            s.push_str(",\"blocks\":[");
            for (j, child) in blocks.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push(']');
        }
        BlockKind::Sidebar { blocks } => {
            s.push_str("\"name\":\"sidebar\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            // Children carry their own spans; recurse so the editor sees the tree.
            write_simple(b.simple, s);
            s.push_str(",\"blocks\":[");
            for (j, child) in blocks.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push(']');
        }
        BlockKind::Section { level, blocks } => {
            // The span covers the title through the last nested block (useful
            // for folding); id/roles/attributes live on the title, `blocks[0]`.
            s.push_str(&format!(
                "\"name\":\"section\",\"level\":{level},\"blocks\":["
            ));
            for (j, child) in blocks.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push(']');
        }
        BlockKind::Preamble { blocks } => {
            // A book's preamble carries the header's `preface-title` source.
            s.push_str("\"name\":\"preamble\"");
            write_title(&b.title, s);
            s.push_str(",\"blocks\":[");
            for (j, child) in blocks.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push(']');
        }
        BlockKind::Open { blocks } => {
            s.push_str("\"name\":\"open\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            // Children carry their own spans; recurse so the editor sees the tree.
            write_simple(b.simple, s);
            s.push_str(",\"blocks\":[");
            for (j, child) in blocks.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push(']');
        }
        BlockKind::Quote {
            attribution,
            blocks,
        } => {
            s.push_str("\"name\":\"quote\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            write_attribution(attribution, s);
            // Children carry their own spans; recurse so the editor sees the tree.
            write_simple(b.simple, s);
            s.push_str(",\"blocks\":[");
            for (j, child) in blocks.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push(']');
        }
        BlockKind::Verse {
            attribution,
            inlines,
        } => {
            s.push_str("\"name\":\"verse\",");
            write_id(&b.id, s);
            write_roles(&b.roles, s);
            write_style(&b.attrs.style, s);
            write_attributes(&b.attrs.named, s);
            write_options(&b.attrs.options, s);
            write_title(&b.title, s);
            write_caption(b, s);
            write_attribution(attribution, s);
            s.push_str(",\"inlines\":");
            write_inlines(inlines, s);
        }
    }
    s.push('}');
}

/// Emit the leading-comma `,"attribution":{…}|null` field of a quote/verse block.
/// Each part is the inline array (carrying spans, like any inline the editor sees)
/// or `null`; an absent attribution is the JSON `null`.
fn write_attribution(attribution: &Option<Attribution>, s: &mut String) {
    s.push_str(",\"attribution\":");
    match attribution {
        Some(attr) => {
            s.push_str("{\"author\":");
            match &attr.author {
                Some(a) => write_inlines(a, s),
                None => s.push_str("null"),
            }
            s.push_str(",\"citetitle\":");
            match &attr.citetitle {
                Some(c) => write_inlines(c, s),
                None => s.push_str("null"),
            }
            // Credit text after the citetitle, only when a split left some.
            if !attr.trailing.is_empty() {
                s.push_str(",\"trailing\":");
                write_inlines(&attr.trailing, s);
            }
            s.push('}');
        }
        None => s.push_str("null"),
    }
}

/// Emit the `"id":<string|null>` field for a block's anchor.
fn write_id(id: &Option<String>, s: &mut String) {
    s.push_str("\"id\":");
    match id {
        Some(id) => write_str(id, s),
        None => s.push_str("null"),
    }
}

/// Emit the leading-comma `,"roles":[…]` field for a block's `[.role]` shorthand
/// roles (an empty array when there are none), so the editor sees them too.
fn write_roles(roles: &[String], s: &mut String) {
    s.push_str(",\"roles\":[");
    for (i, role) in roles.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        write_str(role, s);
    }
    s.push(']');
}

/// Emit the leading-comma `,"style":<string|null>` field — the block style (the
/// first positional attribute), or `null`.
fn write_style(style: &Option<String>, s: &mut String) {
    s.push_str(",\"style\":");
    match style {
        Some(v) => write_str(v, s),
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
        write_str(k, s);
        s.push(':');
        write_str(v, s);
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
        write_str(opt, s);
    }
    s.push(']');
}

/// Emit the leading-comma `,"title":<array|null>` field — the block's `.Title`
/// inlines (each carrying its span, like any inline the editor sees), or `null`
/// when the block has no title.
/// The `,"inlines":[…]` content of a verbatim or raw block converted with
/// custom substitutions (with source spans); absent with the defaults.
fn write_substituted(inlines: &Option<Vec<Inline>>, s: &mut String) {
    if let Some(inlines) = inlines {
        s.push_str(",\"inlines\":");
        write_inlines(inlines, s);
    }
}

fn write_title(title: &Option<Vec<Inline>>, s: &mut String) {
    s.push_str(",\"title\":");
    match title {
        Some(inlines) => write_inlines(inlines, s),
        None => s.push_str("null"),
    }
}

/// Emit the leading-comma `,"caption":<string|null>` field — the block's resolved
/// caption prefix (e.g. `"Example 1. "` on a numbered example block), or `null`
/// when the block has none.
fn write_caption(block: &Block, s: &mut String) {
    s.push_str(",\"caption\":");
    match &block.caption {
        Some(c) => write_str(c, s),
        None => s.push_str("null"),
    }
    // The caption's semantic parts when it is not plain text.
    if !block.caption_parts.is_empty() {
        s.push_str(",\"caption_parts\":");
        adoctra_core::json::write_parts_array(&block.caption_parts, s);
    }
}

/// Emit `,"simple":true` for a container with Ruby's simple content model (a
/// styled paragraph form holding its paragraph); omitted otherwise.
fn write_simple(simple: bool, s: &mut String) {
    if simple {
        s.push_str(",\"simple\":true");
    }
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
            Inline::Substituted { value, parts, .. } => {
                s.push_str("\"name\":\"substituted\",\"value\":");
                write_str(value, s);
                adoctra_core::json::write_attribute_parts(parts, s);
            }
            Inline::Raw { value, .. } => {
                s.push_str("\"name\":\"raw\",\"value\":");
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
            // The editor gets the resolved link `target` plus the `refid` it
            // can resolve or validate (and an interdocument `path`/`fragment`);
            // the display text is in `inlines`.
            Inline::CrossReference { xref, children, .. } => {
                s.push_str("\"name\":\"xref\",\"target\":");
                write_str(&xref.target, s);
                for (key, value) in [
                    ("refid", &xref.refid),
                    ("path", &xref.path),
                    ("fragment", &xref.fragment),
                    ("role", &xref.role),
                    ("window", &xref.window),
                    ("rel", &xref.rel),
                    ("xrefstyle", &xref.xrefstyle),
                ] {
                    if let Some(value) = value {
                        s.push_str(&format!(",\"{key}\":"));
                        write_str(value, s);
                    }
                }
                s.push_str(",\"inlines\":");
                write_inlines(children, s);
            }
            // An inline anchor: an empty `<a id>`. The editor gets the id (a xref
            // target / definition site) on the node's own span; the reftext is
            // resolve metadata, surfaced via a textless xref's resolved text.
            Inline::Anchor { id, .. } => {
                s.push_str("\"name\":\"anchor\",\"id\":");
                write_str(id, s);
            }
            // An inline image or icon with its lexical fields and alt parts.
            Inline::Image { image, .. } => {
                s.push_str("\"name\":\"image\",");
                adoctra_core::json::write_inline_image(image, s);
            }
            // A keyboard, button or menu macro with its label parts.
            Inline::Ui { ui, .. } => adoctra_core::json::write_ui(ui, "name", s),
            // An index term: a flow term's text parts, a concealed one's terms.
            Inline::IndexTerm { term, .. } => adoctra_core::json::write_index_term(term, "name", s),
            Inline::Stem { stem, .. } => adoctra_core::json::write_stem(stem, "name", s),
            // A footnote marker; definitions carry their content parts.
            Inline::Footnote { footnote, .. } => {
                s.push_str("\"name\":\"footnote\",");
                adoctra_core::json::write_footnote(footnote, s);
            }
            // The editor gets the structured reference (name + resolved value),
            // unlike the backends which only need the resolved text.
            Inline::AttributeRef {
                name, value, parts, ..
            } => {
                s.push_str("\"name\":\"attribute-ref\",\"attribute\":");
                write_str(name, s);
                s.push_str(",\"value\":");
                match value {
                    Some(v) => write_str(v, s),
                    None => s.push_str("null"),
                }
                adoctra_core::json::write_attribute_parts(parts, s);
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
    s.push_str("\"span\":");
    write_span(span, s);
}

fn write_span(span: &Span, s: &mut String) {
    s.push_str(&format!(
        "{{\"source_id\":{},\"start\":{},\"end\":{}",
        span.source_id.0, span.start, span.end
    ));
    if let Some(index) = span.provenance {
        s.push_str(&format!(",\"provenance\":{index}"));
    }
    s.push('}');
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

    #[wasm_bindgen]
    pub fn parse_json_with_source(src: &str, source_id: usize) -> String {
        super::parse_json_with_source(src, source_id)
    }

    #[wasm_bindgen]
    pub fn tokens_json_with_source(src: &str, source_id: usize) -> String {
        super::tokens_json_with_source(src, source_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_backend_and_doctype_attributes_resolve() {
        let json = parse_json("= T\n:doctype: book\n\nifdef::backend-html5-doctype-book[{backend}/{doctype}]\nifdef::doctype-article[article]\n");
        assert!(json.contains(r#""value":"html5""#), "{json}");
        assert!(json.contains(r#""value":"book""#), "{json}");
        assert!(!json.contains("article"), "{json}");
    }

    #[test]
    fn titles_without_ids_export_their_conversion_with_spans() {
        let json = parse_json(":sectids!:\n\n== Я {counter:n}\n\np {n}");
        assert!(
            json.contains(r#""inlines":[{"span":{"source_id":0,"start":15,"end":18},"name":"text","value":"Я "},{"span":{"source_id":0,"start":18,"end":29},"name":"attribute-ref","attribute":"counter:n","value":"1","parts":[{"kind":"raw","value":"1"}]}]}"#),
            "{json}"
        );
        assert!(!json.contains(r#""value":"""#), "{json}");
    }

    #[test]
    fn svg_images_export_their_embedding() {
        let json = parse_json("image::a.svg[opts=interactive]\n\nT image:gone.svg[opts=inline] x");
        assert!(
            json.contains(r#""alt":"a","svg":{"mode":"interactive","fallback":null}"#),
            "{json}"
        );
        assert!(
            json.contains(r#""svg":{"mode":"inline","content":null}"#),
            "{json}"
        );
    }

    #[test]
    fn nested_document_titles_export_their_heading_with_spans() {
        let json = parse_json("|===\na|\n= Я\n\nc\n|===");
        assert!(
            json.contains(r#""name":"heading","level":0,"id":null"#),
            "{json}"
        );
        assert!(
            json.contains(
                r#"{"span":{"source_id":0,"start":10,"end":12},"name":"text","value":"Я"}"#
            ),
            "{json}"
        );
    }

    #[test]
    fn captions_export_their_parts_when_not_plain() {
        let json =
            parse_json("[caption='*Я* ']\n.T\n====\nx\n====\n\n[caption=<b>]\n.U\n====\ny\n====");
        assert!(
            json.contains(r#""caption":"Я ","caption_parts":[{"kind":"open","format":"strong"},{"kind":"text","value":"Я"},{"kind":"close","format":"strong"},{"kind":"text","value":" "}]"#),
            "{json}"
        );
        assert!(
            json.contains(r#""caption":"<b>","caption_parts":[{"kind":"raw","value":"<b>"}]"#),
            "{json}"
        );
    }

    #[test]
    fn reference_labels_convert_their_titles_in_the_macros_stage() {
        let json = parse_json("<<s>> footnote:[one]\n\n[#s]\n== Я footnote:[two]");
        // The label borrows the title's spans; the title numbers first.
        assert!(
            json.contains(r##""name":"xref","target":"#s","refid":"s","fragment":"s","inlines":[{"span":{"source_id":0,"start":30,"end":33},"name":"text","value":"Я "},{"span":{"source_id":0,"start":33,"end":47},"name":"substituted","value":"[1]""##),
            "{json}"
        );
        assert!(
            json.contains(r#""name":"footnote","type":"definition","index":"2","id":null,"parts":[{"kind":"text","value":"one"}]"#),
            "{json}"
        );
    }

    #[test]
    fn section_ids_follow_the_id_policy() {
        let json =
            parse_json(":sectids!:\n\n== A\n\n:sectids:\n:idprefix:\n:idseparator: -\n\n== B C\n");
        assert!(
            json.contains(r#""name":"heading","level":1,"id":null"#),
            "{json}"
        );
        assert!(
            json.contains(r#""name":"heading","level":1,"id":"b-c""#),
            "{json}"
        );
    }

    #[test]
    fn bibliography_exports_parts_and_complete_source_anchor_token() {
        let source = "[bibliography]\n* [[[book,Label]]] Text";
        let json = parse_json_with_source(source, 7);
        assert!(json.contains(r#""style":"bibliography""#), "{json}");
        assert!(
            json.contains(
                r#""parts":[{"kind":"anchor","id":"book"},{"kind":"text","value":"[Label]"}]"#
            ),
            "{json}"
        );
        let tokens = tokens_json_with_source(source, 7);
        assert!(
            tokens.contains(r#"{"span":{"source_id":7,"start":17,"end":33},"kind":"anchor"}"#),
            "{tokens}"
        );
    }

    #[test]
    fn orphan_container_entries_leave_references_unresolved() {
        // Ruby drops an entry with no following block in its container.
        let json = parse_json("====\n:x: one\n====\n\n:y: two\n\n{x}{y}");
        assert!(json.contains(r#""attribute":"x","value":null"#), "{json}");
        assert!(json.contains(r#""attribute":"y","value":"two""#), "{json}");
    }

    #[test]
    fn host_source_id_reaches_nodes_diagnostics_and_tokens() {
        let src = "= Doc\n:who: first\n\n{who}\n\n:who: second\n\n{who} <<missing>>";
        let json = parse_json_with_source(src, 42);
        assert!(!json.contains(r#""source_id":0"#), "{json}");
        assert!(
            json.contains(r#""attribute":"who","value":"first""#),
            "{json}"
        );
        assert!(
            json.contains(r#""attribute":"who","value":"second""#),
            "{json}"
        );
        assert!(
            json.contains(r#""diagnostics":[{"span":{"source_id":42"#),
            "{json}"
        );
        let tokens = tokens_json_with_source(src, 42);
        assert!(tokens.contains(r#""source_id":42"#));
        assert!(!tokens.contains(r#""source_id":0"#));
    }

    #[test]
    fn missing_reference_in_title_exposes_fallback_and_diagnostic_span() {
        let json = parse_json(".See <<missing>>\n----\nx\n----");
        assert!(json.contains(r##"{"span":{"source_id":0,"start":5,"end":16},"name":"xref","target":"#missing","refid":"missing","fragment":"missing","inlines":[{"span":{"source_id":0,"start":5,"end":16},"name":"text","value":"[missing]"}]}"##), "{json}");
        assert!(
            json.contains(
                r#""diagnostics":[{"span":{"source_id":0,"start":5,"end":16},"severity":"warning""#
            ),
            "{json}"
        );
    }

    #[test]
    fn markdown_blockquotes_keep_physical_spans() {
        let json = parse_json("> a\n> -- Ann, W\n\n\"b\"\n-- B\n");
        // Marker-stripped content and the credit keep their source ranges.
        assert!(
            json.contains(r#"{"span":{"source_id":0,"start":0,"end":15},"name":"quote""#),
            "{json}"
        );
        assert!(
            json.contains(r#""attribution":{"author":[{"span":{"source_id":0,"start":9,"end":12},"name":"text","value":"Ann"}],"citetitle":[{"span":{"source_id":0,"start":14,"end":15},"name":"text","value":"W"}]}"#),
            "{json}"
        );
        assert!(
            json.contains(
                r#""blocks":[{"span":{"source_id":0,"start":2,"end":3},"name":"paragraph""#
            ),
            "{json}"
        );
        assert!(
            json.contains(r#""simple":true,"blocks":[{"span":{"source_id":0,"start":18,"end":19},"name":"paragraph""#),
            "{json}"
        );
    }

    #[test]
    fn credit_split_inside_formatting_exports_trailing_spans() {
        let json = parse_json("> a\n> -- *A, B* rest\n");
        assert!(
            json.contains(r#""citetitle":[{"span":{"source_id":0,"start":9,"end":15},"name":"strong","inlines":[{"span":{"source_id":0,"start":13,"end":14},"name":"text","value":"B"}]}],"trailing":[{"span":{"source_id":0,"start":15,"end":20},"name":"text","value":" rest"}]}"#),
            "{json}"
        );
    }

    #[test]
    fn styled_paragraph_forms_mark_simple_content() {
        let json = parse_json("[open]\nx\n\n--\ny\n--\n");
        assert!(
            json.contains(r#""name":"open","id":null,"roles":[],"style":"open","attributes":{},"options":[],"title":null,"caption":null,"simple":true,"blocks":[{"span":{"source_id":0,"start":7,"end":8},"name":"paragraph""#),
            "{json}"
        );
        assert_eq!(json.matches(r#""simple":true"#).count(), 1, "{json}");
    }

    #[test]
    fn collapsible_example_keeps_options_and_an_empty_caption() {
        let json = parse_json("[%collapsible]\n.T\n====\nx\n====\n");
        assert!(
            json.contains(r#""name":"example","id":null,"roles":[],"style":null,"attributes":{},"options":["collapsible"],"title":[{"span":{"source_id":0,"start":16,"end":17},"name":"text","value":"T"}],"caption":"","blocks":["#),
            "{json}"
        );
    }

    #[test]
    fn parse_json_has_document_and_diagnostics() {
        let json = parse_json("= Title\n\ntext");
        assert!(json.contains("\"document\""));
        assert!(json.contains("\"diagnostics\":[]"));
        assert!(json.contains("\"heading\""));
        assert!(json.contains("\"span\":{\"source_id\":0,\"start\":0,\"end\":7}"));
    }

    #[test]
    fn parse_json_reports_diagnostic() {
        let json = parse_json("----\nunterminated");
        assert!(json.contains("\"severity\":\"error\""));
    }

    #[test]
    fn section_heading_carries_auto_id() {
        // The editor gets the section anchor; the doctitle's id is null.
        let json = parse_json("= Doc\n\n== Hello World\n\nbody");
        assert!(json.contains("\"level\":0,\"id\":null"), "got: {json}");
        assert!(json.contains("\"id\":\"_hello_world\""), "got: {json}");
    }

    #[test]
    fn unresolved_xref_reports_warning() {
        let json = parse_json("See <<nope>>.");
        assert!(json.contains("\"severity\":\"warning\""), "got: {json}");
        assert!(
            json.contains("\"name\":\"text\",\"value\":\"[nope]\""),
            "got: {json}"
        );
    }

    #[test]
    fn tokens_json_is_array() {
        let json = tokens_json("= Title");
        assert!(json.starts_with('['));
        assert!(json.contains("\"kind\":\"heading\""));
    }

    #[test]
    fn parse_json_includes_tokens_from_the_same_multiline_parse() {
        let source = "Ж *первая\r\nвторая* строка";
        let json = parse_json_with_source(source, 7);
        let tokens = tokens_json_with_source(source, 7);
        assert!(json.contains(&format!(",\"tokens\":{tokens}")), "{json}");
        let start = source.find('*').unwrap();
        let end = source.rfind('*').unwrap() + 1;
        assert!(
            tokens.contains(&format!(
                r#"{{"span":{{"source_id":7,"start":{start},"end":{end}}},"kind":"strong"}}"#
            )),
            "{tokens}"
        );
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":7,"start":{start},"end":{end}}},"name":"strong""#
            )),
            "{json}"
        );
    }

    #[test]
    fn inline_nodes_carry_spans() {
        // The editor needs a span on every inline, not just on blocks.
        let json = parse_json("ab *cd*");
        // the strong runs from byte 3 to byte 7 ("*cd*")
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":3,\"end\":7},\"name\":\"strong\""),
            "got: {json}"
        );
    }

    #[test]
    fn new_quotes_are_structured_nodes_with_spans() {
        let json = parse_json("ab `cd`");
        // the monospace span runs from byte 3 to byte 7 ("`cd`")
        assert!(
            json.contains(
                "\"span\":{\"source_id\":0,\"start\":3,\"end\":7},\"name\":\"monospace\""
            ),
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
            json.contains(
                "\"span\":{\"source_id\":0,\"start\":4,\"end\":28},\"name\":\"substituted\""
            ),
            "got: {json}"
        );
        assert!(json.contains("\"target\":\"https://h\""), "got: {json}");
        assert!(json.contains("\"kind\":\"open-link\""), "got: {json}");
    }

    #[test]
    fn bare_autolink_is_structured_and_bare() {
        let json = parse_json("Bare https://example.org now");
        assert!(json.contains("\"name\":\"substituted\""), "got: {json}");
        assert!(json.contains("\"role\":\"bare\""), "got: {json}");
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
            json.contains("\"span\":{\"source_id\":0,\"start\":0,\"end\":11},\"name\":\"xref\""),
            "got: {json}"
        );
        assert!(
            json.contains("\"target\":\"#sec\",\"refid\":\"sec\",\"fragment\":\"sec\""),
            "got: {json}"
        );
    }

    #[test]
    fn xrefstyle_reference_exposes_its_style_and_generated_label_spans() {
        let src = "[[ex]]\n.T\n====\nx\n====\n\nxref:ex[xrefstyle=short]";
        let json = parse_json(src);
        let start = src.find("xref:").unwrap();
        let span = format!(r#"{{"source_id":0,"start":{start},"end":{}}}"#, src.len());
        assert!(
            json.contains(&format!(r##""name":"xref","target":"#ex","refid":"ex","fragment":"ex","xrefstyle":"short","inlines":[{{"span":{span},"name":"text","value":"Example"}},{{"span":{span},"name":"text","value":" 1"}}]"##)),
            "got: {json}"
        );
    }

    #[test]
    fn passthrough_xref_keeps_the_enclosing_span_and_parts() {
        let src = "[[s]]\n== T\n\npass:c,m[<<s>>]";
        let json = parse_json(src);
        let start = src.find("pass:").unwrap();
        assert!(
            json.contains(&format!(r##"{{"span":{{"source_id":0,"start":{start},"end":{}}},"name":"substituted","value":"T","parts":[{{"kind":"open-xref","target":"#s","refid":"s","fragment":"s"}},{{"kind":"text","value":"T"}},{{"kind":"close-xref"}}]"##, src.len())),
            "got: {json}"
        );
    }

    #[test]
    fn definition_xref_keeps_the_reference_span_and_fixed_label() {
        let src = ":r: pass:c,m[<<s>>]\n\n[[s]]\n== T\n\n:t: pass:c,m[<<s>>]\n\n{r}{t}";
        let json = parse_json(src);
        let start = src.rfind("{r}").unwrap();
        for (offset, label) in [(0, "[s]"), (3, "T")] {
            let span = format!(
                r#"{{"source_id":0,"start":{},"end":{}}}"#,
                start + offset,
                start + offset + 3
            );
            assert!(
                json.contains(&format!(r##"{{"span":{span},"name":"attribute-ref","##)),
                "got: {json}"
            );
            assert!(
                json.contains(&format!(r##"{{"kind":"open-xref","target":"#s","refid":"s","fragment":"s"}},{{"kind":"text","value":"{label}"}},{{"kind":"close-xref"}}"##)),
                "got: {json}"
            );
        }
    }

    #[test]
    fn numbered_section_heading_keeps_its_span_and_caption() {
        let src = ":sectnums:\n\n== A\n\n=== B";
        let json = parse_json(src);
        let start = src.find("=== B").unwrap();
        assert!(
            json.contains(&format!(r#"{{"span":{{"source_id":0,"start":{start},"end":{}}},"name":"heading","level":2,"#, src.len())),
            "got: {json}"
        );
        assert!(
            json.contains(r#""caption":"1.1. ","inlines""#),
            "got: {json}"
        );
    }

    #[test]
    fn appendix_heading_keeps_its_line_span_and_caption() {
        let src = "= Doc\n\n[appendix]\n= A";
        let json = parse_json(src);
        let start = src.find("= A").unwrap();
        assert!(
            json.contains(&format!(r#"{{"span":{{"source_id":0,"start":{start},"end":{}}},"name":"heading","level":1,"#, src.len())),
            "got: {json}"
        );
        assert!(
            json.contains(r#""caption":"Appendix A: ","inlines""#),
            "got: {json}"
        );
    }

    #[test]
    fn interdocument_xref_exposes_path_fragment_constraints_and_span() {
        let src = "A xref:other.adoc#f[T,window=_blank]";
        let json = parse_json(src);
        assert!(
            json.contains(r##"{"span":{"source_id":0,"start":2,"end":36},"name":"xref","target":"other.html#f","refid":"other#f","path":"other.html","fragment":"f","window":"_blank","rel":"noopener","inlines":"##),
            "got: {json}"
        );
        // No unresolved-reference warning for another document.
        assert!(json.contains(r#""diagnostics":[]"#), "got: {json}");
        let json = parse_json("<<#>>");
        assert!(
            json.contains(r##""name":"xref","target":"#","refid":"","fragment":"","inlines":[{"span":{"source_id":0,"start":0,"end":5},"name":"text","value":"[^top]"}]"##),
            "got: {json}"
        );
    }

    #[test]
    fn textless_xref_carries_target_and_bracketed_text() {
        let json = parse_json("<<sec>>");
        assert!(json.contains("\"name\":\"xref\""), "got: {json}");
        assert!(
            json.contains("\"target\":\"#sec\",\"refid\":\"sec\",\"fragment\":\"sec\""),
            "got: {json}"
        );
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
            json.contains("\"span\":{\"source_id\":0,\"start\":0,\"end\":5}"),
            "got: {json}"
        );
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":6,\"end\":11}"),
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
            json.contains("\"span\":{\"source_id\":0,\"start\":3,\"end\":4},\"name\":\"text\""),
            "got: {json}"
        );
    }

    #[test]
    fn list_item_inlines_carry_spans() {
        // The item content "one" sits at bytes 2..5 of "* one".
        let json = parse_json("* one");
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":2,\"end\":5},\"name\":\"text\""),
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
            json.contains(
                "\"blocks\":[{\"span\":{\"source_id\":0,\"start\":6,\"end\":10},\"name\":\"list\""
            ),
            "got: {json}"
        );
        // The nested item's principal text sits at bytes 9..10.
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":9,\"end\":10},\"name\":\"text\""),
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
            json.contains("\"span\":{\"source_id\":0,\"start\":0,\"end\":11}"),
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
            json.contains("\"span\":{\"source_id\":0,\"start\":0,\"end\":8}"),
            "got: {json}"
        );
    }

    #[test]
    fn anchored_block_exposes_its_id() {
        let json = parse_json("[[pa]]\nHi.");
        assert!(
            json.contains("\"name\":\"paragraph\",\"id\":\"pa\""),
            "got: {json}"
        );
    }

    #[test]
    fn plain_paragraph_exposes_null_id() {
        let json = parse_json("Hi.");
        assert!(
            json.contains("\"name\":\"paragraph\",\"id\":null"),
            "got: {json}"
        );
    }

    #[test]
    fn block_anchor_line_is_tokenized_as_anchor() {
        // `tokens_json` should classify a whole `[[id]]` line as one anchor token.
        let json = tokens_json("[[pa]]\nHi.");
        assert!(json.contains("\"kind\":\"anchor\""), "got: {json}");
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":0,\"end\":6},\"kind\":\"anchor\""),
            "got: {json}"
        );
    }

    #[test]
    fn inline_anchor_is_a_structured_node_with_span_and_id() {
        // "[[point]]" spans bytes 2..11 here; the editor gets the anchor id.
        let json = parse_json("x [[point]]y");
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":2,\"end\":11},\"name\":\"anchor\""),
            "got: {json}"
        );
        assert!(json.contains("\"id\":\"point\""), "got: {json}");
    }

    #[test]
    fn xref_resolves_to_an_inline_anchor_reftext() {
        // A textless `<<point>>` to an inline `[[point,…]]` gets the reftext text.
        let json = parse_json("Go <<point>>.\n\n[[point,the Part]]here.");
        assert!(
            json.contains("\"name\":\"text\",\"value\":\"the Part\""),
            "got: {json}"
        );
    }

    #[test]
    fn inline_anchor_is_tokenized_as_anchor() {
        // A mid-line `[[id]]` is one anchor token over the whole construct.
        let json = tokens_json("x [[point]]y");
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":2,\"end\":11},\"kind\":\"anchor\""),
            "got: {json}"
        );
    }

    #[test]
    fn shorthand_id_and_roles_are_exposed() {
        // The editor receives both the id and the `[.role]` roles.
        let json = parse_json("[#intro.lead]\nHi.");
        assert!(
            json.contains("\"id\":\"intro\",\"roles\":[\"lead\"]"),
            "got: {json}"
        );
    }

    #[test]
    fn plain_block_exposes_empty_roles() {
        let json = parse_json("Hi.");
        assert!(json.contains("\"id\":null,\"roles\":[]"), "got: {json}");
    }

    #[test]
    fn source_style_and_options_are_exposed() {
        // The editor sees the block style, named attributes (language folded from
        // `[source,LANG]`) and `%options`.
        let json = parse_json("[source,ruby]\n----\nx\n----");
        assert!(
            json.contains(
                "\"style\":\"source\",\"attributes\":{\"language\":\"ruby\"},\"options\":[]"
            ),
            "got: {json}"
        );
        let json = parse_json("[%hardbreaks]\nHi.");
        assert!(json.contains("\"options\":[\"hardbreaks\"]"), "got: {json}");
    }

    #[test]
    fn plain_block_exposes_empty_attrs() {
        let json = parse_json("Hi.");
        assert!(
            json.contains("\"style\":null,\"attributes\":{},\"options\":[]"),
            "got: {json}"
        );
    }

    #[test]
    fn literal_block_is_exposed_to_the_editor() {
        // Both the `....` delimiter and the `[literal]` style surface a literal
        // node with verbatim text.
        let json = parse_json("....\nverbatim *x*\n....");
        assert!(json.contains("\"name\":\"literal\""), "got: {json}");
        assert!(json.contains("\"text\":\"verbatim *x*\""), "got: {json}");

        let json = parse_json("[literal]\na *b* c");
        assert!(json.contains("\"name\":\"literal\""), "got: {json}");
        assert!(json.contains("\"style\":\"literal\""), "got: {json}");
        assert!(json.contains("\"text\":\"a *b* c\""), "got: {json}");
    }

    #[test]
    fn passthrough_block_is_exposed_to_the_editor() {
        // The `++++` delimiter and the `[pass]` style both surface a pass node with
        // verbatim text (no inline subs — `{x}` / `*b*` stay literal).
        let json = parse_json("++++\n<div>{x} *b*</div>\n++++");
        assert!(json.contains("\"name\":\"pass\""), "got: {json}");
        assert!(
            json.contains("\"text\":\"<div>{x} *b*</div>\""),
            "got: {json}"
        );

        let json = parse_json("[pass]\n<b>raw</b> *c*");
        assert!(json.contains("\"name\":\"pass\""), "got: {json}");
        assert!(json.contains("\"style\":\"pass\""), "got: {json}");
        assert!(json.contains("\"text\":\"<b>raw</b> *c*\""), "got: {json}");
    }

    #[test]
    fn admonition_is_exposed_to_the_editor() {
        // Both forms surface an admonition node with its type; the body is a nested
        // paragraph that keeps its inline markup (unlike a verbatim literal).
        let json = parse_json("[NOTE]\nThis is a *note*.");
        assert!(json.contains("\"name\":\"admonition\""), "got: {json}");
        assert!(json.contains("\"admonition-type\":\"note\""), "got: {json}");
        assert!(json.contains("\"blocks\":["), "got: {json}");
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");

        // Label form: same node, and the body's inline still carries a global span
        // (the label prefix is stripped, so the strong starts at byte 9).
        let json = parse_json("NOTE: be *bold*");
        assert!(json.contains("\"admonition-type\":\"note\""), "got: {json}");
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":9,\"end\":15},\"name\":\"strong\""),
            "got: {json}"
        );
    }

    #[test]
    fn delimited_admonition_exposes_nested_blocks() {
        // `[NOTE]` on a `====` block surfaces an admonition node whose body is the
        // nested child blocks (parsed, not verbatim — the `*there*` is a strong).
        let json = parse_json("[NOTE]\n====\nHi *there*.\n====");
        assert!(json.contains("\"name\":\"admonition\""), "got: {json}");
        assert!(json.contains("\"admonition-type\":\"note\""), "got: {json}");
        assert!(json.contains("\"blocks\":["), "got: {json}");
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");
    }

    #[test]
    fn example_block_serializes_with_nested_blocks_and_spans() {
        let json = parse_json("====\nHi.\n====");
        assert!(json.contains("\"name\":\"example\""), "got: {json}");
        assert!(json.contains("\"blocks\":["), "got: {json}");
        // The example block carries its own span (opening through closing fence).
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":0,\"end\":13},\"name\":\"example\""),
            "got: {json}"
        );
        // The nested paragraph carries its own span too.
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
    }

    #[test]
    fn verbatim_paragraphs_and_adjusted_subs_export_physical_spans() {
        let source = ":n: v\n\n[source,subs=+attributes,indent=2]\n    {n}\n\n[listing]\n--\nx\n--";
        let json = parse_json(source);
        let reference = source.find("{n}").unwrap();
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":0,"start":{},"end":{}}},"name":"listing","id":null,"roles":[],"style":"source""#,
                reference - 4,
                reference + 3
            )),
            "{json}"
        );
        // The converted text spans the reference; the new indent is zero-width.
        assert!(
            json.contains(r#""name":"substituted","value":"  v","#),
            "{json}"
        );
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":0,"start":{reference},"end":{}}}"#,
                reference + 3
            )),
            "{json}"
        );
        assert!(json.contains(r#""style":"listing","#), "{json}");
        let tokens = tokens_json(source);
        let open = source.find("--").unwrap();
        for (start, end, kind) in [
            (open, open + 2, "open-delimiter"),
            (open + 3, open + 4, "listing-text"),
        ] {
            assert!(
                tokens.contains(&format!(
                    r#"{{"span":{{"source_id":0,"start":{start},"end":{end}}},"kind":"{kind}"}}"#
                )),
                "{tokens}"
            );
        }
    }

    #[test]
    fn breaks_and_fenced_code_export_spans_and_tokens() {
        let source = "'''\n\n<<<\n\n```sh\nls\n```";
        let json = parse_json(source);
        assert!(
            json.contains(
                r#""span":{"source_id":0,"start":0,"end":3},"name":"break","variant":"thematic""#
            ),
            "{json}"
        );
        assert!(
            json.contains(
                r#""span":{"source_id":0,"start":5,"end":8},"name":"break","variant":"page""#
            ),
            "{json}"
        );
        assert!(
            json.contains(r#""style":"source","attributes":{"language":"sh"}"#),
            "{json}"
        );
        let tokens = tokens_json(source);
        for (start, end, kind) in [
            (0, 3, "thematic-break"),
            (5, 8, "page-break"),
            (10, 15, "listing-delimiter"),
            (16, 18, "listing-text"),
            (19, 22, "listing-delimiter"),
        ] {
            assert!(
                tokens.contains(&format!(
                    r#"{{"span":{{"source_id":0,"start":{start},"end":{end}}},"kind":"{kind}"}}"#
                )),
                "{tokens}"
            );
        }
    }

    #[test]
    fn longer_fences_export_nested_spans_and_fence_tokens() {
        let source = "======\n====\nx\n====\n======";
        let json = parse_json(source);
        assert!(
            json.contains(r#""span":{"source_id":0,"start":0,"end":25},"name":"example""#),
            "{json}"
        );
        assert!(
            json.contains(r#""span":{"source_id":0,"start":7,"end":18},"name":"example""#),
            "{json}"
        );
        let tokens = tokens_json(source);
        for (start, end) in [(0, 6), (7, 11), (14, 18), (19, 25)] {
            assert!(
                tokens.contains(&format!(
                    r#"{{"span":{{"source_id":0,"start":{start},"end":{end}}},"kind":"example-delimiter"}}"#
                )),
                "{tokens}"
            );
        }
    }

    #[test]
    fn block_title_serializes_as_inlines_with_spans() {
        // The editor gets the title as parsed inlines, each carrying its span.
        let json = parse_json(".A *bold* caption\nBody.");
        assert!(
            json.contains(
                "\"title\":[{\"span\":{\"source_id\":0,\"start\":1,\"end\":3},\"name\":\"text\""
            ),
            "got: {json}"
        );
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");
    }

    #[test]
    fn untitled_block_serializes_null_title() {
        let json = parse_json("Body.");
        assert!(json.contains("\"title\":null"), "got: {json}");
    }

    #[test]
    fn titled_example_serializes_caption() {
        // The editor gets the resolved counter prefix as a string caption.
        let json = parse_json(".Demo\n====\nhi\n====");
        assert!(json.contains("\"caption\":\"Example 1. \""), "got: {json}");
    }

    #[test]
    fn titled_listing_serializes_listing_caption() {
        // Listings number only when the document sets `listing-caption`.
        let json =
            parse_json(".A\n----\na\n----\n\n:listing-caption: Listing\n\n.B\n----\nb\n----");
        assert_eq!(json.matches(r#""caption":null"#).count(), 1, "got: {json}");
        assert!(json.contains(r#""caption":"Listing 1. ""#), "got: {json}");
    }

    #[test]
    fn untitled_block_serializes_null_caption() {
        let json = parse_json("Body.");
        assert!(json.contains("\"caption\":null"), "got: {json}");
    }

    #[test]
    fn titled_listing_serializes_null_caption() {
        // Only example blocks are numbered; a titled listing has no caption.
        let json = parse_json(".Code\n----\nputs 1\n----");
        assert!(json.contains("\"name\":\"listing\""), "got: {json}");
        assert!(json.contains("\"caption\":null"), "got: {json}");
    }

    // --- sidebar blocks ---

    #[test]
    fn sidebar_block_serializes_with_nested_blocks_and_spans() {
        let json = parse_json("****\nHi.\n****");
        assert!(json.contains("\"name\":\"sidebar\""), "got: {json}");
        assert!(json.contains("\"blocks\":["), "got: {json}");
        // The sidebar block carries its own span (opening through closing fence).
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":0,\"end\":13},\"name\":\"sidebar\""),
            "got: {json}"
        );
        // The nested paragraph carries its own span too.
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
    }

    // --- open blocks ---

    #[test]
    fn open_block_serializes_with_nested_blocks_and_spans() {
        let json = parse_json("--\nHi.\n--");
        assert!(json.contains("\"name\":\"open\""), "got: {json}");
        assert!(json.contains("\"blocks\":["), "got: {json}");
        // The open block carries its own span (opening through closing fence).
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":0,\"end\":9},\"name\":\"open\""),
            "got: {json}"
        );
        // The nested paragraph carries its own span too.
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
    }

    // --- quote & verse blocks ---

    #[test]
    fn quote_block_serializes_with_nested_blocks_and_spans() {
        let json = parse_json("____\nHi.\n____");
        assert!(json.contains("\"name\":\"quote\""), "got: {json}");
        assert!(json.contains("\"blocks\":["), "got: {json}");
        // The quote block carries its own span (opening through closing fence).
        assert!(
            json.contains("\"span\":{\"source_id\":0,\"start\":0,\"end\":13},\"name\":\"quote\""),
            "got: {json}"
        );
        assert!(json.contains("\"attribution\":null"), "got: {json}");
    }

    #[test]
    fn quote_serializes_attribution_for_the_editor() {
        let json = parse_json("[quote, Carl Sandburg, Fog]\n____\nx\n____");
        // Attribution parts are inline arrays, each inline carrying its own span.
        assert!(
            json.contains("\"attribution\":{\"author\":[{\"span\":"),
            "got: {json}"
        );
        assert!(json.contains("\"value\":\"Carl Sandburg\""), "got: {json}");
        assert!(json.contains("\"value\":\"Fog\""), "got: {json}");
    }

    #[test]
    fn attribution_spans_point_to_their_fields() {
        // Each attribution part keeps the physical range of its own field.
        let src = "[quote, Ann, \"Fög\"]\n____\nx\n____";
        let json = parse_json(src);
        for part in ["Ann", "Fög"] {
            let at = src.find(part).unwrap();
            assert!(
                json.contains(&format!(
                    "\"span\":{{\"source_id\":0,\"start\":{at},\"end\":{}}},\"name\":\"raw\",\"value\":\"{part}\"",
                    at + part.len()
                )),
                "got: {json}"
            );
        }
        // A named field occupies the author slot.
        let json = parse_json("[quote, role=x, Ann]\n____\nx\n____");
        assert!(json.contains("\"author\":null"), "got: {json}");
    }

    #[test]
    fn attribution_references_keep_their_source_spans() {
        let src = ":a: X\n\n[quote, Ж{a}, '{counter:n}']\nx";
        let json = parse_json(src);
        let at = src.find("{a}").unwrap();
        assert!(
            json.contains(&format!(
                "\"span\":{{\"source_id\":0,\"start\":{at},\"end\":{}}},\"name\":\"attribute-ref\"",
                at + 3
            )),
            "got: {json}"
        );
        // A single-quoted field changed by the early a-stage is rescanned;
        // its nodes take the field's range.
        let field = src.find("{counter:n}").unwrap();
        assert!(
            json.contains(&format!(
                "\"citetitle\":[{{\"span\":{{\"source_id\":0,\"start\":{field},\"end\":{}}},\"name\":\"text\",\"value\":\"1\"}}]",
                field + "{counter:n}".len()
            )),
            "got: {json}"
        );
    }

    #[test]
    fn list_title_spans_point_to_the_field() {
        let src = ":x: X\n\n[title=\"Ж {x}\"]\npara";
        let json = parse_json(src);
        let start = src.find("Ж").unwrap();
        let end = src.find("\"]").unwrap();
        assert!(
            json.contains(&format!(
                "\"title\":[{{\"span\":{{\"source_id\":0,\"start\":{start},\"end\":{end}}}"
            )),
            "got: {json}"
        );
    }

    #[test]
    fn xref_labels_from_cataloged_anchors_keep_reftext_spans() {
        let src = "p\n\nЖ [[a,*R*]]\n\n<<a>>";
        let json = parse_json(src);
        let at = src.find("*R*").unwrap();
        assert!(
            json.contains(&format!(
                "\"span\":{{\"source_id\":0,\"start\":{at},\"end\":{}}},\"name\":\"strong\"",
                at + 3
            )),
            "got: {json}"
        );
    }

    #[test]
    fn admonition_label_and_section_anchor_ids_reach_the_editor() {
        let json = parse_json("[WARNING,caption=Careful]\nx\n\n== S [[s,R]]");
        assert!(
            json.contains("\"admonition-type\":\"warning\",\"label\":\"Careful\""),
            "got: {json}"
        );
        assert!(
            json.contains("\"name\":\"heading\",\"level\":1,\"id\":\"s\""),
            "got: {json}"
        );
    }

    #[test]
    fn verse_block_serializes_inlines_preserving_newline() {
        let json = parse_json("[verse]\n____\nfirst\nsecond\n____");
        assert!(json.contains("\"name\":\"verse\""), "got: {json}");
        assert!(json.contains("\"inlines\":["), "got: {json}");
        // Verse keeps its line break — a JSON-escaped newline in the inline text.
        assert!(json.contains(r"first\nsecond"), "got: {json}");
    }

    // --- verbatim masquerades on a `--` open block ---

    #[test]
    fn source_style_on_open_is_exposed_as_listing() {
        let json = parse_json("[source,ruby]\n--\nputs *x*\n--");
        assert!(json.contains("\"name\":\"listing\""), "got: {json}");
        assert!(
            json.contains("\"style\":\"source\",\"attributes\":{\"language\":\"ruby\"}"),
            "got: {json}"
        );
        assert!(json.contains("\"text\":\"puts *x*\""), "got: {json}");
    }

    #[test]
    fn literal_style_on_open_is_exposed_as_literal() {
        let json = parse_json("[literal]\n--\nverbatim *x*\n--");
        assert!(json.contains("\"name\":\"literal\""), "got: {json}");
        assert!(json.contains("\"text\":\"verbatim *x*\""), "got: {json}");
    }

    #[test]
    fn verse_style_on_open_is_exposed_preserving_newline() {
        let json = parse_json("[verse, Carl Sandburg, Fog]\n--\nfirst\nsecond\n--");
        assert!(json.contains("\"name\":\"verse\""), "got: {json}");
        assert!(json.contains("\"value\":\"Carl Sandburg\""), "got: {json}");
        // Verse keeps its line break — a JSON-escaped newline in the inline text.
        assert!(json.contains(r"first\nsecond"), "got: {json}");
    }
}

#[cfg(test)]
mod header_tests {
    #[test]
    fn sanitized_section_ids_preserve_raw_values_and_source_coordinates() {
        let source = ":v: pass:[<b>Ж</b> &copy;]\r\n\r\n== {v}\r\n\r\n<<_ж>>";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#""id":"_ж""#), "{json}");
        assert!(
            json.contains(r##""target":"#_ж","refid":"_ж","fragment":"_ж""##),
            "{json}"
        );
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        let start = source.find("{v}").unwrap();
        assert!(json.contains(&format!(r#""span":{{"source_id":7,"start":{start},"end":{}}},"name":"attribute-ref","attribute":"v","value":"<b>Ж</b> &copy;""#, start + 3)), "{json}");
        let tokens = super::tokens_json_with_source(source, 7);
        let start = source.find("== {v}").unwrap();
        assert!(
            tokens.contains(&format!(
                r#"{{"span":{{"source_id":7,"start":{start},"end":{}}},"kind":"heading"}}"#,
                start + 6
            )),
            "{tokens}"
        );
    }

    #[test]
    fn xref_labels_keep_title_spans_and_per_reference_reftext() {
        let src = "See <<t>>.\r\n\r\n[[t]]\r\n.Ж *T*\r\n----\r\nx\r\n----\r\n\r\n[[b,[.r\\{x}]#R#]]\r\npara\r\n\r\n:x: 1\r\n\r\n<<b>>\r\n\r\n:x: 2\r\n\r\n<<b>>";
        let json = super::parse_json_with_source(src, 4);
        // The forward label borrows the title's physical spans.
        let at = src.find("*T*").unwrap();
        assert_eq!(
            json.matches(&format!(
                "\"span\":{{\"source_id\":4,\"start\":{at},\"end\":{}}},\"name\":\"strong\"",
                at + 3
            ))
            .count(),
            2,
            "{json}"
        );
        assert!(json.contains(r#""role":"r1""#), "{json}");
        assert!(json.contains(r#""role":"r2""#), "{json}");
        assert!(!json.contains("\"diagnostics\":[{"), "{json}");
        assert_eq!(json, super::parse_json_with_source(src, 4));
    }

    #[test]
    fn block_reftext_exports_early_values_and_physical_expression_tokens() {
        let source = "[[here,Ж {counter:n:Я}]]\r\nBody {counter:n}\r\n\r\n<<here>>";
        let json = super::parse_json_with_source(source, 7);
        assert!(
            json.contains(r#""attribute":"counter:n:Я","value":"Я""#),
            "{json}"
        );
        let ts = super::tokens_json_with_source(source, 7);
        let start = source.find("{counter:n:Я}").unwrap();
        assert!(
            ts.contains(&format!(
                r#"{{"span":{{"source_id":7,"start":{start},"end":{}}},"kind":"attribute-ref"}}"#,
                start + "{counter:n:Я}".len()
            )),
            "{ts}"
        );
        assert_eq!(json, super::parse_json_with_source(source, 7));
        assert_eq!(ts, super::tokens_json_with_source(source, 7));
    }

    #[test]
    fn stateful_block_titles_export_physical_spans_and_stable_tokens() {
        let source = ".Ж {counter:example-number}\r\n====\r\nBody {counter:example-number}\r\n====";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#""caption":"Example 1. ""#), "{json}");
        assert!(
            json.contains(r#""attribute":"counter:example-number","value":"2""#),
            "{json}"
        );
        assert!(json.contains(r#""kind":"raw","value":"3""#), "{json}");
        let ts = super::tokens_json_with_source(source, 7);
        for (start, _) in source.match_indices("{counter:example-number}") {
            assert!(ts.contains(&format!(r#"{{"span":{{"source_id":7,"start":{start},"end":{}}},"kind":"attribute-ref"}}"#, start + "{counter:example-number}".len())), "{ts}");
        }
        assert_eq!(json, super::parse_json_with_source(source, 7));
        assert_eq!(ts, super::tokens_json_with_source(source, 7));
    }

    #[test]
    fn stateful_headings_export_source_references_and_do_not_repeat_effects() {
        let source = "= Ж {counter:n}\r\n\r\n== Б {counter:n}\r\n\r\n{counter:n}";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#""kind":"raw","value":"3""#), "{json}");
        assert!(
            json.contains(r#""attribute":"counter:n","value":"2""#),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 7);
        for (start, _) in source.match_indices("{counter:n}") {
            assert!(tokens.contains(&format!(r#"{{"span":{{"source_id":7,"start":{start},"end":{}}},"kind":"attribute-ref"}}"#, start + "{counter:n}".len())), "{tokens}");
        }
        assert_eq!(json, super::parse_json_with_source(source, 7));
        assert_eq!(tokens, super::tokens_json_with_source(source, 7));
    }

    #[test]
    fn stateful_body_exports_source_tokens_without_reexecuting_effects() {
        let source = "Ж {counter:n}\r\n// skipped\r\nБ {counter:n}\r\n\r\n{n}";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#""kind":"raw","value":"2""#), "{json}");
        let tokens = super::tokens_json_with_source(source, 7);
        for (start, _) in source.match_indices("{counter:n}") {
            assert!(tokens.contains(&format!(r#"{{"span":{{"source_id":7,"start":{start},"end":{}}},"kind":"attribute-ref"}}"#, start + "{counter:n}".len())), "{tokens}");
        }
        assert_eq!(json, super::parse_json_with_source(source, 7));
        assert_eq!(tokens, super::tokens_json_with_source(source, 7));
    }

    #[test]
    fn stateful_definitions_export_parts_physical_diagnostics_and_entry_tokens() {
        let counters = super::parse_json(":v: {counter:N:8}{counter:n}\n\n{v}");
        assert!(
            counters.contains(r#""N":[{"kind":"raw","value":"8"}]"#),
            "{counters}"
        );
        assert!(
            counters.contains(r#""n":[{"kind":"raw","value":"1"}]"#),
            "{counters}"
        );
        let source = ":v: pass:n[link:pass:a[{counter:n}][]]\r\n:w: {set:attribute-missing:warn}{missing}\r\n\r\n{v}";
        let json = super::parse_json_with_source(source, 7);
        assert!(
            json.contains(r#""kind":"open-link","target":"1","role":"bare""#),
            "{json}"
        );
        assert!(json.contains(r#""kind":"raw","value":"2""#), "{json}");
        let start = source.find(":w:").unwrap();
        let end = source[start..].find('\r').unwrap() + start;
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":7,"start":{start},"end":{end}}},"severity":"warning""#
            )),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 7);
        assert_eq!(tokens.matches(r#""kind":"attribute-entry""#).count(), 2);
        assert_eq!(json, super::parse_json_with_source(source, 7));
    }

    #[test]
    fn repeated_link_normal_exports_parts_and_enclosing_physical_span() {
        let source =
            ":v: pass:[+*Ж*+]\r\n\r\nlink:https://e['{v}',role=x] link:https://e['end +',role=x]";
        let json = super::parse_json_with_source(source, 8);
        assert!(json.contains(r#"{"kind":"text","value":"*Ж*"}"#), "{json}");
        assert!(json.contains(r#"{"kind":"line-break"}"#), "{json}");
        let start = source.rfind("link:").unwrap();
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":8,"start":{start},"end":{}}},"name":"substituted""#,
                source.len()
            )),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 8);
        assert!(tokens.contains(r#""kind":"link""#), "{tokens}");
        assert!(!tokens.contains(r#""kind":"pass-text""#), "{tokens}");
        assert!(!tokens.contains(r#""kind":"line-break""#), "{tokens}");
    }

    #[test]
    fn metadata_substitutions_export_each_warning_with_physical_origins() {
        let source = ":attribute-missing: warn\r\n\r\nЖ [.{missing}]*Text* link:https://e['\\{other}',role=x]";
        let json = super::parse_json_with_source(source, 8);
        assert_eq!(json.matches(r#""severity":"warning""#).count(), 3, "{json}");
        for markup in [
            "{missing}",
            "[.{missing}]*",
            "link:https://e['\\{other}',role=x]",
        ] {
            let start = source.find(markup).unwrap();
            let end = start + markup.len();
            assert!(
                json.contains(&format!(
                    r#""span":{{"source_id":8,"start":{start},"end":{end}}},"severity":"warning""#
                )),
                "{json}"
            );
        }
        let tokens = super::tokens_json_with_source(source, 8);
        assert!(tokens.contains(r#""kind":"attribute-ref""#), "{tokens}");
        assert!(tokens.contains(r#""kind":"link""#), "{tokens}");
    }

    #[test]
    fn ordinary_post_replacements_export_parts_spans_and_break_tokens() {
        let source = ":v: one + \\\n two\n\n{v} +\nlast";
        let json = super::parse_json_with_source(source, 9);
        let (document, _) = json.split_once(",\"tokens\":").unwrap();
        assert_eq!(
            document.matches(r#""kind":"line-break""#).count(),
            2,
            "{json}"
        );
        assert!(
            json.contains(r#""attribute":"v","value":"one two""#),
            "{json}"
        );
        let start = source.rfind(" +").unwrap();
        let tokens = super::tokens_json_with_source(source, 9);
        assert!(
            tokens.contains(&format!(
                r#"{{"span":{{"source_id":9,"start":{start},"end":{}}},"kind":"line-break"}}"#,
                start + 2
            )),
            "{tokens}"
        );
    }

    #[test]
    fn shared_attribute_stage_exports_definition_parts_and_diagnostic_origins() {
        let source =
            "\u{feff}:attribute-missing: warn\r\n:v: {cpp} {missing}\r\n\r\n{v} pass:a[{other}]";
        let json = super::parse_json_with_source(source, 9);
        assert!(
            json.contains(r#""kind":"raw","value":"C&#43;&#43;""#),
            "{json}"
        );
        assert!(
            json.contains(r#""attribute":"v","value":"C&#43;&#43; {missing}""#),
            "{json}"
        );
        for (markup, missing, kind) in [
            (":v: {cpp} {missing}", "missing", "attribute-entry"),
            ("pass:a[{other}]", "other", "pass-text"),
        ] {
            let start = source.find(markup).unwrap();
            let end = start + markup.len();
            assert!(json.contains(&format!(r#""span":{{"source_id":9,"start":{start},"end":{end}}},"severity":"warning","message":"skipping reference to missing attribute: {missing}""#)), "{json}");
            let tokens = super::tokens_json_with_source(source, 9);
            assert!(
                tokens.contains(&format!(
                    r#""span":{{"source_id":9,"start":{start},"end":{end}}},"kind":"{kind}""#
                )),
                "{tokens}"
            );
        }
        assert_eq!(json.matches(r#""severity":"warning""#).count(), 2, "{json}");
    }

    #[test]
    fn ordinary_attribute_policies_export_values_diagnostics_and_source_tokens() {
        let source = "\u{feff}:attribute-missing: warn\r\n\r\nЖ {cpp} {missing} \\{missing}";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#""attribute":"cpp","value":"C&#43;&#43;","parts":[{"kind":"raw","value":"C&#43;&#43;"}]"#), "{json}");
        assert!(
            json.contains(r#""attribute":"missing","value":null"#),
            "{json}"
        );
        let start = source.find("{missing}").unwrap();
        let end = start + "{missing}".len();
        assert!(json.contains(&format!(r#""span":{{"source_id":7,"start":{start},"end":{end}}},"severity":"warning","message":"skipping reference to missing attribute: missing""#)), "{json}");
        let tokens = super::tokens_json_with_source(source, 7);
        assert_eq!(
            tokens.matches(r#""kind":"attribute-ref""#).count(),
            2,
            "{tokens}"
        );
        assert!(
            tokens.contains(&format!(
                r#""span":{{"source_id":7,"start":{start},"end":{end}}},"kind":"attribute-ref""#
            )),
            "{tokens}"
        );
        let json = super::parse_json(":attribute-missing: drop-line\n\nBefore {missing} after");
        assert!(
            json.contains(r#""attribute":"missing","value":"""#),
            "{json}"
        );
        assert!(json.contains(r#""severity":"info""#), "{json}");
        assert!(!json.contains("Before"), "{json}");
    }

    #[test]
    fn pass_groups_export_neutral_line_breaks_and_protected_nested_text() {
        let json = super::parse_json("Before pass:n[*Ж* +\n+*Literal* {v}+] after");
        assert!(json.contains(r#""kind":"line-break""#), "{json}");
        assert!(
            json.contains(r#""kind":"open","format":"strong""#),
            "{json}"
        );
        assert!(json.contains("*Literal* {v}"), "{json}");
        assert!(!json.contains("<br>"), "{json}");
        assert!(!json.contains('\u{96}'), "{json}");
    }

    #[test]
    fn pass_group_warnings_export_physical_unicode_positions() {
        let source = "\u{feff}Before pass:basic,q[*Ж*] after";
        let json = super::parse_json(source);
        let start = source.find("basic").unwrap();
        let end = start + "basic,q".len();
        assert!(
            json.contains("invalid substitution type for passthrough macro: basic"),
            "{json}"
        );
        assert!(json.contains(r#""severity":"warning""#), "{json}");
        assert!(
            json.contains(&format!(r#""start":{start},"end":{end}"#)),
            "{json}"
        );
    }

    #[test]
    fn inline_passthrough_exports_literal_raw_and_neutral_formatting_parts() {
        let json = super::parse_json(
            ":v: Ж\n\nBefore +*Bold* {v} &amp;+ +++<b>raw</b>+++ [red]+literal+ pass:c,q[*Bold*]",
        );
        assert!(
            json.contains(r#""parts":[{"kind":"text","value":"*Bold* {v} &amp;"}]"#),
            "{json}"
        );
        assert!(
            json.contains(r#""parts":[{"kind":"raw","value":"<b>raw</b>"}]"#),
            "{json}"
        );
        assert!(
            json.contains(r#""kind":"open","format":"span","role":"red""#),
            "{json}"
        );
        assert!(
            json.contains(r#""kind":"open","format":"strong""#),
            "{json}"
        );
        assert!(!json.contains('\u{96}'), "{json}");
    }

    #[test]
    fn inline_passthrough_tokens_keep_physical_spans_and_hide_inner_syntax() {
        let source = "\u{feff}:v: Ж\r\n\r\nBefore +*Ж* {v} (C)+ after";
        let json = super::tokens_json(source);
        let start = source.find('+').unwrap();
        let end = source.rfind('+').unwrap() + 1;
        assert!(
            json.contains(&format!(r#""start":{start},"end":{end}"#)),
            "{json}"
        );
        assert!(json.contains(r#""kind":"pass-text""#), "{json}");
        assert!(!json.contains(r#""kind":"attribute-ref""#), "{json}");
        assert!(!json.contains(r#""kind":"strong""#), "{json}");
    }

    #[test]
    fn source_quote_metadata_and_typographic_text_export_neutral_semantics() {
        let json = super::parse_json(":role: red\n\nA [{role}]\"`Жёлтый`\" \\*Literal* _Italic_");
        assert!(
            json.contains(r#""parts":[{"kind":"open","format":"span","role":"red"}]"#),
            "{json}"
        );
        assert!(json.contains("“"), "{json}");
        assert!(json.contains("”"), "{json}");
        assert!(json.contains(r#""name":"emphasis""#), "{json}");
        assert!(!json.contains(r#""name":"strong""#), "{json}");
        assert!(!json.contains("<span"), "{json}");
    }

    #[test]
    fn direct_links_export_substituted_targets_and_formatted_labels() {
        let json =
            super::parse_json(":target: page?a=1&b=2\n\nlink:{target}[*Ж&*,role=red,title=Title]");
        assert!(json.contains(r#""name":"substituted","value":"Ж&","parts":[{"kind":"open-link","target":"page?a=1&amp;b=2","role":"red","title":"Title"},{"kind":"open","format":"strong"},{"kind":"text","value":"Ж&"},{"kind":"close","format":"strong"},{"kind":"close-link"}]"#), "{json}");
        assert!(
            json.contains(r#""name":"attribute-ref","attribute":"target","value":"""#),
            "{json}"
        );
        assert!(!json.contains("<a href"), "{json}");
    }

    #[test]
    fn post_attribute_macros_export_neutral_parts_without_generated_markup() {
        let json = super::parse_json(":s: ttps\n\nh{s}://example.org");
        assert!(json.contains(r#""name":"substituted","value":"https://example.org","parts":[{"kind":"open-link","target":"https://example.org","role":"bare"},{"kind":"text","value":"https://example.org"},{"kind":"close-link"}]"#), "{json}");
        assert!(!json.contains("<a href"), "{json}");
        let json = super::parse_json(":v: link:page[Ж&,role=red]\n\n{v}");
        assert!(json.contains(r#""value":"Ж&","parts":[{"kind":"open-link","target":"page","role":"red"},{"kind":"text","value":"Ж&"},{"kind":"close-link"}]"#), "{json}");
    }

    #[test]
    fn attribute_links_export_neutral_targets_properties_and_label_parts() {
        let json = super::parse_json(":v: pass:c,q,m[link:page[*Ж&*,id=anchor,role=red,title=Title,window=_blank,opts=nofollow]]\n\n{v}");
        assert!(json.contains(r#""value":"Ж&","parts":[{"kind":"open-link","target":"page","id":"anchor","role":"red","title":"Title","window":"_blank","rel":"nofollow noopener"},{"kind":"open","format":"strong"},{"kind":"text","value":"Ж&"},{"kind":"close","format":"strong"},{"kind":"close-link"}]"#), "{json}");
        assert!(!json.contains("<a href"), "{json}");
        let json = super::parse_json(":v: pass:m[mailto:a@example.org[Mail,Subject,Body]]\n\n{v}");
        assert!(
            json.contains(r#""target":"mailto:a@example.org?subject=Subject&amp;body=Body""#),
            "{json}"
        );
        let json = super::parse_json(":v: pass:m[link:page[Alpha]]\n\n<<_alpha>>\n\n== {v}");
        assert!(
            json.contains(r##""target":"#_alpha","refid":"_alpha","fragment":"_alpha""##),
            "{json}"
        );
    }

    #[test]
    fn source_quote_tokens_keep_consumed_metadata_and_unicode_offsets() {
        let source = "\u{feff}:role: красный\r\n\r\nЖ(C) [{role}]*Жёлтый* \\_Literal_";
        let json = super::parse_json_with_source(source, 7);
        let start = source.find("[{role}]").unwrap();
        let end = start + "[{role}]*".len();
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":7,"start":{start},"end":{end}}},"name":"substituted""#
            )),
            "{json}"
        );
        assert!(
            json.contains(r#""name":"attribute-ref","attribute":"role","value":"""#),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 7);
        for (kind, syntax) in [("strong", "[{role}]*Жёлтый*"), ("attribute-ref", "{role}")] {
            let start = source.find(syntax).unwrap();
            assert!(
                tokens.contains(&format!(
                    r#""span":{{"source_id":7,"start":{start},"end":{}}},"kind":"{kind}""#,
                    start + syntax.len()
                )),
                "{tokens}"
            );
        }
        assert!(!tokens.contains(r#""kind":"emphasis""#), "{tokens}");
    }

    #[test]
    fn direct_link_exports_precise_span_and_consumed_source_tokens() {
        let source = "\u{feff}:target: страница\r\n\r\nЖ(C) link:{target}[*Жёлтый*]";
        let json = super::parse_json_with_source(source, 7);
        let start = source.find("link:").unwrap();
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":7,"start":{start},"end":{}}},"name":"substituted""#,
                source.len()
            )),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 7);
        for (kind, syntax) in [
            ("link", "link:{target}[*Жёлтый*]"),
            ("strong", "*Жёлтый*"),
            ("attribute-ref", "{target}"),
        ] {
            let start = source.find(syntax).unwrap();
            assert!(
                tokens.contains(&format!(
                    r#""span":{{"source_id":7,"start":{start},"end":{}}},"kind":"{kind}""#,
                    start + syntax.len()
                )),
                "{tokens}"
            );
        }
    }

    #[test]
    fn post_attribute_macros_export_physical_spans_and_consumed_references() {
        let source = "\u{feff}:s: ttps\r\n\r\nh{s}://example.org";
        let json = super::parse_json_with_source(source, 7);
        let start = source.find("h{s}").unwrap();
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":7,"start":{start},"end":{}}},"name":"substituted""#,
                source.len()
            )),
            "{json}"
        );
        let start = start + 1;
        assert!(json.contains(&format!(r#""span":{{"source_id":7,"start":{start},"end":{}}},"name":"attribute-ref","attribute":"s","value":"""#, start+3)), "{json}");
        let tokens = super::tokens_json_with_source(source, 7);
        assert!(
            tokens.contains(&format!(
                r#""span":{{"source_id":7,"start":{start},"end":{}}},"kind":"attribute-ref""#,
                start + 3
            )),
            "{tokens}"
        );
    }

    #[test]
    fn attribute_link_snapshots_keep_physical_unicode_reference_coordinates() {
        let source = "\u{feff}:v: pass:c,m[link:страница[Ж&]]\r\n\r\n{v}";
        let json = super::parse_json_with_source(source, 7);
        let start = source.rfind("{v}").unwrap();
        assert!(
            json.contains(r#""attribute_parts":{"v":[{"kind":"open-link","target":"страница"}"#),
            "{json}"
        );
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":7,"start":{start},"end":{}}},"name":"attribute-ref""#,
                source.len()
            )),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 7);
        assert_eq!(tokens.matches(r#""kind":"attribute-entry""#).count(), 1);
        assert_eq!(tokens.matches(r#""kind":"attribute-ref""#).count(), 1);
    }

    #[test]
    fn quote_attributes_export_neutral_metadata_and_preserve_empty_roles() {
        let source = ":v: pass:c,q[[#якорь.роль]*Ж&* [..]#span#]\n\n{v}";
        let json = super::parse_json(source);
        assert!(
            json.contains(r#""kind":"open","format":"strong","id":"якорь","role":"роль""#),
            "{json}"
        );
        assert!(
            json.contains(r#""kind":"open","format":"span","role":"""#),
            "{json}"
        );
        assert!(json.contains(r#""kind":"close","format":"span""#), "{json}");
        assert!(json.contains(r#""value":"Ж& span""#), "{json}");
        assert!(!json.contains("<strong"), "{json}");
    }

    #[test]
    fn attributed_quote_parts_keep_editor_source_positions() {
        let source = "\u{feff}:v: pass:c,q[[#id.red]*Ж&*]\r\n\r\n{v}";
        let json = super::parse_json_with_source(source, 7);
        let start = source.rfind("{v}").unwrap();
        assert!(json.contains(r#""attribute_parts":{"v":[{"kind":"open","format":"strong","id":"id","role":"red"}"#), "{json}");
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":7,"start":{start},"end":{}}},"name":"attribute-ref""#,
                source.len()
            )),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 7);
        assert_eq!(tokens.matches(r#""kind":"attribute-entry""#).count(), 1);
        assert_eq!(tokens.matches(r#""kind":"attribute-ref""#).count(), 1);
    }

    #[test]
    fn pass_quotes_export_semantics_and_physical_reference_coordinates() {
        let source = "\u{feff}:v: pass:c,q[*Ж&*]\r\n\r\n{v}";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#""v":"Ж&""#), "{json}");
        assert!(json.contains(r#""attribute_parts":{"v":[{"kind":"open","format":"strong"},{"kind":"text","value":"Ж&"},{"kind":"close","format":"strong"}]}"#), "{json}");
        let start = source.rfind("{v}").unwrap();
        assert!(json.contains(&format!(r#""span":{{"source_id":7,"start":{start},"end":{}}},"name":"attribute-ref","attribute":"v","value":"Ж&","parts":[{{"kind":"open","format":"strong"}}"#, source.len())), "{json}");
        let tokens = super::tokens_json_with_source(source, 7);
        assert_eq!(tokens.matches(r#""kind":"attribute-entry""#).count(), 1);
        assert_eq!(tokens.matches(r#""kind":"attribute-ref""#).count(), 1);
        for (marker, kind) in [
            ("_", "emphasis"),
            ("`", "monospace"),
            ("#", "mark"),
            ("^", "superscript"),
            ("~", "subscript"),
        ] {
            let json = super::parse_json(&format!(":v: pass:q[{marker}x{marker}]\n\n{{v}}"));
            assert!(
                json.contains(&format!(r#"{{"kind":"open","format":"{kind}"}}"#)),
                "{json}"
            );
            assert!(
                json.contains(&format!(r#"{{"kind":"close","format":"{kind}"}}"#)),
                "{json}"
            );
        }
    }

    #[test]
    fn pass_replacements_export_definition_parts_and_reference_coordinates() {
        let source = ":v: pass:c,r[<b>(C)</b>]\n\n{v}";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#""v":"<b>&#169;</b>""#), "{json}");
        let start = source.find("{v}").unwrap();
        assert!(json.contains(&format!(r#""span":{{"source_id":7,"start":{start},"end":{}}},"name":"attribute-ref","attribute":"v","value":"<b>&#169;</b>","parts":[{{"kind":"text","value":"<b>"}},{{"kind":"raw","value":"&#169;"}},{{"kind":"text","value":"</b>"}}]"#, source.len())), "{json}");
        assert!(json.contains(r#""attribute_parts":{"v":[{"kind":"text","value":"<b>"},{"kind":"raw","value":"&#169;"},{"kind":"text","value":"</b>"}]}"#), "{json}");
        let tokens = super::tokens_json_with_source(source, 7);
        assert_eq!(tokens.matches(r#""kind":"attribute-entry""#).count(), 1);
        assert_eq!(tokens.matches(r#""kind":"attribute-ref""#).count(), 1);
    }

    #[test]
    fn shared_replacements_export_raw_spans_and_keep_consumed_attribute_tokens() {
        let source = ":v: C\n:name: copy\n\n({v}) &{name};";
        let json = super::parse_json_with_source(source, 8);
        let start = source.find("&{name};").unwrap();
        assert!(json.contains(&format!(r#""span":{{"source_id":8,"start":{start},"end":{}}},"name":"raw","value":"&copy;""#, source.len())), "{json}");
        assert!(json.contains(r#""attribute":"v","value":"""#), "{json}");
        assert!(json.contains(r#""value":"©""#), "{json}");
        let tokens = super::tokens_json_with_source(source, 8);
        assert_eq!(tokens.matches(r#""kind":"attribute-ref""#).count(), 2);
    }

    #[test]
    fn restored_entities_preserve_attribute_values_and_reference_coordinates() {
        let source = ":v: pass:[&amp;copy;]\n\n{v}";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#""v":"&amp;copy;""#), "{json}");
        let start = source.find("{v}").unwrap();
        assert!(json.contains(&format!(r#""span":{{"source_id":7,"start":{start},"end":{}}},"name":"attribute-ref","attribute":"v","value":"&copy;","parts":[{{"kind":"raw","value":"&copy;"}}]"#, source.len())), "{json}");
        let tokens = super::tokens_json_with_source(source, 7);
        assert_eq!(tokens.matches(r#""kind":"attribute-ref""#).count(), 1);
    }

    #[test]
    fn author_markup_exports_attribute_boundaries_and_physical_reference_span() {
        let source =
            "= Doc\nJohn Doe\n:name: pass:[<b>Анна</b>]\n:author: {name} Смирнова\n\n{author}";
        let json = super::parse_json_with_source(source, 9);
        assert!(json.contains(r#""name":"John Doe""#), "{json}");
        assert!(json.contains(r#""firstname":"Анна""#), "{json}");
        assert!(json.contains(r#""attribute_parts":{"author":[{"kind":"raw","value":"<b>Анна</b>"},{"kind":"text","value":" Смирнова"}]"#), "{json}");
        let start = source.rfind("{author}").unwrap();
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":9,"start":{start},"end":{}}},"name":"attribute-ref""#,
                source.len()
            )),
            "{json}"
        );
        assert!(json.contains(r#""parts":[{"kind":"raw","value":"<b>Анна</b>"},{"kind":"text","value":" Смирнова"}]"#), "{json}");
    }

    #[test]
    fn indexed_authors_keep_declarations_and_tokens_separate_from_effective_names() {
        let source = "= Doc\nJohn Doe; Mary Roe\n:author_2: Alice Brown\n\n{authors}";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#""author_2":"Alice Brown""#), "{json}");
        assert!(
            json.contains(r#""value":"John Doe, Alice Brown""#),
            "{json}"
        );
        assert!(
            json.contains(r#""span":{"source_id":7,"start":16,"end":24},"name":"Mary Roe""#),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 7);
        assert!(
            tokens.contains(r#""span":{"source_id":7,"start":6,"end":24},"kind":"author""#),
            "{tokens}"
        );
        assert!(tokens.contains(r#""kind":"attribute-entry""#), "{tokens}");
    }

    #[test]
    fn doctitle_override_exports_declaration_and_replacement_source() {
        let source = "= Original\n:doctitle: *Effective*\n\nBody";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#""title":"Original""#), "{json}");
        assert!(json.contains(r#""value":"Effective""#), "{json}");
        assert!(
            json.contains(r#""span":{"source_id":7,"start":11,"end":33},"name":"strong""#),
            "{json}"
        );
        assert_eq!(json.matches(r#""name":"heading""#).count(), 1);
        let tokens = super::tokens_json_with_source(source, 7);
        assert_eq!(tokens.matches(r#""kind":"heading""#).count(), 1);
        assert!(tokens.contains(r#""kind":"attribute-entry""#), "{tokens}");
    }

    #[test]
    fn setext_header_exports_both_title_lines_and_physical_heading_tokens() {
        let source = "*Я*\r\n===\r\nJane Doe\r\nv2\r\n\r\n{author}";
        let json = super::parse_json_with_source(source, 7);
        assert!(
            json.contains(r#""title_span":{"source_id":7,"start":0,"end":9}"#),
            "{json}"
        );
        assert!(json.contains(r#""name":"strong""#), "{json}");
        assert!(json.contains(r#""compat-mode":"""#), "{json}");
        assert!(json.contains(r#""name":"Jane Doe""#), "{json}");
        let tokens = super::tokens_json_with_source(source, 7);
        for (start, end) in [(0, 4), (6, 9)] {
            assert!(
                tokens.contains(&format!(
                    r#"{{"span":{{"source_id":7,"start":{start},"end":{end}}},"kind":"heading"}}"#
                )),
                "{tokens}"
            );
        }
        assert!(!tokens.contains("example-delimiter"), "{tokens}");
    }

    #[test]
    fn setext_body_section_exports_two_line_span_and_heading_tokens() {
        let source = "Intro\r\n\r\n*Я*\r\n----\r\n\r\nBody";
        let json = super::parse_json_with_source(source, 7);
        assert!(
            json.contains(
                r#"{"span":{"source_id":7,"start":9,"end":19},"name":"heading","level":1"#
            ),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 7);
        for (start, end) in [(9, 13), (15, 19)] {
            assert!(
                tokens.contains(&format!(
                    r#"{{"span":{{"source_id":7,"start":{start},"end":{end}}},"kind":"heading"}}"#
                )),
                "{tokens}"
            );
        }
        assert!(!tokens.contains("listing-delimiter"), "{tokens}");
    }

    #[test]
    fn sections_export_nested_spans_for_folding() {
        let source = "= Doc\r\n\r\nPre\r\n\r\n== Я\r\n\r\nx\r\n\r\nPara\r\n== T";
        let json = super::parse_json_with_source(source, 7);
        assert!(
            json.contains(r#"{"span":{"source_id":7,"start":9,"end":12},"name":"preamble","title":null,"blocks":[{"span":{"source_id":7,"start":9,"end":12},"name":"paragraph""#),
            "{json}"
        );
        // The section spans its title through the paragraph that swallowed `== T`.
        assert!(
            json.contains(r#"{"span":{"source_id":7,"start":16,"end":40},"name":"section","level":1,"blocks":[{"span":{"source_id":7,"start":16,"end":21},"name":"heading""#),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 7);
        assert_eq!(tokens.matches(r#""kind":"heading""#).count(), 2, "{tokens}");
    }

    #[test]
    fn image_macro_keeps_its_span_and_block_macro_token() {
        let source = "Intro\n\n.T\nimage::a.png[A,link=l]";
        let json = super::parse_json_with_source(source, 3);
        assert!(
            json.contains(r#"{"span":{"source_id":3,"start":10,"end":32},"name":"image","target":"a.png","src":"a.png","alt":"A","id":null,"roles":[],"style":null,"attributes":{"alt":"A","link":"l"},"options":[],"title":[{"span":{"source_id":3,"start":8,"end":9},"name":"text","value":"T"}],"caption":"Figure 1. "}"#),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 3);
        assert!(
            tokens.contains(r#"{"span":{"source_id":3,"start":10,"end":32},"kind":"block-macro"}"#),
            "{tokens}"
        );
    }

    #[test]
    fn block_subs_export_converted_content_spans_and_tokens() {
        let source = "[subs=\"+quotes\"]\n----\n*x* // <1>\n----\n\n[subs=none]\n*p* {a}";
        let json = super::parse_json_with_source(source, 4);
        assert!(json.contains(r#""text":"*x* // <1>","inlines":[{"span":{"source_id":4,"start":22,"end":32},"name":"substituted","value":"x ","parts":[{"kind":"open","format":"strong"},{"kind":"text","value":"x"},{"kind":"close","format":"strong"},{"kind":"text","value":" "},{"kind":"callout","number":"1","id":"CO1-1","guard":"// ","icons":null,"xml":false,"src":""}]}]"#), "{json}");
        assert!(json.contains(r#""inlines":[{"span":{"source_id":4,"start":51,"end":58},"name":"substituted","value":"*p* {a}","parts":[{"kind":"raw","value":"*p* {a}"}]}]"#), "{json}");
        let tokens = super::tokens_json_with_source(source, 4);
        assert!(
            tokens.contains(r#"{"span":{"source_id":4,"start":26,"end":32},"kind":"callout"}"#),
            "{tokens}"
        );
        // Unselected stages leave no inline tokens.
        assert!(!tokens.contains("strong"), "{tokens}");
        assert!(!tokens.contains("attribute-ref"), "{tokens}");
    }

    #[test]
    fn document_attributes_export_icon_font_defaults() {
        let json = super::parse_json("x");
        assert!(json.contains(r#""iconfont-remote":"""#), "{json}");
        assert!(json.contains(r#""stylesdir":".""#), "{json}");
        let json = super::parse_json(":iconfont-remote!:\n\nx");
        assert!(!json.contains("iconfont-remote"), "{json}");
    }

    #[test]
    fn admonition_icons_export_the_mode_where_blocks_convert() {
        let json = super::parse_json("NOTE: a\n\n:icons:\n:iconsdir: ic\n\n[TIP,icon=b.svg]\nb");
        assert!(
            json.contains(r#""simple":true,"icons":null,"src":"","blocks""#),
            "{json}"
        );
        assert!(
            json.contains(r#""simple":true,"icons":"","src":"ic/b.svg","blocks""#),
            "{json}"
        );
    }

    #[test]
    fn callout_icons_export_the_mode_where_blocks_convert() {
        let json = super::parse_json(
            ":icons: font\n\n----\nx <1>\n----\n<1> a\n\n:icons!:\n\n----\ny <1>\n----\n<1> b",
        );
        assert!(
            json.contains(
                r#""number":"1","id":"CO1-1","guard":null,"xml":false,"icons":"font","src":""}]"#
            ),
            "{json}"
        );
        assert!(
            json.contains(r#""coids":["CO1-1"],"icons":"font","src":""}"#),
            "{json}"
        );
        assert!(
            json.contains(
                r#""number":"1","id":"CO2-1","guard":null,"xml":false,"icons":null,"src":""}]"#
            ),
            "{json}"
        );
    }

    #[test]
    fn callouts_export_spans_and_tokens() {
        let source = "----\nx <1>\n----\n<1> one";
        let json = super::parse_json_with_source(source, 3);
        assert!(json.contains(r#""callouts":[{"span":{"source_id":3,"start":7,"end":10},"number":"1","id":"CO1-1","guard":null,"xml":false,"icons":null,"src":""}]"#), "{json}");
        assert!(
            json.contains(r#""blocks":[],"coids":["CO1-1"],"icons":null,"src":""}"#),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 3);
        assert!(
            tokens.contains(r#"{"span":{"source_id":3,"start":7,"end":10},"kind":"callout"}"#),
            "{tokens}"
        );
    }

    #[test]
    fn description_lists_export_term_spans_and_marker_tokens() {
        let source = "CPU:: brain";
        let json = super::parse_json_with_source(source, 3);
        assert!(json.contains(r#""items":[{"span":{"source_id":3,"start":0,"end":11},"terms":[{"span":{"source_id":3,"start":0,"end":3},"inlines":[{"span":{"source_id":3,"start":0,"end":3},"name":"text","value":"CPU"}]}],"description":{"span":{"source_id":3,"start":6,"end":11},"inlines":[{"span":{"source_id":3,"start":6,"end":11},"name":"text","value":"brain"}],"blocks":[]}}]"#), "{json}");
        let tokens = super::tokens_json_with_source(source, 3);
        assert!(
            tokens.contains(r#"{"span":{"source_id":3,"start":3,"end":5},"kind":"list-marker"}"#),
            "{tokens}"
        );
    }

    #[test]
    fn stem_macros_export_source_spans_and_multiline_tokens() {
        let source = "Ж stem:[а\r\n*б*]";
        let json = super::parse_json_with_source(source, 7);
        assert!(json.contains(r#"{"span":{"source_id":7,"start":3,"end":18},"name":"stem","style":"asciimath","parts":[{"kind":"text","value":"а\n*б*"}]}"#), "{json}");
        let tokens = super::tokens_json_with_source(source, 7);
        assert!(
            tokens.contains(r#"{"span":{"source_id":7,"start":3,"end":18},"kind":"inline-macro"}"#),
            "{tokens}"
        );
        assert!(!tokens.contains(r#""kind":"strong""#), "{tokens}");
        let json = super::parse_json(":s: pass:m[latexmath:[x]]\n\n{s}");
        assert!(
            json.contains(
                r#"{"kind":"stem","style":"latexmath","parts":[{"kind":"text","value":"x"}]}"#
            ),
            "{json}"
        );
    }

    #[test]
    fn index_terms_export_metadata_spans_and_tokens() {
        let source = "Ж ((Лев)) (((Кот)))";
        let json = super::parse_json_with_source(source, 3);
        assert!(json.contains(r#"{"span":{"source_id":3,"start":3,"end":13},"name":"indexterm","parts":[{"kind":"text","value":"Лев"}],"terms":[],"see":null,"see_also":[]}"#), "{json}");
        assert!(json.contains(r#"{"span":{"source_id":3,"start":14,"end":26},"name":"indexterm","parts":null,"terms":["Кот"],"see":null,"see_also":[]}"#), "{json}");
        let tokens = super::tokens_json_with_source(source, 3);
        assert!(
            tokens.contains(r#"{"span":{"source_id":3,"start":3,"end":13},"kind":"inline-macro"}"#),
            "{tokens}"
        );
        assert!(
            tokens
                .contains(r#"{"span":{"source_id":3,"start":14,"end":26},"kind":"inline-macro"}"#),
            "{tokens}"
        );
        let json = super::parse_json(":t: ((Attr))\n\n{t}");
        assert!(json.contains(r#"{"kind":"indexterm","parts":[{"kind":"text","value":"Attr"}],"terms":[],"see":null,"see_also":[]}"#), "{json}");
    }

    #[test]
    fn manual_pages_export_the_name_section_with_spans_and_tokens() {
        let source = "= x(1)\n:doctype: manpage\n\n== NAME\n\n*a* - {doctype}\n";
        let json = super::parse_json_with_source(source, 3);
        assert!(json.contains(r#","manpage":{"names":["*a*"],"name_section":{"heading":{"span":{"source_id":3,"start":26,"end":33},"name":"heading","level":1,"id":"_name","#), "{json}");
        assert!(
            json.contains(r#""body":{"source_id":3,"start":35,"end":50}}}"#),
            "{json}"
        );
        assert!(json.contains(r#""manpurpose":"manpage""#), "{json}");
        let tokens = super::tokens_json_with_source(source, 3);
        assert!(tokens.contains(r#"{"span":{"source_id":3,"start":26,"end":33},"kind":"heading"},{"span":{"source_id":3,"start":41,"end":50},"kind":"attribute-ref"}"#), "{tokens}");
        assert!(!tokens.contains("strong"), "{tokens}");
    }

    #[test]
    fn ui_macros_export_spans_and_inline_macro_tokens() {
        let source = ":experimental:\n\nA kbd:[B] btn:[C] \"D > E\"";
        let json = super::parse_json_with_source(source, 3);
        assert!(json.contains(r#"{"span":{"source_id":3,"start":18,"end":25},"name":"kbd","keys":[[{"kind":"text","value":"B"}]]}"#), "{json}");
        assert!(json.contains(r#"{"span":{"source_id":3,"start":26,"end":33},"name":"button","parts":[{"kind":"text","value":"C"}]}"#), "{json}");
        assert!(json.contains(r#"{"span":{"source_id":3,"start":34,"end":41},"name":"menu","menu":[{"kind":"text","value":"D"}],"submenus":[],"item":[{"kind":"text","value":"E"}],"font":false}"#), "{json}");
        // A nested link is a part of the label; the macro keeps one span.
        let nested = super::parse_json_with_source(":experimental:\n\nbtn:[link:x[y]]", 3);
        assert!(nested.contains(r#"{"span":{"source_id":3,"start":16,"end":31},"name":"button","parts":[{"kind":"open-link","target":"x"},{"kind":"text","value":"y"},{"kind":"close-link"}]}"#), "{nested}");
        let tokens = super::tokens_json_with_source(source, 3);
        for span in ["18,\"end\":25", "26,\"end\":33", "34,\"end\":41"] {
            assert!(
                tokens.contains(&format!(
                    "{{\"span\":{{\"source_id\":3,\"start\":{span}}},\"kind\":\"inline-macro\"}}"
                )),
                "{tokens}"
            );
        }
    }

    #[test]
    fn footnotes_export_spans_and_inline_macro_tokens() {
        let source = "A footnote:[n] z";
        let json = super::parse_json_with_source(source, 3);
        assert!(json.contains(r#"{"span":{"source_id":3,"start":2,"end":14},"name":"footnote","type":"definition","index":"1","id":null,"parts":[{"kind":"text","value":"n"}]}"#), "{json}");
        assert!(json.contains(r#""footnotes":[{"span":{"source_id":3,"start":2,"end":14},"index":"1","id":null,"parts":[{"kind":"text","value":"n"}]}]"#), "{json}");
        let tokens = super::tokens_json_with_source(source, 3);
        assert!(
            tokens.contains(r#"{"span":{"source_id":3,"start":2,"end":14},"kind":"inline-macro"}"#),
            "{tokens}"
        );
    }

    #[test]
    fn asciidoc_cells_export_blocks_with_spans() {
        let json = super::parse_json_with_source("|===\na|para\n|===", 3);
        assert!(
            json.contains(r#""style":"asciidoc","halign":"left","valign":"top","colspan":null,"rowspan":null,"background":null,"blocks":[{"span":{"source_id":3,"start":7,"end":11},"name":"paragraph","#),
            "{json}"
        );
    }

    #[test]
    fn tables_export_columns_cells_with_spans_and_tokens() {
        let source = ".T\n[cols=\"1,2\"]\n|===\n|H |*I*\n\n2+|l\n|===";
        let json = super::parse_json_with_source(source, 3);
        assert!(
            json.contains(r#""name":"table","id":null,"roles":[],"style":null,"attributes":{"cols":"1,2"},"options":[],"title":[{"span":{"source_id":3,"start":1,"end":2},"name":"text","value":"T"}],"caption":"Table 1. ","width":100,"frame":"all","grid":"all","stripes":null,"columns":[{"halign":"left","valign":"top","style":null,"width":"33.3333","autowidth":false},{"halign":"left","valign":"top","style":null,"width":"66.6667","autowidth":false}]"#),
            "{json}"
        );
        assert!(
            json.contains(r#""head":[[{"span":{"source_id":3,"start":21,"end":23},"style":null,"halign":"left","valign":"top","colspan":null,"rowspan":null,"background":null,"paragraphs":[[{"span":{"source_id":3,"start":22,"end":23},"name":"text","value":"H"}]]},{"span":{"source_id":3,"start":24,"end":28},"style":null,"halign":"left","valign":"top","colspan":null,"rowspan":null,"background":null,"paragraphs":[[{"span":{"source_id":3,"start":25,"end":28},"name":"strong""#),
            "{json}"
        );
        assert!(
            json.contains(r#""body":[[{"span":{"source_id":3,"start":30,"end":34},"style":null,"halign":"left","valign":"top","colspan":2,"rowspan":null,"background":null,"paragraphs":[[{"span":{"source_id":3,"start":33,"end":34},"name":"text","value":"l"}]]}]],"foot":[]"#),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 3);
        for token in [
            r#"{"span":{"source_id":3,"start":16,"end":20},"kind":"table-delimiter"}"#,
            r#"{"span":{"source_id":3,"start":30,"end":33},"kind":"cell-separator"}"#,
            r#"{"span":{"source_id":3,"start":25,"end":28},"kind":"strong"}"#,
        ] {
            assert!(tokens.contains(token), "{tokens}");
        }
    }

    #[test]
    fn inline_images_keep_their_spans_and_inline_macro_tokens() {
        let source = "A image:a.png[B] z";
        let json = super::parse_json_with_source(source, 3);
        assert!(
            json.contains(r#"{"span":{"source_id":3,"start":2,"end":16},"name":"image","icon":false,"target":"a.png","src":"a.png","alt":"B","icons":null,"attributes":{},"options":[],"parts":[{"kind":"text","value":"B"}]}"#),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 3);
        assert!(
            tokens.contains(r#"{"span":{"source_id":3,"start":2,"end":16},"kind":"inline-macro"}"#),
            "{tokens}"
        );
    }

    #[test]
    fn media_macros_keep_their_spans_and_block_macro_tokens() {
        let source = "video::v.mp4[vimeo]\n\n.T\naudio::s.ogg[start=1]";
        let json = super::parse_json_with_source(source, 3);
        assert!(
            json.contains(r#"{"span":{"source_id":3,"start":0,"end":19},"name":"video","target":"v.mp4","src":"https://player.vimeo.com/video/v.mp4","poster":null,"embedded":true,"id":null,"roles":[],"style":null,"attributes":{"poster":"vimeo"},"options":[],"title":null,"caption":null}"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"span":{"source_id":3,"start":24,"end":45},"name":"audio","target":"s.ogg","src":"s.ogg#t=1","id":null,"roles":[],"style":null,"attributes":{"start":"1"},"options":[],"title":[{"span":{"source_id":3,"start":22,"end":23},"name":"text","value":"T"}],"caption":null}"#),
            "{json}"
        );
        let tokens = super::tokens_json_with_source(source, 3);
        for (start, end) in [(0, 19), (24, 45)] {
            let token = format!(
                r#"{{"span":{{"source_id":3,"start":{start},"end":{end}}},"kind":"block-macro"}}"#
            );
            assert!(tokens.contains(&token), "{tokens}");
        }
    }

    #[test]
    fn toc_macro_keeps_its_span_and_block_macro_token() {
        let source = "= D\n:toc: macro\n\ntoc::[levels=1]\n\n== A";
        let json = super::parse_json_with_source(source, 2);
        assert!(
            json.contains(r#"{"span":{"source_id":2,"start":17,"end":32},"name":"toc","id":null,"roles":[],"style":null,"attributes":{"levels":"1"},"options":[],"title":null,"caption":null}"#),
            "{json}"
        );
        assert!(json.contains(r#""toc-placement":"macro""#), "{json}");
        let tokens = super::tokens_json_with_source(source, 2);
        assert!(
            tokens.contains(r#"{"span":{"source_id":2,"start":17,"end":32},"kind":"block-macro"}"#),
            "{tokens}"
        );
    }

    #[test]
    fn book_preamble_title_and_partintro_reach_the_editor() {
        let source = "= B\n:doctype: book\n:preface-title: Fore\n\nPre\n\n= P\n\nIntro\n\n== C";
        let json = super::parse_json_with_source(source, 3);
        // The preface title has no source text of its own: an empty span at the
        // preamble start.
        assert!(
            json.contains(r#"{"span":{"source_id":3,"start":41,"end":44},"name":"preamble","title":[{"span":{"source_id":3,"start":41,"end":41},"name":"text","value":"Fore"}],"blocks":["#),
            "{json}"
        );
        // The generated partintro spans the content it wraps.
        assert!(
            json.contains(r#"{"span":{"source_id":3,"start":51,"end":56},"name":"open","id":null,"roles":[],"style":"partintro""#),
            "{json}"
        );
    }

    #[test]
    fn header_metadata_and_tokens_keep_source_coordinates() {
        let json =
            super::parse_json_with_source("= Doc\nJane Doe <j@example.org>\nv2\n\n{author}", 7);
        assert!(json.contains(r#""header":{"span":{"source_id":7,"start":0,"end":33},"title_span":{"source_id":7,"start":0,"end":5},"authors":[{"span":{"source_id":7,"start":6,"end":30},"name":"Jane Doe""#), "{json}");
        assert!(json.contains(r#""revision":{"span":{"source_id":7,"start":31,"end":33},"number":"2","date":null,"remark":null}"#), "{json}");
        assert!(json.contains(r#""author":"Jane Doe""#), "{json}");
        let tokens = super::tokens_json_with_source("= Doc\nJane Doe <j@example.org>\nv2", 7);
        assert!(
            tokens.contains(r#"{"span":{"source_id":7,"start":6,"end":30},"kind":"author"}"#),
            "{tokens}"
        );
        assert!(
            tokens.contains(r#"{"span":{"source_id":7,"start":31,"end":33},"kind":"revision"}"#),
            "{tokens}"
        );
    }
}

#[cfg(test)]
mod comment_tests {
    #[test]
    fn omitted_comments_are_available_as_editor_ranges() {
        let src = "// hidden\n\nbody\n////\nhidden\n////";
        let json = super::parse_json_with_source(src, 12);
        assert!(json.contains(r#""comments":[{"span":{"source_id":12,"start":0,"end":9}},{"span":{"source_id":12,"start":16,"end":32}}]"#), "{json}");
        assert!(!json.contains("hidden"), "{json}");
        let tokens = super::tokens_json_with_source(src, 12);
        assert_eq!(
            tokens,
            r#"[{"span":{"source_id":12,"start":0,"end":9},"kind":"comment"},{"span":{"source_id":12,"start":16,"end":32},"kind":"comment"}]"#
        );
    }
}

#[cfg(test)]
mod conditional_tests {
    #[test]
    fn conditional_trivia_and_selected_content_use_original_editor_ranges() {
        let src = "ifdef::missing[]\nhidden\nendif::[]\nifndef::missing[*yes*]";
        let json = super::parse_json_with_source(src, 12);
        assert!(
            json.contains(
                r#""conditional_directives":[{"span":{"source_id":12,"start":0,"end":16}}"#
            ),
            "{json}"
        );
        assert!(
            json.contains(r#""inactive":[{"span":{"source_id":12,"start":17,"end":23}}]"#),
            "{json}"
        );
        assert!(!json.contains("hidden"), "{json}");
        let tokens = super::tokens_json_with_source(src, 12);
        let start = src.find("*yes*").unwrap();
        assert!(
            tokens.contains(&format!(
                r#"{{"span":{{"source_id":12,"start":{start},"end":{}}},"kind":"strong"}}"#,
                start + 5
            )),
            "{tokens}"
        );
        assert!(tokens.contains(r#""kind":"inactive""#));
        assert!(tokens.contains(r#""kind":"conditional-directive""#));
    }
}

#[cfg(test)]
mod ifeval_tests {
    #[test]
    fn selected_content_trivia_and_errors_keep_editor_source_ranges() {
        let src = "ifeval::[1 > 2]\r\nскрыто\r\nendif::[]\r\nifeval::[2 > 1]\r\n*Привет*\r\nendif::[]\r\nifeval::[bad]";
        let json = super::parse_json_with_source(src, 29);
        let tokens = super::tokens_json_with_source(src, 29);
        assert!(!json.contains("скрыто"), "{json}");
        assert!(json.contains("Привет"), "{json}");
        for (text, kind) in [
            ("ifeval::[1 > 2]", "conditional-directive"),
            ("скрыто", "inactive"),
            ("*Привет*", "strong"),
            ("ifeval::[bad]", "conditional-directive"),
        ] {
            let start = src.find(text).unwrap();
            let end = start + text.len();
            assert!(
                tokens.contains(&format!(
                    r#"{{"span":{{"source_id":29,"start":{start},"end":{end}}},"kind":"{kind}"}}"#
                )),
                "{tokens}"
            );
        }
        assert!(json.contains("invalid expression"), "{json}");
        let start = src.find("ifeval::[bad]").unwrap();
        let end = src.len();
        assert!(
            json.contains(&format!(
                r#""span":{{"source_id":29,"start":{start},"end":{end}}},"severity":"error""#
            )),
            "{json}"
        );
    }

    #[test]
    fn late_anchors_keep_spans_and_attribute_parts() {
        let src = ":id: foo\n:a: [[q]]x\n\nA [[{id}]] {a}";
        let json = super::parse_json(src);
        let start = src.find("[[{id}]]").unwrap();
        let span = format!(
            r#"{{"span":{{"source_id":0,"start":{start},"end":{}}}"#,
            start + 8
        );
        assert!(
            json.contains(&format!(r#"{span},"name":"anchor","id":"foo"}}"#)),
            "{json}"
        );
        assert!(
            json.contains(r#""parts":[{"kind":"anchor","id":"q"},{"kind":"text","value":"x"}]"#),
            "{json}"
        );
        let tokens = super::tokens_json(src);
        assert!(
            tokens.contains(&format!(r#"{span},"kind":"anchor"}}"#)),
            "{tokens}"
        );
    }

    #[test]
    fn late_xrefs_keep_spans_roles_and_attribute_parts() {
        let src = ":id: s\n:r: <<s>>\n\n[[s]]\n== T\n\nA <<{id}>> {r} xref:s[x,role=r]";
        let json = super::parse_json(src);
        let start = src.find("<<{id}>>").unwrap();
        let span = format!(
            r#"{{"span":{{"source_id":0,"start":{start},"end":{}}}"#,
            start + 8
        );
        assert!(
            json.contains(&format!(
                r##"{span},"name":"xref","target":"#s","refid":"s","fragment":"s","inlines":"##
            )),
            "{json}"
        );
        assert!(
            json.contains(r##""parts":[{"kind":"open-xref","target":"#s","refid":"s","fragment":"s"},{"kind":"text","value":"T"},{"kind":"close-xref"}]"##),
            "{json}"
        );
        assert!(
            json.contains(
                r##""name":"xref","target":"#s","refid":"s","fragment":"s","role":"r","inlines":"##
            ),
            "{json}"
        );
        let tokens = super::tokens_json(src);
        assert!(
            tokens.contains(&format!(r#"{span},"kind":"xref"}}"#)),
            "{tokens}"
        );
    }

    #[test]
    fn xref_fields_and_labels_keep_character_references_with_spans() {
        let json = crate::parse_json("xref:a&#38;b[]");
        assert!(
            json.contains(r##""name":"xref","target":"#a&#38;b","refid":"a&#38;b""##),
            "got: {json}"
        );
        // Every label node carries the reference's span.
        assert!(
            json.contains(
                r#"{"span":{"source_id":0,"start":0,"end":14},"name":"raw","value":"&#38;"}"#
            ),
            "got: {json}"
        );
    }
}
