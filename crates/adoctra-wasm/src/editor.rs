//! The editor view: one compact JSON answer per edit.
//!
//! [`parse_json`](crate::parse_json) gives the whole tree with byte ranges; an
//! editor that reparses on every edit needs less and in its own units. This
//! view lists, for the edited source only:
//!
//! - `source_id` — the id of the edited source;
//! - `tokens` — `[start, end, kind]` highlight ranges;
//! - `blocks` — the block tree flattened in document order with a `depth`,
//!   including list items and table cells, so the host finds the block at an
//!   offset, draws a block gutter and folds without walking a tree;
//! - `outline` — the document title and the sections;
//! - `anchors` — the ids a cross-reference can name;
//! - `attributes` — the document attributes;
//! - `diagnostics`.
//!
//! Every offset is a **UTF-16 code unit** offset into the source text, the
//! unit of a JavaScript string. Nodes read from an included file are left
//! out (their include directive is a token); a diagnostic located in another
//! source keeps that source's id and its byte range there.
//!
//! The contract is `docs/editor.adoc`.

use adoctra_core::ast::{Block, BlockKind, CellContent, Inline, ListItem, Span};
use adoctra_core::diagnostic::ParseResult;
use adoctra_core::json::escape_into as write_str;
use adoctra_core::SourceId;

/// The version of the view's shape, raised on an incompatible change.
pub const EDITOR_VIEW_VERSION: u32 = 1;

/// Parse `src` and return its editor view.
pub fn editor_json(src: &str) -> String {
    editor_json_with_source(src, 0)
}

/// Like [`editor_json`], with the source id assigned by the host.
pub fn editor_json_with_source(src: &str, source_id: usize) -> String {
    let options = adoctra_core::ParseOptions {
        source_id: SourceId(source_id),
        ..Default::default()
    };
    let mut result = adoctra_core::parse_with_options(src, &options);
    serialize_editor_view(&mut result, src, SourceId(source_id))
}

/// The editor view of `result`, parsed from the `source` text of `root`.
///
/// The result is borrowed mutably only to walk it with the shared visitor;
/// it is not changed.
pub fn serialize_editor_view(result: &mut ParseResult, source: &str, root: SourceId) -> String {
    let view = View {
        offsets: Offsets::new(source),
        source,
        root,
    };
    let mut s = format!(
        "{{\"version\":{EDITOR_VIEW_VERSION},\"source_id\":{},\"tokens\":[",
        root.0
    );
    let mut first = true;
    for token in &result.tokens {
        let Some((start, end)) = view.range(token.span) else {
            continue;
        };
        comma(&mut first, &mut s);
        s.push_str(&format!("[{start},{end},\"{}\"]", token.kind.as_str()));
    }
    s.push_str("],\"blocks\":[");
    let mut outline = String::new();
    let mut anchors = Vec::new();
    if let Some(header) = &result.document.header {
        if let (Some((start, end)), Some(title)) =
            (view.range(header.title_span), view.slice(header.title_span))
        {
            outline.push_str(&format!("{{\"level\":0,\"start\":{start},\"end\":{end},"));
            outline.push_str("\"id\":null,\"title\":");
            write_str(title.trim_start_matches(['=', '#']).trim(), &mut outline);
            outline.push('}');
        }
    }
    view.write_blocks(&result.document.blocks, &mut s, &mut outline, &mut anchors);
    s.push_str("],\"outline\":[");
    s.push_str(&outline);
    s.push_str("],\"anchors\":[");
    view.inline_anchors(result, &mut anchors);
    anchors.sort();
    anchors.dedup();
    let mut first = true;
    for (id, start, end) in &anchors {
        comma(&mut first, &mut s);
        s.push_str("{\"id\":");
        write_str(id, &mut s);
        s.push_str(&format!(",\"start\":{start},\"end\":{end}}}"));
    }
    s.push_str("],\"attributes\":{");
    let mut first = true;
    for (name, value) in result.document.attributes.iter() {
        comma(&mut first, &mut s);
        write_str(name, &mut s);
        s.push(':');
        write_str(value, &mut s);
    }
    s.push_str("},\"diagnostics\":[");
    let mut first = true;
    for diagnostic in &result.diagnostics {
        comma(&mut first, &mut s);
        let span = diagnostic.span;
        let (start, end) = view.range(span).unwrap_or((span.start, span.end));
        s.push_str(&format!(
            "{{\"source_id\":{},\"start\":{start},\"end\":{end},\"severity\":\"{}\",\"message\":",
            span.source_id.0,
            diagnostic.severity.as_str()
        ));
        write_str(&diagnostic.message, &mut s);
        s.push('}');
    }
    s.push_str("]}");
    s
}

