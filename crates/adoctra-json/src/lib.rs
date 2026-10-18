//! JSON (ASG) backend.
//!
//! Emits a JSON tree of the document; the natural place to target the official
//! AsciiDoc Abstract Syntax Graph and act as a TCK adapter. Hand-serialized to
//! stay dependency-free; switch to serde for the real ASG shape.

use adoctra_core::ast::{Attribution, Block, BlockKind, Inline};
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
        s.push(']');
        adoctra_core::json::write_footnote_definitions(&doc.footnotes, &mut s, None);
        if let Some(header) = &doc.header {
            s.push_str(",\"header\":");
            write_header(header, &mut s);
        }
        if let Some(manpage) = &doc.manpage {
            write_manpage(manpage, &mut s);
        }
        s.push('}');
        Ok(s.into_bytes())
    }
}

/// A manual page's names and the NAME section its header read (`null` when
/// the names come from attributes); the values stay in the attributes.
fn write_manpage(manpage: &adoctra_core::Manpage, s: &mut String) {
    s.push_str(",\"manpage\":{\"names\":[");
    for (i, name) in manpage.names.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        escape_into(name, s);
    }
    s.push_str("],\"name_section\":");
    match &manpage.name_section {
        Some(section) => {
            s.push_str("{\"heading\":");
            write_block(&section.heading, s);
            s.push_str(&format!(",\"conforming\":{}}}", section.body.is_some()));
        }
        None => s.push_str("null"),
    }
    s.push('}');
}

// Header declarations share the core model; effective overrides remain attributes.
fn write_header(header: &adoctra_core::DocumentHeader, s: &mut String) {
    s.push('{');

    s.push_str("\"authors\":[");
    for (i, author) in header.authors.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('{');

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
            escape_into(key, s);
            s.push(':');
            if let Some(value) = value {
                escape_into(value, s);
            } else {
                s.push_str("null");
            }
        }
        s.push('}');
    }
    s.push_str("],\"revision\":");
    if let Some(revision) = &header.revision {
        s.push('{');

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
            escape_into(key, s);
            s.push(':');
            if let Some(value) = value {
                escape_into(value, s);
            } else {
                s.push_str("null");
            }
        }
        s.push('}');
    } else {
        s.push_str("null");
    }
    s.push_str(",\"title\":");
    escape_into(&header.title, s);
    s.push('}');
}

/// A table: its resolved presentation (`width` percentage, `frame`, `grid`,
/// `stripes`), the columns and the head/body/foot rows of cells. A cell has
/// `paragraphs` (arrays of inlines; one run in the head), the `blocks` of an
/// AsciiDoc cell or verbatim `literal` text.
fn write_table(table: &adoctra_core::ast::Table, s: &mut String) {
    s.push_str(&format!(",\"width\":{},\"frame\":", table.width));
    escape_into(&table.frame, s);
    s.push_str(",\"grid\":");
    escape_into(&table.grid, s);
    s.push_str(",\"stripes\":");
    match &table.stripes {
        Some(stripes) => escape_into(stripes, s),
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
        escape_into(&column.width, s);
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
        Some(color) => escape_into(color, s),
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
        adoctra_core::ast::CellContent::Blocks { blocks, footnotes } => {
            s.push_str(",\"blocks\":[");
            for (i, block) in blocks.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_block(block, s);
            }
            s.push(']');
            adoctra_core::json::write_footnote_definitions(footnotes, s, None);
        }
        adoctra_core::ast::CellContent::Literal(text) => {
            s.push_str(",\"literal\":");
            escape_into(text, s);
        }
    }
    s.push('}');
}

/// A callout icon captured where its block converts: the `icons` mode and
/// the image icon (empty unless icons are images).
fn write_icon(icons: &Option<String>, src: &str, s: &mut String) {
    s.push_str(",\"icons\":");
    match icons {
        Some(icons) => escape_into(icons, s),
        None => s.push_str("null"),
    }
    s.push_str(",\"src\":");
    escape_into(src, s);
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
        s.push_str("\"number\":");
        escape_into(&callout.number, s);
        s.push_str(",\"id\":");
        escape_into(&callout.id, s);
        s.push_str(",\"guard\":");
        match &callout.guard {
            Some(guard) => escape_into(guard, s),
            None => s.push_str("null"),
        }
        s.push_str(&format!(",\"xml\":{}", callout.xml));
        write_icon(&callout.icons, &callout.src, s);
        s.push('}');
    }
    s.push(']');
}

