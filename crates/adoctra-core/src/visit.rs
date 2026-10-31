//! Shared mutable traversal of the semantic AST, including metadata.
//!
//! A pass overrides hooks: [`VisitorMut::enter_block`] and
//! [`VisitorMut::leave_block`] around a block's content and nested blocks,
//! and the inline hooks, which call their `walk_*` function to continue.
//! Inline fields carry their context: visiting every field does not imply that
//! every field uses the same substitutions or anchor-registration rules.
//!
//! Blocks are walked without recursion (user decision 2026-10-31): nested
//! block groups are taken out of their block onto an explicit stack of owned
//! frames and put back once walked, so no document depth can overflow the
//! call stack. The order is the structural order of the recursive walk.

use crate::ast::{Block, BlockKind, Document, Inline, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineContext {
    Body,
    Title,
    Reftext,
    Attribution,
}

pub trait VisitorMut {
    fn visit_span(&mut self, _span: &mut Span) {}

    /// Before a block's metadata, content and nested blocks. Returning
    /// `false` skips them and [`leave_block`](VisitorMut::leave_block): the
    /// visitor handled the block itself.
    fn enter_block(&mut self, _block: &mut Block) -> bool {
        true
    }

    /// After a block's content and nested blocks.
    fn leave_block(&mut self, _block: &mut Block) {}

    fn visit_inlines(&mut self, inlines: &mut Vec<Inline>, _context: InlineContext) {
        walk_inlines_mut(self, inlines);
    }

    fn visit_inline(&mut self, inline: &mut Inline) {
        walk_inline_mut(self, inline);
    }
}

pub fn walk_document_mut(visitor: &mut (impl VisitorMut + ?Sized), document: &mut Document) {
    if let Some(header) = &mut document.header {
        visitor.visit_span(&mut header.span);
        visitor.visit_span(&mut header.title_span);
        for author in &mut header.authors {
            visitor.visit_span(&mut author.span);
        }
        if let Some(revision) = &mut header.revision {
            visitor.visit_span(&mut revision.span);
        }
    }
    if let Some(section) = document
        .manpage
        .as_mut()
        .and_then(|manpage| manpage.name_section.as_mut())
    {
        walk_block_mut(visitor, &mut section.heading);
        if let Some(body) = &mut section.body {
            visitor.visit_span(body);
        }
    }
    walk_blocks_mut(visitor, &mut document.blocks);
    for footnote in &mut document.footnotes {
        visitor.visit_span(&mut footnote.span);
    }
}

pub fn walk_blocks_mut(visitor: &mut (impl VisitorMut + ?Sized), blocks: &mut [Block]) {
    for block in blocks {
        walk_block_mut(visitor, block);
    }
}

/// A block being walked: the root is borrowed, nested blocks are owned by
/// their frame while their group is out of the parent.
enum Held<'a> {
    Root(&'a mut Block),
    Owned(Box<Block>),
}

impl Held<'_> {
    fn get(&mut self) -> &mut Block {
        match self {
            Held::Root(block) => block,
            Held::Owned(block) => block,
        }
    }
}

struct Frame<'a> {
    block: Held<'a>,
    /// The next local part (and group) of the block.
    next: usize,
    groups: usize,
    /// The group being walked: its remaining and its walked blocks.
    pending: Option<(std::vec::IntoIter<Block>, Vec<Block>)>,
}

impl<'a> Frame<'a> {
    fn new(mut block: Held<'a>) -> Self {
        let groups = groups(block.get());
        Frame {
            block,
            next: 0,
            groups,
            pending: None,
        }
    }
}