fn comma(first: &mut bool, s: &mut String) {
    if !std::mem::take(first) {
        s.push(',');
    }
}

/// Byte offset to UTF-16 offset in one source.
pub struct Utf16Offsets(Offsets);

impl Utf16Offsets {
    pub fn new(source: &str) -> Self {
        Utf16Offsets(Offsets::new(source))
    }

    /// The UTF-16 offset of the character holding `byte` (of the end of the
    /// source past it).
    pub fn utf16(&self, byte: usize) -> usize {
        self.0.utf16(byte)
    }
}

enum Offsets {
    /// ASCII only: the offsets are equal.
    Same(usize),
    /// The UTF-16 offset of the character holding each byte, and of the end.
    Table(Vec<u32>),
}

impl Offsets {
    fn new(source: &str) -> Self {
        if source.is_ascii() {
            return Offsets::Same(source.len());
        }
        let mut table = Vec::with_capacity(source.len() + 1);
        let mut units = 0u32;
        for ch in source.chars() {
            table.extend(std::iter::repeat_n(units, ch.len_utf8()));
            units += ch.len_utf16() as u32;
        }
        table.push(units);
        Offsets::Table(table)
    }

    fn utf16(&self, byte: usize) -> usize {
        match self {
            Offsets::Same(len) => byte.min(*len),
            Offsets::Table(table) => table[byte.min(table.len() - 1)] as usize,
        }
    }
}

/// A node of the flattened block tree still to write.
enum Node<'a> {
    Block(&'a Block),
    Item(&'a ListItem, &'static str),
    Cell(&'a adoctra_core::ast::TableCell),
}

struct View<'a> {
    offsets: Offsets,
    source: &'a str,
    root: SourceId,
}

