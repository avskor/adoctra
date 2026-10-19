//! Shared mutable traversal of the semantic AST, including metadata.
//!
//! A pass can override a hook and call its `walk_*` function to continue walking.
//! Inline fields carry their context: visiting every field does not imply that
//! every field uses the same substitutions or anchor-registration rules.

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

    fn visit_block(&mut self, block: &mut Block) {
        walk_block_mut(self, block);
    }

    fn visit_inlines(&mut self, inlines: &mut [Inline], _context: InlineContext) {
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
        visitor.visit_block(&mut section.heading);
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
        visitor.visit_block(block);
    }
}

/// Visit metadata, then content, recursively. This is structural order, not
/// exact source order: metadata can originate before a block or be synthesized.
pub fn walk_block_mut(visitor: &mut (impl VisitorMut + ?Sized), block: &mut Block) {
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
    match &mut block.kind {
        BlockKind::Heading { inlines, .. }
        | BlockKind::Paragraph { inlines }
        | BlockKind::Stem { inlines, .. }
        | BlockKind::Verse { inlines, .. } => visitor.visit_inlines(inlines, InlineContext::Body),
        BlockKind::List { items, .. } => {
            for item in items {
                visitor.visit_span(&mut item.span);
                visitor.visit_inlines(&mut item.inlines, InlineContext::Body);
                walk_blocks_mut(visitor, &mut item.blocks);
            }
        }
        BlockKind::CalloutList { items } => {
            for entry in items {
                visitor.visit_span(&mut entry.item.span);
                visitor.visit_inlines(&mut entry.item.inlines, InlineContext::Body);
                walk_blocks_mut(visitor, &mut entry.item.blocks);
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
            for item in items {
                visitor.visit_span(&mut item.span);
                for term in &mut item.terms {
                    visitor.visit_span(&mut term.span);
                    visitor.visit_inlines(&mut term.inlines, InlineContext::Body);
                }
                if let Some(description) = &mut item.description {
                    visitor.visit_span(&mut description.span);
                    visitor.visit_inlines(&mut description.inlines, InlineContext::Body);
                    walk_blocks_mut(visitor, &mut description.blocks);
                }
            }
        }
        BlockKind::Admonition { blocks, .. }
        | BlockKind::Example { blocks }
        | BlockKind::Quote { blocks, .. }
        | BlockKind::Sidebar { blocks }
        | BlockKind::Open { blocks }
        | BlockKind::Section { blocks, .. }
        | BlockKind::Preamble { blocks } => walk_blocks_mut(visitor, blocks),
        BlockKind::Table(table) => {
            for cell in table.cells_mut() {
                visitor.visit_span(&mut cell.span);
                match &mut cell.content {
                    crate::ast::CellContent::Paragraphs(paragraphs) => {
                        for inlines in paragraphs {
                            visitor.visit_inlines(inlines, InlineContext::Body);
                        }
                    }
                    crate::ast::CellContent::Blocks {
                        blocks, footnotes, ..
                    } => {
                        walk_blocks_mut(visitor, blocks);
                        for footnote in footnotes {
                            visitor.visit_span(&mut footnote.span);
                        }
                    }
                    crate::ast::CellContent::Literal(_) => {}
                }
            }
        }
        BlockKind::Passthrough { .. }
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
            fn visit_inlines(&mut self, inlines: &mut [Inline], context: InlineContext) {
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