/// Visit metadata, then content and nested blocks in structural order. This
/// is not exact source order: metadata can originate before a block or be
/// synthesized.
pub fn walk_block_mut(visitor: &mut (impl VisitorMut + ?Sized), root: &mut Block) {
    if !visitor.enter_block(root) {
        return;
    }
    let mut stack = vec![Frame::new(Held::Root(root))];
    while let Some(top) = stack.last_mut() {
        if let Some((pending, walked)) = &mut top.pending {
            if let Some(mut child) = pending.next() {
                if visitor.enter_block(&mut child) {
                    stack.push(Frame::new(Held::Owned(Box::new(child))));
                } else {
                    walked.push(child);
                }
                continue;
            }
            let walked = std::mem::take(walked);
            put_group(top.block.get(), top.next, walked);
            top.pending = None;
            top.next += 1;
        }
        walk_local(visitor, top.block.get(), top.next);
        if top.next < top.groups {
            let group = take_group(top.block.get(), top.next);
            let walked = Vec::with_capacity(group.len());
            top.pending = Some((group.into_iter(), walked));
            continue;
        }
        let mut frame = stack.pop().expect("a frame");
        visitor.leave_block(frame.block.get());
        if let Held::Owned(block) = frame.block {
            let parent = stack.last_mut().expect("an owned block has a parent");
            parent
                .pending
                .as_mut()
                .expect("a walked group")
                .1
                .push(*block);
        }
    }
}

/// Drop blocks without recursion: take each block's nested groups out onto a
/// stack before the (now shallow) block itself is dropped.
pub(crate) fn dismantle(mut stack: Vec<Block>) {
    while let Some(mut block) = stack.pop() {
        for k in 0..groups(&block) {
            stack.append(&mut take_group(&mut block, k));
        }
    }
}

/// How many nested block groups a block has: one per list item, description
/// or table cell (empty when it holds none), one for a container.
fn groups(block: &Block) -> usize {
    match &block.kind {
        BlockKind::List { items, .. } => items.len(),
        BlockKind::CalloutList { items } => items.len(),
        BlockKind::DescriptionList { items } => items.len(),
        BlockKind::Table(table) => table
            .head
            .iter()
            .chain(&table.body)
            .chain(&table.foot)
            .map(Vec::len)
            .sum(),
        BlockKind::Admonition { .. }
        | BlockKind::Example { .. }
        | BlockKind::Quote { .. }
        | BlockKind::Sidebar { .. }
        | BlockKind::Open { .. }
        | BlockKind::Section { .. }
        | BlockKind::Preamble { .. } => 1,
        _ => 0,
    }
}

/// The table cell at flat position `k` (head, body, foot rows).
fn table_cell(table: &mut crate::ast::Table, mut k: usize) -> &mut crate::ast::TableCell {
    for row in table
        .head
        .iter_mut()
        .chain(&mut table.body)
        .chain(&mut table.foot)
    {
        if k < row.len() {
            return &mut row[k];
        }
        k -= row.len();
    }
    unreachable!("a cell index within the table")
}

/// The blocks of group `k`, taken out of the block.
fn take_group(block: &mut Block, k: usize) -> Vec<Block> {
    match &mut block.kind {
        BlockKind::List { items, .. } => std::mem::take(&mut items[k].blocks),
        BlockKind::CalloutList { items } => std::mem::take(&mut items[k].item.blocks),
        BlockKind::DescriptionList { items } => items[k]
            .description
            .as_mut()
            .map(|description| std::mem::take(&mut description.blocks))
            .unwrap_or_default(),
        BlockKind::Table(table) => match &mut table_cell(table, k).content {
            crate::ast::CellContent::Blocks { blocks, .. } => std::mem::take(blocks),
            _ => Vec::new(),
        },
        BlockKind::Admonition { blocks, .. }
        | BlockKind::Example { blocks }
        | BlockKind::Quote { blocks, .. }
        | BlockKind::Sidebar { blocks }
        | BlockKind::Open { blocks }
        | BlockKind::Section { blocks, .. }
        | BlockKind::Preamble { blocks } => std::mem::take(blocks),
        _ => Vec::new(),
    }
}