impl View<'_> {
    /// The UTF-16 range of a span of the edited source.
    fn range(&self, span: Span) -> Option<(usize, usize)> {
        (span.source_id == self.root)
            .then(|| (self.offsets.utf16(span.start), self.offsets.utf16(span.end)))
    }

    fn slice(&self, span: Span) -> Option<&str> {
        (span.source_id == self.root)
            .then(|| self.source.get(span.start..span.end))
            .flatten()
    }

    /// The source text of a run of inline nodes.
    fn inline_source(&self, inlines: &[Inline]) -> &str {
        let (Some(first), Some(last)) = (inlines.first(), inlines.last()) else {
            return "";
        };
        let (first, last) = (first.span(), last.span());
        if first.source_id != self.root || last.source_id != self.root {
            return "";
        }
        self.source.get(first.start..last.end).unwrap_or("").trim()
    }

    /// The blocks in document order on an explicit stack (no recursion: a
    /// document may nest arbitrarily deep).
    fn write_blocks(
        &self,
        blocks: &[Block],
        s: &mut String,
        outline: &mut String,
        anchors: &mut Vec<(String, usize, usize)>,
    ) {
        let mut first = true;
        let mut stack: Vec<(Node<'_>, usize)> =
            blocks.iter().rev().map(|b| (Node::Block(b), 0)).collect();
        while let Some((node, depth)) = stack.pop() {
            let mut children = Vec::new();
            match node {
                Node::Block(block) => {
                    self.block_children(block, &mut children);
                    // A section's metadata lines and id are its heading's.
                    let heading = match &block.kind {
                        BlockKind::Section { blocks, .. } => blocks.first(),
                        _ => None,
                    };
                    let meta = heading.unwrap_or(block).meta_span;
                    let id = heading.unwrap_or(block).id.as_deref();
                    if let Some((start, end)) = self.range(block.span) {
                        comma(&mut first, s);
                        s.push_str(&format!(
                            "{{\"start\":{start},\"end\":{end},\"depth\":{depth},\"kind\":\"{}\"",
                            kind_name(block)
                        ));
                        if let Some((meta_start, _)) = meta.and_then(|meta| self.range(meta)) {
                            s.push_str(&format!(",\"meta_start\":{meta_start}"));
                        }
                        self.block_details(block, s);
                        s.push('}');
                        let anchor = heading.map_or(Some((start, end)), |h| self.range(h.span));
                        if let (Some(id), Some((start, end))) = (id, anchor) {
                            anchors.push((id.to_owned(), start, end));
                        }
                        if let (BlockKind::Section { level, .. }, Some(heading)) =
                            (&block.kind, heading)
                        {
                            if !outline.is_empty() {
                                outline.push(',');
                            }
                            outline.push_str(&format!(
                                "{{\"level\":{level},\"start\":{start},\"end\":{end},\"id\":"
                            ));
                            match id {
                                Some(id) => write_str(id, outline),
                                None => outline.push_str("null"),
                            }
                            outline.push_str(",\"title\":");
                            let title = match &heading.kind {
                                BlockKind::Heading { inlines, .. } => self.inline_source(inlines),
                                _ => "",
                            };
                            write_str(title, outline);
                            outline.push('}');
                        }
                    }
                }
                Node::Item(item, kind) => {
                    children.extend(item.blocks.iter().map(Node::Block));
                    if let Some((start, end)) = self.range(item.span) {
                        comma(&mut first, s);
                        s.push_str(&format!(
                            "{{\"start\":{start},\"end\":{end},\"depth\":{depth},\"kind\":\"{kind}\"}}"
                        ));
                    }
                }
                Node::Cell(cell) => {
                    if let CellContent::Blocks { blocks, .. } = &cell.content {
                        children.extend(blocks.iter().map(Node::Block));
                    }
                    if let Some((start, end)) = self.range(cell.span) {
                        comma(&mut first, s);
                        s.push_str(&format!(
                            "{{\"start\":{start},\"end\":{end},\"depth\":{depth},\"kind\":\"table-cell\"}}"
                        ));
                    }
                }
            }
            stack.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
        }
    }

    fn block_children<'b>(&self, block: &'b Block, children: &mut Vec<Node<'b>>) {
        match &block.kind {
            BlockKind::Section { blocks, .. }
            | BlockKind::Preamble { blocks }
            | BlockKind::Admonition { blocks, .. }
            | BlockKind::Example { blocks }
            | BlockKind::Quote { blocks, .. }
            | BlockKind::Sidebar { blocks }
            | BlockKind::Open { blocks } => children.extend(blocks.iter().map(Node::Block)),
            BlockKind::List { items, .. } => {
                children.extend(items.iter().map(|item| Node::Item(item, "list-item")));
            }
            BlockKind::CalloutList { items } => {
                children.extend(
                    items
                        .iter()
                        .map(|entry| Node::Item(&entry.item, "list-item")),
                );
            }
            BlockKind::DescriptionList { items } => children.extend(
                items
                    .iter()
                    .filter_map(|item| item.description.as_ref())
                    .map(|item| Node::Item(item, "description")),
            ),
            BlockKind::Table(table) => children.extend(
                table
                    .head
                    .iter()
                    .chain(&table.body)
                    .chain(&table.foot)
                    .flatten()
                    .map(Node::Cell),
            ),
            _ => {}
        }
    }

    /// What a block gutter shows beside the kind.
    fn block_details(&self, block: &Block, s: &mut String) {
        match &block.kind {
            BlockKind::Heading { level, .. } | BlockKind::Section { level, .. } => {
                s.push_str(&format!(",\"level\":{level}"));
            }
            BlockKind::List { ordered, .. } => s.push_str(&format!(",\"ordered\":{ordered}")),
            BlockKind::Admonition { kind, .. } => {
                s.push_str(&format!(",\"admonition\":\"{}\"", kind.as_str()));
            }
            _ => {}
        }
        if let Some(style) = &block.attrs.style {
            s.push_str(",\"style\":");
            write_str(style, s);
        }
        if let Some(language) = block.attrs.named.get("language") {
            s.push_str(",\"language\":");
            write_str(language, s);
        }
    }

    /// The inline anchors (`[[id]]` in text) of the edited source.
    fn inline_anchors(&self, result: &mut ParseResult, anchors: &mut Vec<(String, usize, usize)>) {
        struct Collect<'v, 'a>(&'v View<'a>, &'v mut Vec<(String, usize, usize)>);
        impl adoctra_core::visit::VisitorMut for Collect<'_, '_> {
            fn visit_inline(&mut self, inline: &mut Inline) {
                if let Inline::Anchor { id, span, .. } = inline {
                    if let Some((start, end)) = self.0.range(*span) {
                        self.1.push((id.clone(), start, end));
                    }
                }
                adoctra_core::visit::walk_inline_mut(self, inline);
            }
        }
        adoctra_core::visit::walk_document_mut(&mut Collect(self, anchors), &mut result.document);
    }
}