/// A list item (or a description): its text, nested blocks and, in a
/// checklist, its `checkbox` state.
fn write_list_item(item: &adoctra_core::ast::ListItem, s: &mut String) {
    s.push_str("{\"inlines\":");
    write_inlines(&item.inlines, s);
    if let Some(checked) = item.checkbox {
        s.push_str(&format!(",\"checkbox\":{checked}"));
    }
    s.push_str(",\"blocks\":[");
    for (j, child) in item.blocks.iter().enumerate() {
        if j > 0 {
            s.push(',');
        }
        write_block(child, s);
    }
    s.push_str("]}");
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
            write_caption(&block.caption, s);
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
            write_caption(&block.caption, s);
            s.push_str(",\"inlines\":");
            write_inlines(inlines, s);
            s.push('}');
        }
        BlockKind::Listing {
            text,
            callouts,
            inlines,
        } => {
            s.push_str("{\"name\":\"listing\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            s.push_str(",\"text\":");
            escape_into(text, s);
            write_callouts(callouts, s);
            write_substituted(inlines, s);
            s.push('}');
        }
        BlockKind::Literal {
            text,
            callouts,
            inlines,
        } => {
            s.push_str("{\"name\":\"literal\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            s.push_str(",\"text\":");
            escape_into(text, s);
            write_callouts(callouts, s);
            write_substituted(inlines, s);
            s.push('}');
        }
        BlockKind::Stem { style, inlines } => {
            s.push_str("{\"name\":\"stem\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&Some(style.as_str().into()), s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            s.push_str(",\"inlines\":");
            write_inlines(inlines, s);
            s.push('}');
        }
        BlockKind::Passthrough { text, inlines } => {
            s.push_str("{\"name\":\"pass\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            s.push_str(",\"text\":");
            escape_into(text, s);
            write_substituted(inlines, s);
            s.push('}');
        }
        BlockKind::ThematicBreak | BlockKind::PageBreak => {
            // The ASG `break` block with its variant; it has no content.
            s.push_str("{\"name\":\"break\",\"variant\":");
            s.push_str(if matches!(block.kind, BlockKind::PageBreak) {
                "\"page\","
            } else {
                "\"thematic\","
            });
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            s.push('}');
        }
        BlockKind::Toc => {
            // The `toc::[]` macro; consumers build the outline from sections.
            s.push_str("{\"name\":\"toc\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            s.push('}');
        }
        BlockKind::Image { target, src, alt } => {
            // The resolved image: its target, web path and flat alt text.
            s.push_str("{\"name\":\"image\",\"target\":");
            escape_into(target, s);
            s.push_str(",\"src\":");
            escape_into(src, s);
            s.push_str(",\"alt\":");
            escape_into(&adoctra_core::attributes::parts_text(alt), s);
            s.push(',');
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            s.push('}');
        }
        BlockKind::Table(table) => {
            s.push_str("{\"name\":\"table\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            write_table(table, s);
            s.push('}');
        }
        BlockKind::Video {
            target,
            src,
            poster,
            embedded,
        } => {
            // The resolved video source (a player URL when embedded).
            s.push_str("{\"name\":\"video\",\"target\":");
            escape_into(target, s);
            s.push_str(",\"src\":");
            escape_into(src, s);
            s.push_str(",\"poster\":");
            match poster {
                Some(poster) => escape_into(poster, s),
                None => s.push_str("null"),
            }
            s.push_str(&format!(",\"embedded\":{embedded},"));
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            s.push('}');
        }
        BlockKind::Audio { target, src } => {
            s.push_str("{\"name\":\"audio\",\"target\":");
            escape_into(target, s);
            s.push_str(",\"src\":");
            escape_into(src, s);
            s.push(',');
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
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
            write_caption(&block.caption, s);
            s.push_str(",\"items\":[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_list_item(item, s);
            }
            s.push_str("]}");
        }
        BlockKind::CalloutList { items } => {
            s.push_str("{\"name\":\"colist\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
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
                    escape_into(id, s);
                }
                s.push(']');
                write_icon(&entry.icons, &entry.src, s);
                s.push('}');
            }
            s.push_str("]}");
        }
        BlockKind::DescriptionList { items } => {
            s.push_str("{\"name\":\"dlist\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            s.push_str(",\"items\":[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str("{\"terms\":[");
                for (j, term) in item.terms.iter().enumerate() {
                    if j > 0 {
                        s.push(',');
                    }
                    write_inlines(&term.inlines, s);
                }
                s.push_str("],\"description\":");
                match &item.description {
                    Some(description) => write_list_item(description, s),
                    None => s.push_str("null"),
                }
                s.push('}');
            }
            s.push_str("]}");
        }
        BlockKind::Admonition {
            kind,
            label,
            blocks,
        } => {
            s.push_str(&format!(
                "{{\"name\":\"admonition\",\"admonition-type\":\"{}\",\"label\":",
                kind.as_str()
            ));
            escape_into(&adoctra_core::attributes::parts_text(label), s);
            s.push(',');
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            write_simple(block.simple, s);
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
            write_caption(&block.caption, s);
            write_simple(block.simple, s);
            s.push_str(",\"blocks\":[");
            for (i, child) in blocks.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push_str("]}");
        }
        BlockKind::Sidebar { blocks } => {
            s.push_str("{\"name\":\"sidebar\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            write_simple(block.simple, s);
            s.push_str(",\"blocks\":[");
            for (i, child) in blocks.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push_str("]}");
        }
        BlockKind::Section { level, blocks } => {
            // The section's id, roles and attributes live on its title, `blocks[0]`.
            s.push_str(&format!(
                "{{\"name\":\"section\",\"level\":{level},\"blocks\":["
            ));
            for (i, child) in blocks.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push_str("]}");
        }
        BlockKind::Preamble { blocks } => {
            // A book's preamble carries the header's `preface-title` source.
            s.push_str("{\"name\":\"preamble\"");
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
        BlockKind::Open { blocks } => {
            s.push_str("{\"name\":\"open\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            write_simple(block.simple, s);
            s.push_str(",\"blocks\":[");
            for (i, child) in blocks.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push_str("]}");
        }
        BlockKind::Quote {
            attribution,
            blocks,
        } => {
            s.push_str("{\"name\":\"quote\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            write_attribution(attribution, s);
            write_simple(block.simple, s);
            s.push_str(",\"blocks\":[");
            for (i, child) in blocks.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_block(child, s);
            }
            s.push_str("]}");
        }
        BlockKind::Verse {
            attribution,
            inlines,
        } => {
            s.push_str("{\"name\":\"verse\",");
            write_id(&block.id, s);
            write_roles(&block.roles, s);
            write_style(&block.attrs.style, s);
            write_attributes(&block.attrs.named, s);
            write_options(&block.attrs.options, s);
            write_title(&block.title, s);
            write_caption(&block.caption, s);
            write_attribution(attribution, s);
            s.push_str(",\"inlines\":");
            write_inlines(inlines, s);
            s.push('}');
        }
    }
}

/// Emit the leading-comma `,"attribution":{…}|null` field of a quote/verse block.
/// Each part is the inline array (so formatting is preserved) or `null`; an absent
/// attribution is the JSON `null`. A non-empty `trailing` follows the citetitle.
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
/// The `,"inlines":[…]` content of a verbatim or raw block converted with
/// custom substitutions; absent with the defaults.
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
/// caption prefix (e.g. `"Example 1. "` on a numbered example), or `null` when the
/// block has none (every block but a titled example block).
fn write_caption(caption: &Option<String>, s: &mut String) {
    s.push_str(",\"caption\":");
    match caption {
        Some(c) => escape_into(c, s),
        None => s.push_str("null"),
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
        match inline {
            Inline::Text { value, .. } => {
                s.push_str("{\"name\":\"text\",\"value\":");
                escape_into(value, s);
                s.push('}');
            }
            Inline::Substituted { value, parts, .. } => {
                s.push_str("{\"name\":\"substituted\",\"value\":");
                escape_into(value, s);
                adoctra_core::json::write_attribute_parts(parts, s);
                s.push('}');
            }
            Inline::Raw { value, .. } => {
                s.push_str("{\"name\":\"raw\",\"value\":");
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
            Inline::Image { image, .. } => {
                s.push_str("{\"name\":\"image\",");
                adoctra_core::json::write_inline_image(image, s);
                s.push('}');
            }
            Inline::Ui { ui, .. } => {
                s.push('{');
                adoctra_core::json::write_ui(ui, "name", s);
                s.push('}');
            }
            Inline::Stem { stem, .. } => {
                s.push('{');
                adoctra_core::json::write_stem(stem, "name", s);
                s.push('}');
            }
            Inline::IndexTerm { term, .. } => {
                s.push('{');
                adoctra_core::json::write_index_term(term, "name", s);
                s.push('}');
            }
            Inline::Footnote { footnote, .. } => {
                s.push_str("{\"name\":\"footnote\",");
                adoctra_core::json::write_footnote(footnote, s);
                s.push('}');
            }
            Inline::CrossReference { xref, children, .. } => {
                s.push_str("{\"name\":\"xref\",\"target\":");
                escape_into(&xref.target, s);
                for (key, value) in [
                    ("refid", &xref.refid),
                    ("path", &xref.path),
                    ("role", &xref.role),
                ] {
                    if let Some(value) = value {
                        s.push_str(&format!(",\"{key}\":"));
                        escape_into(value, s);
                    }
                }
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
            Inline::AttributeRef {
                name, value, parts, ..
            } => {
                s.push_str("{\"name\":\"text\",\"value\":");
                match value {
                    Some(v) => escape_into(v, s),
                    None => escape_into(&format!("{{{name}}}"), s),
                }
                adoctra_core::json::write_attribute_parts(parts, s);
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
    fn orphan_container_entries_do_not_reach_conversion() {
        // JSON projects resolved text: the dropped entry leaves `{x}`.
        let json = render("====\n:x: one\n====\n\n:y: two\n\n{x}{y}");
        assert!(
            json.contains(r#"[{"name":"text","value":"{x}"},{"name":"text","value":"two"}]"#),
            "{json}"
        );
    }

    #[test]
    fn builtin_backend_and_doctype_attributes_resolve() {
        let json = render("= T\n:doctype: book\n\nifdef::backend-html5-doctype-book[{backend}/{doctype}]\nifdef::doctype-article[article]\n");
        assert!(
            json.contains(r#"[{"name":"text","value":"html5"},{"name":"text","value":"/"},{"name":"text","value":"book"}]"#),
            "{json}"
        );
        assert!(!json.contains("article"), "{json}");
    }

    #[test]
    fn section_ids_follow_the_id_policy() {
        let json =
            render(":sectids!:\n\n== A\n\n:sectids:\n:idprefix:\n:idseparator: -\n\n== B C\n");
        assert!(
            json.contains(r#"{"name":"heading","level":1,"id":null,"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"name":"heading","level":1,"id":"b-c","#),
            "{json}"
        );
    }

    #[test]
    fn manual_pages_export_names_and_the_name_section() {
        let json = render("= x(1)\n:doctype: manpage\n\n== NAME\n\na, b - p\n");
        assert!(json.ends_with(r#","manpage":{"names":["a","b"],"name_section":{"heading":{"name":"heading","level":1,"id":"_name","roles":[],"style":null,"attributes":{},"options":[],"title":null,"caption":null,"inlines":[{"name":"text","value":"NAME"}]},"conforming":true}}}"#), "{json}");
        let json = render("= x(1)\n:doctype: manpage\n\n== NAME\n\na b\n");
        assert!(json.contains(r#""manpage":{"names":["command"],"name_section":{"heading":{"name":"heading","level":1,"id":"_name","#), "{json}");
        assert!(json.ends_with(r#""conforming":false}}}"#), "{json}");
        let json = render("= x(1)\n:doctype: manpage\n:manname: a\n:manpurpose: p\n");
        assert!(
            json.ends_with(r#","manpage":{"names":["a"],"name_section":null}}"#),
            "{json}"
        );
        assert!(!render("= x(1)\n\n== NAME\n\na - p\n").contains("manpage"));
    }

    #[test]
    fn bibliography_exports_list_style_and_neutral_anchor_parts() {
        let json = render("[bibliography]\n* [[[book,Label]]] Text\n\n<<book>>");
        assert!(json.contains(r#""style":"bibliography""#), "{json}");
        assert!(
            json.contains(
                r#""parts":[{"kind":"anchor","id":"book"},{"kind":"text","value":"[Label]"}]"#
            ),
            "{json}"
        );
        assert!(json.contains(r##""target":"#book""##), "{json}");
    }

    #[test]
    fn emits_json() {
        let json = render("= Title");
        assert!(json.starts_with("{\"name\":\"document\""));
        assert!(json.contains("\"heading\""));
    }

    #[test]
    fn indexed_authors_export_effective_text_and_original_declarations() {
        let json = render("= Doc\nJohn Doe; Mary Roe\n:author_2: Alice Brown\n\n{authors}");
        assert!(json.contains(r#""name":"Mary Roe""#), "{json}");
        assert!(
            json.contains(r#""value":"John Doe, Alice Brown""#),
            "{json}"
        );
    }

    #[test]
    fn author_markup_exports_raw_and_literal_parts_without_rewriting_declarations() {
        let json = render("= Doc\nJohn Doe\n:name: pass:[<b>Анна</b>]\n:author: {name} Смирнова\n\n{author}\n\n{firstname}");
        assert!(json.contains(r#""name":"John Doe""#), "{json}");
        assert!(json.contains(r#""value":"Анна""#), "{json}");
        assert!(json.contains(r#""parts":[{"kind":"raw","value":"<b>Анна</b>"},{"kind":"text","value":" Смирнова"}]"#), "{json}");
    }

    #[test]
    fn block_reftext_exports_evaluated_values_and_literal_macros() {
        let json = render("[[here,R {counter:n} link:p[x]]]\nBody {counter:n}\n\n<<here>>");
        assert!(
            json.contains(r##""name":"xref","target":"#here","refid":"here""##),
            "{json}"
        );
        assert!(json.contains(r#""name":"text","value":"1""#), "{json}");
        assert!(json.contains("link:p[x]"), "{json}");
        assert!(!json.contains("{counter:n}"), "{json}");
    }

    #[test]
    fn stateful_block_titles_export_captions_and_neutral_counter_parts() {
        let json = render(".T {counter:example-number}\n====\nBody {counter:example-number}\n====");
        assert!(json.contains(r#""caption":"Example 1. ""#), "{json}");
        assert!(
            json.contains(r#""title":[{"name":"text","value":"T "}"#),
            "{json}"
        );
        assert!(json.contains(r#""kind":"raw","value":"2""#), "{json}");
        assert!(json.contains(r#""kind":"raw","value":"3""#), "{json}");
        assert!(!json.contains("{counter:example-number}"), "{json}");
    }

    #[test]
    fn stateful_headings_export_resolved_counters_and_original_header() {
        let json = render("= H {counter:n}\n\n== S {counter:n}\n\n{counter:n}");
        assert!(json.contains(r#""title":"H {counter:n}""#), "{json}");
        assert!(json.contains(r#""id":"_s_2""#), "{json}");
        assert!(json.contains(r#""kind":"raw","value":"2""#), "{json}");
        assert!(json.contains(r#""kind":"raw","value":"3""#), "{json}");
        assert!(!json.contains('\u{96}'));
    }

    #[test]
    fn stateful_body_exports_evaluated_values_and_raw_counter_parts() {
        let json = render("{counter:n} {set:x:yes}{x}\n\n{counter:n}");
        assert!(json.contains(r#""value":"yes""#), "{json}");
        assert!(json.contains(r#""kind":"raw","value":"2""#), "{json}");
        assert!(!json.contains("{counter:n}"), "{json}");
        assert!(!json.contains('\u{96}'));
    }

    #[test]
    fn stateful_definitions_export_replayed_values_and_neutral_link_parts() {
        let json =
            render(":v: pass:n[link:pass:a[{counter:n}][]]\n\n{v}\n\n:w: {counter:n}\n\n{w} {n}");
        assert!(json.contains(r#""value":"3""#), "{json}");
        assert!(
            json.contains(r#""kind":"open-link","target":"1","role":"bare""#),
            "{json}"
        );
        assert!(json.contains(r#""kind":"raw","value":"2""#), "{json}");
        assert!(!json.contains('\u{96}'));
    }

    #[test]
    fn repeated_link_normal_exports_raw_text_and_neutral_break_parts() {
        let json = render(":v: pass:[+*bold*+]\n:r: pass:[+++<b>raw</b>+++]\n\nlink:https://e['{v}',role=x] link:https://e['{r}',role=x] link:https://e['end +',role=x]");
        assert!(
            json.contains(r#"{"kind":"text","value":"*bold*"}"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"kind":"raw","value":"<b>raw</b>"}"#),
            "{json}"
        );
        assert!(json.contains(r#"{"kind":"line-break"}"#), "{json}");
        assert!(!json.contains('\u{96}'));
    }

    #[test]
    fn metadata_missing_policies_export_semantic_link_parts() {
        let json = render(":attribute-missing: drop\n\nBefore [.{missing}]*Text* link:https://e['\\{missing}',role=x] link:https://e[Label,title='\\{missing}']");
        assert!(json.contains(r#""name":"strong""#), "{json}");
        assert!(
            json.contains(r#""kind":"open-link","target":"https://e","role":"bare x""#),
            "{json}"
        );
        assert!(json.contains(r#""title":"{missing}""#), "{json}");
    }

    #[test]
    fn ordinary_post_replacements_export_neutral_breaks_for_both_owners() {
        let json = render(":v: one + \\\n two\n\n{v} +\nlast");
        assert_eq!(json.matches(r#""kind":"line-break""#).count(), 2, "{json}");
        assert!(json.contains(r#""name":"substituted""#), "{json}");
        assert!(json.contains(r#""value":"one two""#), "{json}");
    }

    #[test]
    fn shared_attribute_stage_exports_raw_and_protected_intrinsic_values() {
        let json =
            render(":raw: {cpp}\n\n{raw} / pass:a,c[{cpp}] / pass:a,q[{asterisk}Bold{asterisk}]");
        assert!(
            json.contains(
                r#""value":"C&#43;&#43;","parts":[{"kind":"raw","value":"C&#43;&#43;"}]"#
            ),
            "{json}"
        );
        assert!(
            json.contains(r#""kind":"text","value":"C&#43;&#43;"}"#),
            "{json}"
        );
        assert!(
            json.contains(r#""kind":"open","format":"strong""#),
            "{json}"
        );
        let json = render(":attribute-missing: drop-line\n:v: {missing}\n\n{v}");
        assert!(json.contains(r#""name":"text","value":"""#), "{json}");
    }

    #[test]
    fn ordinary_attributes_export_intrinsic_parts_escapes_and_missing_values() {
        let json = render(r"{cpp} \{cpp} {missing}");
        assert!(json.contains(r#""name":"text","value":"C&#43;&#43;","parts":[{"kind":"raw","value":"C&#43;&#43;"}]"#), "{json}");
        assert!(json.contains("{cpp}"), "{json}");
        assert!(
            json.contains(r#""name":"text","value":"{missing}""#),
            "{json}"
        );
        let json = render(":attribute-missing: drop-line\n\nBefore {missing} after");
        assert!(json.contains(r#""name":"text","value":"""#), "{json}");
        assert!(!json.contains("Before"), "{json}");
    }

    #[test]
    fn pass_groups_export_neutral_line_breaks_and_protected_nested_text() {
        let json = render("Before pass:n[*Ж* +\n+*Literal* {v}+] after");
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
    fn inline_passthrough_exports_literal_raw_and_neutral_formatting_parts() {
        let json = render(
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
    fn source_quote_metadata_and_typographic_text_export_neutral_semantics() {
        let json = render(":role: red\n\nA [{role}]\"`Жёлтый`\" \\*Literal* _Italic_");
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
        let json = render(":target: page?a=1&b=2\n\nlink:{target}[*Ж&*,role=red,title=Title]");
        assert!(json.contains(r#""name":"substituted","value":"Ж&","parts":[{"kind":"open-link","target":"page?a=1&amp;b=2","role":"red","title":"Title"},{"kind":"open","format":"strong"},{"kind":"text","value":"Ж&"},{"kind":"close","format":"strong"},{"kind":"close-link"}]"#), "{json}");
        assert!(json.contains(r#""name":"text","value":"""#), "{json}");
        assert!(!json.contains("<a href"), "{json}");
    }

    #[test]
    fn post_attribute_macros_export_neutral_parts_without_generated_markup() {
        let json = render(":s: ttps\n\nh{s}://example.org");
        assert!(json.contains(r#""name":"substituted","value":"https://example.org","parts":[{"kind":"open-link","target":"https://example.org","role":"bare"},{"kind":"text","value":"https://example.org"},{"kind":"close-link"}]"#), "{json}");
        assert!(!json.contains("<a href"), "{json}");
        let json = render(":v: link:page[Ж&,role=red]\n\n{v}");
        assert!(json.contains(r#""value":"Ж&","parts":[{"kind":"open-link","target":"page","role":"red"},{"kind":"text","value":"Ж&"},{"kind":"close-link"}]"#), "{json}");
    }

    #[test]
    fn attribute_links_export_neutral_targets_properties_and_label_parts() {
        let json = render(":v: pass:c,q,m[link:page[*Ж&*,id=anchor,role=red,title=Title,window=_blank,opts=nofollow]]\n\n{v}");
        assert!(json.contains(r#""value":"Ж&","parts":[{"kind":"open-link","target":"page","id":"anchor","role":"red","title":"Title","window":"_blank","rel":"nofollow noopener"},{"kind":"open","format":"strong"},{"kind":"text","value":"Ж&"},{"kind":"close","format":"strong"},{"kind":"close-link"}]"#), "{json}");
        assert!(!json.contains("<a href"), "{json}");
        let json = render(":v: pass:m[mailto:a@example.org[Mail,Subject,Body]]\n\n{v}");
        assert!(
            json.contains(r#""target":"mailto:a@example.org?subject=Subject&amp;body=Body""#),
            "{json}"
        );
        let json = render(":v: pass:m[link:page[Alpha]]\n\n<<_alpha>>\n\n== {v}");
        assert_eq!(
            json.matches(r#""kind":"open-link","target":"page""#)
                .count(),
            1
        );
    }

    #[test]
    fn quote_attributes_export_neutral_metadata_and_preserve_empty_roles() {
        let source = ":v: pass:c,q[[#якорь.роль]*Ж&* [..]#span#]\n\n{v}";
        let json = render(source);
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
    fn pass_quotes_export_semantic_boundaries_without_backend_tags() {
        let json = render(":v: pass:c,q[*Ж&* _é_]\n\n{v}");
        assert!(json.contains(r#""value":"Ж& é","parts":[{"kind":"open","format":"strong"},{"kind":"text","value":"Ж&"},{"kind":"close","format":"strong"},{"kind":"text","value":" "},{"kind":"open","format":"emphasis"},{"kind":"text","value":"é"},{"kind":"close","format":"emphasis"}]"#), "{json}");
        for (marker, kind) in [
            ("`", "monospace"),
            ("#", "mark"),
            ("^", "superscript"),
            ("~", "subscript"),
        ] {
            let json = render(&format!(":v: pass:q[{marker}x{marker}]\n\n{{v}}"));
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
    fn pass_replacements_export_raw_entities_and_literal_parts() {
        let json = render(":v: pass:c,r[<b>(C)</b>]\n\n{v}");
        assert!(json.contains(r#""value":"<b>&#169;</b>","parts":[{"kind":"text","value":"<b>"},{"kind":"raw","value":"&#169;"},{"kind":"text","value":"</b>"}]"#), "{json}");
    }

    #[test]
    fn section_ids_are_sanitized_without_changing_raw_titles_or_reference_targets() {
        let json = render(":v: pass:[<b>Alpha</b> &copy;]\n\n== {v}\n\n<<_alpha>>");
        assert!(json.contains(r#""id":"_alpha""#), "{json}");
        assert!(
            json.contains(r##""target":"#_alpha","refid":"_alpha""##),
            "{json}"
        );
        assert_eq!(
            json.matches(r#""value":"<b>Alpha</b> &copy;""#).count(),
            4,
            "{json}"
        );
    }

    #[test]
    fn shared_replacements_export_raw_source_entities_and_consumed_references() {
        let json = render(":v: C\n:name: copy\n\n({v}) &{name}; \\&copy;");
        assert!(json.contains(r#"{"name":"text","value":"©"}"#), "{json}");
        assert!(
            json.contains(r#"{"name":"raw","value":"&copy;"}"#),
            "{json}"
        );
        assert_eq!(json.matches(r#"{"name":"text","value":""}"#).count(), 2);
        assert!(json.contains(r#""value":" &copy;""#), "{json}");
    }

    #[test]
    fn restored_attribute_entities_export_raw_syntax_and_literal_escapes() {
        let json = render(":v: &copy;\n:e: \\&copy;\n\n{v}\n\n{e}");
        assert!(
            json.contains(r#""value":"&copy;","parts":[{"kind":"raw","value":"&copy;"}]"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"name":"text","value":"&copy;"}"#),
            "{json}"
        );
        assert_eq!(json.matches(r#""parts":"#).count(), 1);
    }

    #[test]
    fn doctitle_override_exports_effective_heading_and_source_declaration() {
        let json = render("= Original\n:doctitle: *Effective*\n\n{doctitle}");
        assert!(json.contains(r#""title":"Original""#), "{json}");
        assert!(json.contains(r#""value":"Effective""#), "{json}");
        assert_eq!(json.matches(r#""name":"heading""#).count(), 1);
        let json = render(":doctitle: Only\n\nBody");
        assert!(!json.contains(r#""header":"#), "{json}");
        assert!(json.contains(r#""level":0"#), "{json}");
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
        assert!(
            json.contains("\"target\":\"#_section_two\",\"refid\":\"_section_two\""),
            "got: {json}"
        );
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
    fn link_serializes_with_target_and_semantic_label() {
        let json = render("See link:https://example.org[the site].");
        assert!(json.contains("\"name\":\"substituted\""), "got: {json}");
        assert!(
            json.contains("\"target\":\"https://example.org\""),
            "got: {json}"
        );
        assert!(json.contains("\"kind\":\"open-link\""), "got: {json}");
        assert!(
            json.contains("{\"kind\":\"text\",\"value\":\"the site\"}"),
            "got: {json}"
        );
    }

    #[test]
    fn bare_autolink_sets_bare_role() {
        let json = render("Bare https://example.org now.");
        assert!(json.contains("\"name\":\"substituted\""), "got: {json}");
        assert!(json.contains("\"role\":\"bare\""), "got: {json}");
        assert!(
            json.contains("\"target\":\"https://example.org\""),
            "got: {json}"
        );
    }

    #[test]
    fn xref_serializes_with_target_and_inlines() {
        let json = render("See <<sec,Section Two>>.");
        assert!(json.contains("\"name\":\"xref\""), "got: {json}");
        assert!(
            json.contains("\"target\":\"#sec\",\"refid\":\"sec\""),
            "got: {json}"
        );
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"Section Two\"}"),
            "got: {json}"
        );
    }

    #[test]
    fn xrefstyle_label_serializes_generated_text_and_title() {
        let json = render(":xrefstyle: full\n\n[[ex]]\n.T\n====\nx\n====\n\n<<ex>>");
        assert!(
            json.contains("{\"name\":\"xref\",\"target\":\"#ex\",\"refid\":\"ex\",\"inlines\":[{\"name\":\"text\",\"value\":\"Example\"},{\"name\":\"text\",\"value\":\" 1\"},{\"name\":\"text\",\"value\":\", \u{201c}\"},{\"name\":\"text\",\"value\":\"T\"},{\"name\":\"text\",\"value\":\"\u{201d}\"}]}"),
            "got: {json}"
        );
        let json = render("[[ex]]\n.T\n====\nx\n====\n\n:m: xref:ex[xrefstyle=short]\n\n{m}");
        assert!(
            json.contains(r##"{"kind":"open-xref","target":"#ex","refid":"ex","fragment":"ex","xrefstyle":"short"},{"kind":"text","value":"Example 1"},{"kind":"close-xref"}"##),
            "got: {json}"
        );
    }

    #[test]
    fn passthrough_xref_serializes_as_open_xref_parts() {
        let json = render("[[sec]]\n== T\n\npass:c,m[<<sec>>]");
        assert!(
            json.contains(r##"{"name":"substituted","value":"T","parts":[{"kind":"open-xref","target":"#sec","refid":"sec","fragment":"sec"},{"kind":"text","value":"T"},{"kind":"close-xref"}]}"##),
            "got: {json}"
        );
    }

    #[test]
    fn definition_xref_serializes_the_label_fixed_at_the_definition() {
        let json = render(":r: pass:c,m[<<sec>>]\n\n[[sec]]\n== T\n\n{r}");
        assert!(
            json.contains(r##""parts":[{"kind":"open-xref","target":"#sec","refid":"sec","fragment":"sec"},{"kind":"text","value":"[sec]"},{"kind":"close-xref"}]"##),
            "got: {json}"
        );
    }

    #[test]
    fn numbered_section_headings_serialize_their_caption() {
        let json = render(":sectnums:\n\n== A\n\n=== B");
        assert!(
            json.contains(r#""name":"heading","level":1,"id":"_a","roles":[],"style":null,"attributes":{},"options":[],"title":null,"caption":"1. ""#),
            "got: {json}"
        );
        assert!(json.contains(r#""caption":"1.1. ""#), "got: {json}");
        let json = render("== A");
        assert!(
            json.contains(r#""title":null,"caption":null,"inlines""#),
            "got: {json}"
        );
    }

    #[test]
    fn appendix_headings_serialize_their_caption_and_level() {
        let json = render("= Doc\n:appendix-caption: Приложение\n\n[appendix]\n= A");
        assert!(
            json.contains(r#""name":"heading","level":1,"id":"_a","roles":[],"style":"appendix","attributes":{},"options":[],"title":null,"caption":"Приложение A: ""#),
            "got: {json}"
        );
    }

    #[test]
    fn interdocument_xref_serializes_target_refid_and_path() {
        let json = render("<<other.adoc#f>> and <<Sec Title>>\n\n[[sec]]\n== Sec Title");
        assert!(
            json.contains(r##"{"name":"xref","target":"other.html#f","refid":"other#f","path":"other.html","inlines":[{"name":"text","value":"other.html"}]}"##),
            "got: {json}"
        );
        // A natural reference is retargeted to the id its title names.
        assert!(
            json.contains(r##"{"name":"xref","target":"#sec","refid":"sec","inlines":[{"name":"text","value":"Sec Title"}]}"##),
            "got: {json}"
        );
        let json = render(":p: xref:o.adoc[]\n\n{p}");
        assert!(
            json.contains(r##""parts":[{"kind":"open-xref","target":"o.html","refid":"o","path":"o.html"},{"kind":"text","value":"o.html"},{"kind":"close-xref"}]"##),
            "got: {json}"
        );
    }

    #[test]
    fn textless_xref_serializes_bracketed_id() {
        let json = render("See <<sec>>.");
        assert!(json.contains("\"name\":\"xref\""), "got: {json}");
        assert!(
            json.contains("\"target\":\"#sec\",\"refid\":\"sec\""),
            "got: {json}"
        );
        assert!(
            json.contains("{\"name\":\"text\",\"value\":\"[sec]\"}"),
            "got: {json}"
        );
    }

    #[test]
    fn xref_macro_serializes_like_angle_form() {
        let json = render("See xref:sec[Mac].");
        assert!(json.contains("\"name\":\"xref\""), "got: {json}");
        assert!(
            json.contains("\"target\":\"#sec\",\"refid\":\"sec\""),
            "got: {json}"
        );
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
    fn delimited_passthrough_serializes_by_name() {
        let json = render("++++\n<div>{x} *b*</div>\n++++");
        assert!(json.contains("\"name\":\"pass\""), "got: {json}");
        // The raw text is carried verbatim (subs not applied); `<`/`>`/`&` are
        // JSON-string-escaped only, not HTML-escaped.
        assert!(
            json.contains("\"text\":\"<div>{x} *b*</div>\""),
            "got: {json}"
        );
    }

    #[test]
    fn pass_style_paragraph_serializes_by_name() {
        let json = render("[pass]\n<b>{x}</b> *c*");
        assert!(json.contains("\"name\":\"pass\""), "got: {json}");
        assert!(json.contains("\"style\":\"pass\""), "got: {json}");
        assert!(json.contains("\"text\":\"<b>{x}</b> *c*\""), "got: {json}");
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
    fn source_defaults_and_adjusted_verbatim_text_serialize() {
        let json = render(
            ":source-language: ruby\n:source-linenums-option:\n:source-indent: 1\n:tabsize: 2\n\n----\n\tx\n----\n\n[listing]\nplain\n\n[verse,indent=0]\n  v",
        );
        assert!(
            json.contains(r#"{"name":"listing","id":null,"roles":[],"style":"source","attributes":{"indent":"1","language":"ruby","linenums":""},"options":[],"title":null,"caption":null,"text":" x"#),
            "{json}"
        );
        assert!(
            json.contains(r#""style":"listing","attributes":{},"options":[],"title":null,"caption":null,"text":"plain""#),
            "{json}"
        );
        assert!(json.contains(r#""name":"verse""#), "{json}");
        assert!(json.contains(r#"{"name":"text","value":"v"}"#), "{json}");
    }

    #[test]
    fn breaks_and_fenced_code_serialize() {
        let json = render("[.r]\n'''\n\n<<<\n\n```js,1\nx\n```");
        assert!(
            json.contains(r#"{"name":"break","variant":"thematic","id":null,"roles":["r"],"style":null,"attributes":{},"options":[],"title":null,"caption":null}"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"name":"break","variant":"page","#),
            "{json}"
        );
        assert!(
            json.contains(r#""style":"source","attributes":{"language":"js","linenums":""}"#),
            "{json}"
        );
    }

    #[test]
    fn markdown_and_quoted_paragraph_blockquotes_are_quotes() {
        let json = render("> a\n> -- *A*, W\n\n\"b\"\n-- B\n");
        assert!(
            json.contains(r#"{"name":"quote","id":null,"roles":[],"style":"quote","attributes":{},"options":[],"title":null,"caption":null,"attribution":{"author":[{"name":"strong","inlines":[{"name":"text","value":"A"}]}],"citetitle":[{"name":"text","value":"W"}]},"blocks":[{"name":"paragraph""#),
            "{json}"
        );
        assert!(
            json.contains(r#""attribution":{"author":[{"name":"text","value":"B"}],"citetitle":null},"simple":true,"blocks":[{"name":"paragraph""#),
            "{json}"
        );
    }

    #[test]
    fn credit_split_inside_formatting_exports_trailing() {
        let json = render("> a\n> -- *A, B* rest\n");
        assert!(
            json.contains(r#""attribution":{"author":[{"name":"strong","inlines":[{"name":"text","value":"A"}]}],"citetitle":[{"name":"strong","inlines":[{"name":"text","value":"B"}]}],"trailing":[{"name":"text","value":" rest"}]}"#),
            "{json}"
        );
        // Without a remainder there is no trailing field.
        assert!(!render("> a\n> -- A, B\n").contains("trailing"));
    }

    #[test]
    fn styled_paragraph_forms_mark_simple_content() {
        let json = render("[sidebar]\nx\n\n****\ny\n****\n");
        assert!(
            json.contains(r#""name":"sidebar","id":null,"roles":[],"style":"sidebar","attributes":{},"options":[],"title":null,"caption":null,"simple":true,"blocks":[{"name":"paragraph""#),
            "{json}"
        );
        assert_eq!(json.matches(r#""simple":true"#).count(), 1, "{json}");
        let json = render("NOTE: n\n\n[quote]\nq\n");
        assert_eq!(
            json.matches(r#""caption":null,"simple":true,"#).count(),
            1,
            "{json}"
        );
        assert!(
            json.contains(r#""caption":null,"attribution":null,"simple":true"#),
            "{json}"
        );
    }

    #[test]
    fn collapsible_example_keeps_options_and_an_empty_caption() {
        let json = render("[%collapsible%open]\n.T\n====\nx\n====\n");
        assert!(
            json.contains(r#""name":"example","id":null,"roles":[],"style":null,"attributes":{},"options":["collapsible","open"],"title":[{"name":"text","value":"T"}],"caption":"","blocks":["#),
            "{json}"
        );
    }

    #[test]
    fn longer_fences_serialize_nested_blocks_of_one_kind() {
        let json =
            render("======\nouter\n\n====\ninner\n====\n======\n\n------\ncode\n----\n------");
        assert!(
            json.contains(r#""name":"example","id":null,"roles":[],"style":null,"attributes":{},"options":[],"title":null,"caption":null,"blocks":[{"name":"paragraph""#),
            "{json}"
        );
        assert_eq!(json.matches(r#""name":"example""#).count(), 2, "{json}");
        assert!(json.contains(r#""text":"code\n----""#), "{json}");
    }

    #[test]
    fn callouts_and_callout_lists_serialize() {
        let json = render("----\nx // <1>\n----\n<1> one");
        assert!(json.contains(r#""text":"x // <1>","callouts":[{"number":"1","id":"CO1-1","guard":"// ","xml":false,"icons":null,"src":""}]"#), "{json}");
        assert!(json.contains(r#"{"name":"colist","id":null,"roles":[],"style":"arabic","attributes":{},"options":[],"title":null,"caption":null,"items":[{"inlines":[{"name":"text","value":"one"}],"blocks":[],"coids":["CO1-1"],"icons":null,"src":""}]}"#), "{json}");
        // Icons follow the `icons` mode where each block converts.
        let json =
            render("----\nx <1>\n----\n<1> a\n\n:icons:\n:iconsdir: i\n\n----\ny <1>\n----\n<1> b");
        assert!(json.contains(r#""text":"x <1>","callouts":[{"number":"1","id":"CO1-1","guard":null,"xml":false,"icons":null,"src":""}]"#), "{json}");
        assert!(json.contains(r#""text":"y <1>","callouts":[{"number":"1","id":"CO2-1","guard":null,"xml":false,"icons":"","src":"i/callouts/1.png"}]"#), "{json}");
        assert!(
            json.contains(r#""coids":["CO2-1"],"icons":"","src":"i/callouts/1.png"}"#),
            "{json}"
        );
    }

    #[test]
    fn block_subs_serialize_converted_content() {
        let json = render(":icons: font\n\n[subs=\"+quotes\"]\n----\n*x* // <1>\n----\n\n[subs=attributes]\n++++\n<u>{icons}</u>\n++++\n\n[subs=none]\n*p*");
        assert!(json.contains(r#""text":"*x* // <1>","inlines":[{"name":"substituted","value":"x ","parts":[{"kind":"open","format":"strong"},{"kind":"text","value":"x"},{"kind":"close","format":"strong"},{"kind":"text","value":" "},{"kind":"callout","number":"1","id":"CO1-1","guard":"// ","icons":"font","xml":false,"src":""}]}]}"#), "{json}");
        assert!(json.contains(r#""text":"<u>{icons}</u>","inlines":[{"name":"substituted","value":"<u>font</u>","parts":[{"kind":"raw","value":"<u>"},{"kind":"text","value":"font"},{"kind":"raw","value":"</u>"}]}]}"#), "{json}");
        assert!(json.contains(r#""inlines":[{"name":"substituted","value":"*p*","parts":[{"kind":"raw","value":"*p*"}]}]"#), "{json}");
        // Default substitutions keep the verbatim text only.
        let json = render("----\n*x*\n----");
        assert!(json.contains(r#""text":"*x*"}"#), "{json}");
    }

    #[test]
    fn description_lists_and_checklists_serialize() {
        let json = render("A::\nB:: *x*\nC::");
        assert!(json.contains(r#"{"name":"dlist","id":null,"roles":[],"style":null,"attributes":{},"options":[],"title":null,"caption":null,"items":[{"terms":[[{"name":"text","value":"A"}],[{"name":"text","value":"B"}]],"description":{"inlines":[{"name":"strong","inlines":[{"name":"text","value":"x"}]}],"blocks":[]}},{"terms":[[{"name":"text","value":"C"}]],"description":null}]}"#), "{json}");
        let json = render("* [x] a");
        assert!(json.contains(r#""options":["checklist"]"#), "{json}");
        assert!(
            json.contains(
                r#"{"inlines":[{"name":"text","value":"a"}],"checkbox":true,"blocks":[]}"#
            ),
            "{json}"
        );
    }

    #[test]
    fn stem_macros_serialize_notation_and_protected_parts() {
        let json = render(":s: pass:m[stem:[x]]\n\nA stem:[<x>] latexmath:[$y$] {s}");
        assert!(
            json.contains(
                r#"{"name":"stem","style":"asciimath","parts":[{"kind":"text","value":"<x>"}]}"#
            ),
            "{json}"
        );
        assert!(
            json.contains(
                r#"{"name":"stem","style":"latexmath","parts":[{"kind":"text","value":"y"}]}"#
            ),
            "{json}"
        );
        assert!(
            json.contains(
                r#"{"kind":"stem","style":"asciimath","parts":[{"kind":"text","value":"x"}]}"#
            ),
            "{json}"
        );
    }

    #[test]
    fn index_terms_serialize_visible_parts_and_concealed_metadata() {
        let json =
            render(":t: ((Attr))\n\n((*A*)) (((B,C >> D))) indexterm2:[E,see-also=\"F,G\"] {t}");
        assert!(json.contains(r#"{"name":"indexterm","parts":[{"kind":"open","format":"strong"},{"kind":"text","value":"A"},{"kind":"close","format":"strong"}],"terms":[],"see":null,"see_also":[]}"#), "{json}");
        assert!(
            json.contains(
                r#"{"name":"indexterm","parts":null,"terms":["B","C"],"see":"D","see_also":[]}"#
            ),
            "{json}"
        );
        assert!(json.contains(r#"{"name":"indexterm","parts":[{"kind":"text","value":"E"}],"terms":[],"see":null,"see_also":["F","G"]}"#), "{json}");
        assert!(json.contains(r#"{"kind":"indexterm","parts":[{"kind":"text","value":"Attr"}],"terms":[],"see":null,"see_also":[]}"#), "{json}");
    }

    #[test]
    fn ui_macros_serialize_keys_labels_and_menus() {
        let json = render(":experimental:\n:b: btn:[Go]\n\nkbd:[*A*+B] menu:F[S > T] menu:X[] {b}");
        assert!(json.contains(r#"{"name":"kbd","keys":[[{"kind":"open","format":"strong"},{"kind":"text","value":"A"},{"kind":"close","format":"strong"}],[{"kind":"text","value":"B"}]]}"#), "{json}");
        assert!(json.contains(r#"{"name":"menu","menu":[{"kind":"text","value":"F"}],"submenus":[[{"kind":"text","value":"S"}]],"item":[{"kind":"text","value":"T"}],"font":false}"#), "{json}");
        assert!(json.contains(r#"{"name":"menu","menu":[{"kind":"text","value":"X"}],"submenus":[],"item":null,"font":false}"#), "{json}");
        assert!(
            json.contains(r#""parts":[{"kind":"button","parts":[{"kind":"text","value":"Go"}]}]"#),
            "{json}"
        );
        // A nested macro is a part of its label.
        let json = render(":experimental:\n\nbtn:[link:x[y]]");
        assert!(json.contains(r#"{"name":"button","parts":[{"kind":"open-link","target":"x"},{"kind":"text","value":"y"},{"kind":"close-link"}]}"#), "{json}");
    }

    #[test]
    fn footnotes_serialize_markers_and_the_document_list() {
        let json = render("A footnote:x[*n*] B footnote:x[] C footnote:y[]");
        assert!(json.contains(r#"{"name":"footnote","type":"definition","index":"1","id":"x","parts":[{"kind":"open","format":"strong"},{"kind":"text","value":"n"},{"kind":"close","format":"strong"}]}"#), "{json}");
        assert!(
            json.contains(
                r#"{"name":"footnote","type":"reference","index":"1","id":null,"parts":[]}"#
            ),
            "{json}"
        );
        assert!(
            json.contains(
                r#"{"name":"footnote","type":"unresolved","index":null,"id":"y","parts":[]}"#
            ),
            "{json}"
        );
        assert!(json.contains(r#"],"footnotes":[{"index":"1","id":"x","parts":[{"kind":"open","format":"strong"},{"kind":"text","value":"n"},{"kind":"close","format":"strong"}]}]}"#), "{json}");
        let json = render(":fn: pass:m[footnote:[a]]\n\n{fn}");
        assert!(json.contains(r#"{"kind":"footnote","type":"definition","index":"1","id":null,"parts":[{"kind":"raw","value":"a"}]}"#), "{json}");
    }

    #[test]
    fn asciidoc_cells_serialize_their_blocks() {
        let json = render("|===\na|* x\n|===");
        assert!(
            json.contains(r#""style":"asciidoc","halign":"left","valign":"top","colspan":null,"rowspan":null,"background":null,"blocks":[{"name":"list","ordered":false,"#),
            "{json}"
        );
    }

    #[test]
    fn tables_serialize_columns_rows_and_cells() {
        let json = render(
            "[%footer,cols=\"1,1l\",frame=topbot]\n|===\n|H |I\n\n|a |  lit\n|{set:cellbgcolor:red}f |g\n|===",
        );
        assert!(json.contains(r#""name":"table","#), "{json}");
        assert!(
            json.contains(r#""width":100,"frame":"ends","grid":"all","stripes":null,"columns":[{"halign":"left","valign":"top","style":null,"width":"50","autowidth":false},{"halign":"left","valign":"top","style":"literal","width":"50","autowidth":false}]"#),
            "{json}"
        );
        assert!(
            json.contains(r#""head":[[{"style":null,"halign":"left","valign":"top","colspan":null,"rowspan":null,"background":null,"paragraphs":[[{"name":"text","value":"H"}]]},"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"style":"literal","halign":"left","valign":"top","colspan":null,"rowspan":null,"background":null,"literal":"  lit"}"#),
            "{json}"
        );
        assert!(json.contains(r#""foot":[[{"style":null"#), "{json}");
        assert!(
            json.contains(r#""background":"red","paragraphs":[[{"name":"text","value":""},{"name":"text","value":"f"}]]},{"style":"literal","halign":"left","valign":"top","colspan":null,"rowspan":null,"background":"red","literal":"g"}"#),
            "{json}"
        );
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

    #[test]
    fn titled_example_serializes_caption() {
        // A titled example carries its resolved counter prefix as a string caption.
        let json = render(".Demo\n====\nhi\n====");
        assert!(json.contains("\"caption\":\"Example 1. \""), "got: {json}");
    }

    #[test]
    fn titled_listing_serializes_listing_caption() {
        // Listings number only when the document sets `listing-caption`.
        let json = render(".A\n----\na\n----\n\n:listing-caption: Listing\n\n.B\n----\nb\n----");
        assert!(
            json.contains(r#""name":"listing","id":null"#),
            "got: {json}"
        );
        assert_eq!(json.matches(r#""caption":null"#).count(), 1, "got: {json}");
        assert!(json.contains(r#""caption":"Listing 1. ""#), "got: {json}");
    }

    #[test]
    fn untitled_block_serializes_null_caption() {
        // No title (or a non-example titled block) → no caption.
        let json = render("Body.");
        assert!(json.contains("\"caption\":null"), "got: {json}");
    }

    #[test]
    fn titled_listing_serializes_null_caption() {
        // Only example blocks are numbered; a titled listing has a title but no
        // caption.
        let json = render(".Code\n----\nputs 1\n----");
        assert!(json.contains("\"name\":\"listing\""), "got: {json}");
        assert!(json.contains("\"caption\":null"), "got: {json}");
    }

    // --- sidebar blocks ---

    #[test]
    fn sidebar_block_serializes_nested_blocks() {
        let json = render("****\nHi *there*.\n****");
        assert!(json.contains("\"name\":\"sidebar\""), "got: {json}");
        // The child paragraph (with its own inline markup) is nested under blocks.
        assert!(json.contains("\"blocks\":["), "got: {json}");
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");
    }

    #[test]
    fn sidebar_block_id_and_roles_serialize() {
        let json = render("[#sb.box]\n****\nhi\n****");
        assert!(json.contains("\"name\":\"sidebar\""), "got: {json}");
        assert!(json.contains("\"id\":\"sb\""), "got: {json}");
        assert!(json.contains("\"roles\":[\"box\"]"), "got: {json}");
    }

    // --- open blocks ---

    #[test]
    fn open_block_serializes_nested_blocks() {
        let json = render("--\nHi *there*.\n--");
        assert!(json.contains("\"name\":\"open\""), "got: {json}");
        // The child paragraph (with its own inline markup) is nested under blocks.
        assert!(json.contains("\"blocks\":["), "got: {json}");
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");
    }

    #[test]
    fn open_block_id_and_roles_serialize() {
        let json = render("[#o.box]\n--\nhi\n--");
        assert!(json.contains("\"name\":\"open\""), "got: {json}");
        assert!(json.contains("\"id\":\"o\""), "got: {json}");
        assert!(json.contains("\"roles\":[\"box\"]"), "got: {json}");
    }

    #[test]
    fn quote_style_on_open_serializes_as_quote() {
        // The masquerade resolves before serialization: a `[quote]` on `--` emits a
        // quote node, not an open node.
        let json = render("[quote]\n--\nx\n--");
        assert!(json.contains("\"name\":\"quote\""), "got: {json}");
        assert!(!json.contains("\"name\":\"open\""), "got: {json}");
    }

    // --- quote & verse blocks ---

    #[test]
    fn quote_block_serializes_attribution_and_blocks() {
        let json = render("[quote, Albert Einstein, Theory]\n____\nHi *there*.\n____");
        assert!(json.contains("\"name\":\"quote\""), "got: {json}");
        assert!(
            json.contains(
                "\"attribution\":{\"author\":[{\"name\":\"raw\",\"value\":\"Albert Einstein\"}],\"citetitle\":[{\"name\":\"raw\",\"value\":\"Theory\"}]}"
            ),
            "got: {json}"
        );
        assert!(json.contains("\"name\":\"paragraph\""), "got: {json}");
        assert!(json.contains("\"name\":\"strong\""), "got: {json}");
    }

    #[test]
    fn quote_without_attribution_serializes_null() {
        let json = render("____\nx\n____");
        assert!(json.contains("\"name\":\"quote\""), "got: {json}");
        assert!(json.contains("\"attribution\":null"), "got: {json}");
    }

    #[test]
    fn verse_block_serializes_attribution_and_inlines() {
        let json = render("[verse, Carl Sandburg, Fog]\n____\nfirst\nsecond\n____");
        assert!(json.contains("\"name\":\"verse\""), "got: {json}");
        assert!(
            json.contains(
                "\"attribution\":{\"author\":[{\"name\":\"raw\",\"value\":\"Carl Sandburg\"}]"
            ),
            "got: {json}"
        );
        // Verse keeps its line break — a JSON-escaped newline in the inline text.
        assert!(json.contains(r"first\nsecond"), "got: {json}");
    }

    #[test]
    fn verse_citetitle_only_serializes_null_author() {
        let json = render("[verse, , Fog]\n____\nx\n____");
        assert!(
            json.contains(
                "\"attribution\":{\"author\":null,\"citetitle\":[{\"name\":\"raw\",\"value\":\"Fog\"}]}"
            ),
            "got: {json}"
        );
    }

    #[test]
    fn attribution_follows_merged_ruby_keys() {
        // The second line replaces the author key and keeps the citetitle;
        // a quoted empty value is an empty inline array.
        let json = render("[quote, A, W]\n[quote, \"\"]\n____\nx\n____");
        assert!(
            json.contains(
                "\"attribution\":{\"author\":[],\"citetitle\":[{\"name\":\"raw\",\"value\":\"W\"}]}"
            ),
            "got: {json}"
        );
    }

    #[test]
    fn attribution_serializes_raw_source_and_references() {
        // JSON keeps its resolved-text projection of attribute references.
        let json = render(":a: X\n\n[quote, <i>{a}</i>, '*W*']\nx");
        assert!(
            json.contains(
                "\"author\":[{\"name\":\"raw\",\"value\":\"<i>\"},{\"name\":\"text\",\"value\":\"X\"},{\"name\":\"raw\",\"value\":\"</i>\"}]"
            ),
            "got: {json}"
        );
        assert!(
            json.contains("\"citetitle\":[{\"name\":\"strong\""),
            "got: {json}"
        );
    }

    #[test]
    fn list_title_serializes_like_a_dot_title() {
        let json = render("[#p,title=T]\npara");
        assert!(
            json.contains("\"title\":[{\"name\":\"text\",\"value\":\"T\"}]"),
            "got: {json}"
        );
        // The JSON projection has no reftext field; the id is still exposed.
        assert!(json.contains("\"id\":\"p\""), "got: {json}");
    }

    #[test]
    fn xref_labels_serialize_titles_and_reftext_formatted_per_reference() {
        let json = render("[[t]]\n.Ж *T*\n----\nx\n----\n\n[[b,[.r\\{x}]#R#]]\npara\n\n:x: 1\n\n<<t>> <<b>>\n\n:x: 2\n\n<<b>>");
        assert!(
            json.contains(r##"{"name":"xref","target":"#t","refid":"t","inlines":[{"name":"text","value":"Ж "},{"name":"strong","inlines":[{"name":"text","value":"T"}]}]}"##),
            "{json}"
        );
        assert!(json.contains(r#""role":"r1""#), "{json}");
        assert!(json.contains(r#""role":"r2""#), "{json}");
    }

    #[test]
    fn xrefs_use_cataloged_anchor_reftext() {
        let json = render("p\n\nA [[a,R {counter:n}]]\n\n* x [[b,L]]\n\n<<a>> <<b>>");
        assert!(
            json.contains("{\"name\":\"xref\",\"target\":\"#a\",\"refid\":\"a\",\"inlines\":[{\"name\":\"text\",\"value\":\"R \"},{\"name\":\"text\",\"value\":\"1\"}]}"),
            "got: {json}"
        );
        // A trailing list item anchor is rendered but not cataloged.
        assert!(
            json.contains("{\"name\":\"xref\",\"target\":\"#b\",\"refid\":\"b\",\"inlines\":[{\"name\":\"text\",\"value\":\"[b]\"}]}"),
            "got: {json}"
        );
    }

    #[test]
    fn admonition_serializes_its_resolved_label() {
        let json = render(":note-caption: Nota\n\nNOTE: x");
        assert!(
            json.contains("\"admonition-type\":\"note\",\"label\":\"Nota\""),
            "got: {json}"
        );
    }

    // --- verbatim masquerades on a `--` open block ---

    #[test]
    fn source_style_on_open_serializes_as_listing() {
        let json = render("[source,ruby]\n--\nputs *x*\n--");
        assert!(json.contains("\"name\":\"listing\""), "got: {json}");
        assert!(
            json.contains("\"style\":\"source\",\"attributes\":{\"language\":\"ruby\"}"),
            "got: {json}"
        );
        assert!(json.contains("\"text\":\"puts *x*\""), "got: {json}");
    }

    #[test]
    fn literal_style_on_open_serializes_as_literal() {
        let json = render("[literal]\n--\nverbatim *not bold*\n--");
        assert!(json.contains("\"name\":\"literal\""), "got: {json}");
        assert!(
            json.contains("\"text\":\"verbatim *not bold*\""),
            "got: {json}"
        );
    }

    #[test]
    fn verse_style_on_open_serializes_as_verse() {
        let json = render("[verse, Carl Sandburg, Fog]\n--\nfirst\nsecond\n--");
        assert!(json.contains("\"name\":\"verse\""), "got: {json}");
        assert!(
            json.contains(
                "\"attribution\":{\"author\":[{\"name\":\"raw\",\"value\":\"Carl Sandburg\"}]"
            ),
            "got: {json}"
        );
        // Verse keeps its line break — a JSON-escaped newline in the inline text.
        assert!(json.contains(r"first\nsecond"), "got: {json}");
    }

    #[test]
    fn late_anchors_serialize_as_nodes_or_attribute_parts() {
        let json = render(":id: foo\n:a: [[q]]x\n\nA [[{id}]] {a} \\[[e]]");
        assert!(json.contains(r#"{"name":"anchor","id":"foo"}"#), "{json}");
        assert!(
            json.contains(r#""parts":[{"kind":"anchor","id":"q"},{"kind":"text","value":"x"}]"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"name":"text","value":" [[e]]"}"#),
            "{json}"
        );
    }

    #[test]
    fn late_xrefs_serialize_as_nodes_or_attribute_parts() {
        let json = render(":r: <<s>>\n\n[[s]]\n== T\n\nA xref:s[x,role=r] {r}");
        assert!(
            json.contains(
                r##"{"name":"xref","target":"#s","refid":"s","role":"r","inlines":[{"name":"text","value":"x"}]}"##
            ),
            "{json}"
        );
        assert!(
            json.contains(r##""value":"T","parts":[{"kind":"open-xref","target":"#s","refid":"s","fragment":"s"},{"kind":"text","value":"T"},{"kind":"close-xref"}]"##),
            "{json}"
        );
        let json = render(":r: xref:s[x,role=r]\n\n{r}");
        assert!(
            json.contains(r##""parts":[{"kind":"open-xref","target":"#s","refid":"s","fragment":"s","role":"r"},{"kind":"text","value":"x"},{"kind":"close-xref"}]"##),
            "{json}"
        );
    }
}