/// Put the walked blocks of group `k` back.
fn put_group(block: &mut Block, k: usize, walked: Vec<Block>) {
    let slot = match &mut block.kind {
        BlockKind::List { items, .. } => &mut items[k].blocks,
        BlockKind::CalloutList { items } => &mut items[k].item.blocks,
        BlockKind::DescriptionList { items } => match &mut items[k].description {
            Some(description) => &mut description.blocks,
            None => return,
        },
        BlockKind::Table(table) => match &mut table_cell(table, k).content {
            crate::ast::CellContent::Blocks { blocks, .. } => blocks,
            _ => return,
        },
        BlockKind::Admonition { blocks, .. }
        | BlockKind::Example { blocks }
        | BlockKind::Quote { blocks, .. }
        | BlockKind::Sidebar { blocks }
        | BlockKind::Open { blocks }
        | BlockKind::Section { blocks, .. }
        | BlockKind::Preamble { blocks } => blocks,
        _ => return,
    };
    *slot = walked;
}

/// The block's own content before group `k` (after the last one when `k`
/// is the number of groups): its metadata first, then each item's or cell's
/// fields ahead of its blocks.
fn walk_local(visitor: &mut (impl VisitorMut + ?Sized), block: &mut Block, k: usize) {
    if k == 0 {
        visitor.visit_span(&mut block.span);
        if let Some(title) = &mut block.title {
            visitor.visit_inlines(title, InlineContext::Title);
        }
        if let Some(reftext) = &mut block.reftext {
            visitor.visit_inlines(reftext, InlineContext::Reftext);
        }
        if let BlockKind::Quote {
            attribution: Some(attribution),
            ..
        }
        | BlockKind::Verse {
            attribution: Some(attribution),
            ..
        } = &mut block.kind
        {
            if let Some(author) = &mut attribution.author {
                visitor.visit_inlines(author, InlineContext::Attribution);
            }
            if let Some(citetitle) = &mut attribution.citetitle {
                visitor.visit_inlines(citetitle, InlineContext::Attribution);
            }
        }
    }
    match &mut block.kind {
        BlockKind::Heading { inlines, .. }
        | BlockKind::Paragraph { inlines }
        | BlockKind::Stem { inlines, .. }
        | BlockKind::Verse { inlines, .. } => visitor.visit_inlines(inlines, InlineContext::Body),
        BlockKind::List { items, .. } => {
            if let Some(item) = items.get_mut(k) {
                visitor.visit_span(&mut item.span);
                visitor.visit_inlines(&mut item.inlines, InlineContext::Body);
            }
        }
        BlockKind::CalloutList { items } => {
            if let Some(entry) = items.get_mut(k) {
                visitor.visit_span(&mut entry.item.span);
                visitor.visit_inlines(&mut entry.item.inlines, InlineContext::Body);
            }
        }
        BlockKind::Listing {
            callouts, inlines, ..
        }
        | BlockKind::Literal {
            callouts, inlines, ..
        } => {
            for callout in callouts {
                visitor.visit_span(&mut callout.span);
            }
            if let Some(inlines) = inlines {
                visitor.visit_inlines(inlines, InlineContext::Body);
            }
        }
        BlockKind::Passthrough {
            inlines: Some(inlines),
            ..
        } => visitor.visit_inlines(inlines, InlineContext::Body),
        BlockKind::DescriptionList { items } => {
            if let Some(item) = items.get_mut(k) {
                visitor.visit_span(&mut item.span);
                for term in &mut item.terms {
                    visitor.visit_span(&mut term.span);
                    visitor.visit_inlines(&mut term.inlines, InlineContext::Body);
                }
                if let Some(description) = &mut item.description {
                    visitor.visit_span(&mut description.span);
                    visitor.visit_inlines(&mut description.inlines, InlineContext::Body);
                }
            }
        }
        BlockKind::Table(table) => {
            // The previous cell's footnotes follow its blocks.
            if k > 0 {
                if let crate::ast::CellContent::Blocks { footnotes, .. } =
                    &mut table_cell(table, k - 1).content
                {
                    for footnote in footnotes {
                        visitor.visit_span(&mut footnote.span);
                    }
                }
            }
            let cells = table
                .head
                .iter()
                .chain(&table.body)
                .chain(&table.foot)
                .map(Vec::len)
                .sum();
            if k < cells {
                let cell = table_cell(table, k);
                visitor.visit_span(&mut cell.span);
                if let crate::ast::CellContent::Paragraphs(paragraphs) = &mut cell.content {
                    for inlines in paragraphs {
                        visitor.visit_inlines(inlines, InlineContext::Body);
                    }
                }
            }
        }
        BlockKind::Admonition { .. }
        | BlockKind::Example { .. }
        | BlockKind::Quote { .. }
        | BlockKind::Sidebar { .. }
        | BlockKind::Open { .. }
        | BlockKind::Section { .. }
        | BlockKind::Preamble { .. }
        | BlockKind::Passthrough { .. }
        | BlockKind::Toc
        | BlockKind::ThematicBreak
        | BlockKind::PageBreak
        | BlockKind::Image { .. }
        | BlockKind::Video { .. }
        | BlockKind::Audio { .. } => {}
    }
}