/// The block names of [`parse_json`](crate::parse_json).
fn kind_name(block: &Block) -> &'static str {
    match &block.kind {
        BlockKind::Heading { .. } => "heading",
        BlockKind::Paragraph { .. } => "paragraph",
        BlockKind::Listing { .. } => "listing",
        BlockKind::Literal { .. } => "literal",
        BlockKind::Passthrough { .. } => "pass",
        BlockKind::Stem { .. } => "stem",
        BlockKind::List { .. } => "list",
        BlockKind::CalloutList { .. } => "colist",
        BlockKind::DescriptionList { .. } => "dlist",
        BlockKind::Admonition { .. } => "admonition",
        BlockKind::Example { .. } => "example",
        BlockKind::Quote { .. } => "quote",
        BlockKind::Verse { .. } => "verse",
        BlockKind::Sidebar { .. } => "sidebar",
        BlockKind::Open { .. } => "open",
        BlockKind::ThematicBreak | BlockKind::PageBreak => "break",
        BlockKind::Toc => "toc",
        BlockKind::Image { .. } => "image",
        BlockKind::Video { .. } => "video",
        BlockKind::Audio { .. } => "audio",
        BlockKind::Table(_) => "table",
        BlockKind::Section { .. } => "section",
        BlockKind::Preamble { .. } => "preamble",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_tokens_blocks_and_diagnostics_in_utf16() {
        // «Я» is two bytes and one unit, the emoji four bytes and two units.
        let json = editor_json("Яя 😀 *b*\n\n----\nx");
        assert!(
            json.starts_with("{\"version\":1,\"source_id\":0,\"tokens\":["),
            "{json}"
        );
        assert!(json.contains("[6,9,\"strong\"]"), "{json}");
        assert!(
            json.contains(r#"{"start":0,"end":9,"depth":0,"kind":"paragraph"}"#),
            "{json}"
        );
        assert!(
            json.contains(r#""kind":"listing""#) && json.contains(r#""severity":"error""#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"source_id":0,"start":11,"end":15,"#),
            "{json}"
        );
    }

    #[test]
    fn a_block_starts_at_its_metadata_lines() {
        let json = editor_json("p\n\n[[a]]\n.T\n[source,rust]\n----\nfn\n----\n");
        assert!(
            json.contains(
                r#"{"start":26,"end":38,"depth":0,"kind":"listing","meta_start":3,"style":"source","language":"rust"}"#
            ),
            "{json}"
        );
        assert!(json.contains(r#"{"id":"a","start":26,"end":38}"#), "{json}");
    }

    #[test]
    fn flattens_sections_lists_and_cells_with_depth() {
        let src = "= Doc\n\n== One *x*\n\n* a\n** b\n\nNOTE: n\n\n|===\n|c\n|===\n";
        let json = editor_json(src);
        assert!(
            json.contains(r#"{"start":7,"end":50,"depth":0,"kind":"section","level":1}"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"start":7,"end":17,"depth":1,"kind":"heading","level":1}"#),
            "{json}"
        );
        for part in [
            r#""depth":1,"kind":"list","ordered":false"#,
            r#""depth":2,"kind":"list-item"}"#,
            r#""depth":3,"kind":"list","ordered":false"#,
            r#""depth":4,"kind":"list-item"}"#,
            r#""depth":1,"kind":"admonition","admonition":"note""#,
            r#""depth":2,"kind":"paragraph"}"#,
            r#""depth":1,"kind":"table""#,
            r#""depth":2,"kind":"table-cell"}"#,
        ] {
            assert!(json.contains(part), "{part} in {json}");
        }
        assert!(
            json.contains(
                r#""outline":[{"level":0,"start":0,"end":5,"id":null,"title":"Doc"},{"level":1,"start":7,"end":50,"id":"_one_x","title":"One *x*"}]"#
            ),
            "{json}"
        );
        assert!(
            json.contains(r#"{"id":"_one_x","start":7,"end":17}"#),
            "{json}"
        );
    }

    #[test]
    fn lists_inline_anchors_and_attributes() {
        let json = editor_json(":k: v\n\nText [[in]]here.\n");
        assert!(
            json.contains(r#"{"id":"in","start":12,"end":18}"#),
            "{json}"
        );
        assert!(json.contains(r#""k":"v""#), "{json}");
    }

    #[test]
    fn leaves_out_nodes_of_another_source() {
        let mut workspace = crate::ResourceWorkspace::new();
        let _ = workspace.set("main.adoc", "Ё\n\ninclude::part.adoc[]\n\nend\n");
        let _ = workspace.set("part.adoc", "== In\n\ninner *b*\n\n----\nopen\n");
        let id = workspace.load("main.adoc", None).unwrap();
        let json = workspace.editor_json(id).unwrap();
        assert!(json.contains("\"include-directive\"]"), "{json}");
        assert!(!json.contains("\"strong\""), "{json}");
        assert!(json.contains(r#""kind":"paragraph""#), "{json}");
        assert!(!json.contains(r#""kind":"listing""#), "{json}");
        // The unterminated listing is reported in the included source.
        assert!(json.contains(r#"{"source_id":1,"start":18,"#), "{json}");
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let mut src = String::new();
        for depth in 1..=5 {
            src.push_str(&format!("{} item\n", "*".repeat(depth)));
        }
        let json = editor_json(&src);
        assert!(json.contains(r#""depth":9,"kind":"list-item"}"#), "{json}");
    }
}