pub fn walk_inlines_mut(visitor: &mut (impl VisitorMut + ?Sized), inlines: &mut [Inline]) {
    for inline in inlines {
        visitor.visit_inline(inline);
    }
}

pub fn walk_inline_mut(visitor: &mut (impl VisitorMut + ?Sized), inline: &mut Inline) {
    visitor.visit_span(inline.span_mut());
    match inline {
        Inline::Strong { children, .. }
        | Inline::Emphasis { children, .. }
        | Inline::Monospace { children, .. }
        | Inline::Mark { children, .. }
        | Inline::Superscript { children, .. }
        | Inline::Subscript { children, .. }
        | Inline::Link { children, .. }
        | Inline::CrossReference { children, .. } => walk_inlines_mut(visitor, children),
        Inline::Anchor { .. }
        | Inline::Image { .. }
        | Inline::Footnote { .. }
        | Inline::Ui { .. }
        | Inline::IndexTerm { .. }
        | Inline::Stem { .. }
        | Inline::Text { .. }
        | Inline::Raw { .. }
        | Inline::Substituted { .. }
        | Inline::AttributeRef { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transformation_reaches_metadata_and_nested_content_once() {
        struct Uppercase {
            fields: Vec<InlineContext>,
        }
        impl VisitorMut for Uppercase {
            fn visit_inlines(&mut self, inlines: &mut Vec<Inline>, context: InlineContext) {
                self.fields.push(context);
                walk_inlines_mut(self, inlines);
            }
            fn visit_inline(&mut self, inline: &mut Inline) {
                if let Inline::Text { value, .. } = inline {
                    *value = format!("{}!", value.to_uppercase());
                }
                walk_inline_mut(self, inline);
            }
        }
        let source = "[[q,reference]]\n.Caption\n[quote, 'author', 'work']\n____\n* *item*\n+\n--\nnested [[point,anchor text]]body\n--\n____";
        let mut document = crate::parse(source).document;
        let mut visitor = Uppercase { fields: Vec::new() };
        walk_document_mut(&mut visitor, &mut document);
        assert_eq!(
            visitor.fields,
            vec![
                InlineContext::Title,
                InlineContext::Reftext,
                InlineContext::Attribution,
                InlineContext::Attribution,
                InlineContext::Body,
                InlineContext::Body,
            ]
        );
        let debug = format!("{document:?}");
        for value in [
            "CAPTION!",
            "REFERENCE!",
            "AUTHOR!",
            "WORK!",
            "ITEM!",
            "BODY!",
        ] {
            assert!(debug.contains(value), "missing {value}: {debug}");
        }
        assert!(!debug.contains("!!"), "a field was visited twice");
    }
}
