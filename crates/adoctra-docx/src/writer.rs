//! Body markup from the document's blocks, walked on an explicit stack (no
//! recursion over block nesting, like the parser and the other backends).

use adoctra_core::ast::{Attribution, Block, BlockKind};
use adoctra_core::{AttributePart, Document, Inline};

use crate::inline::{Cx, Newlines, Runs};
use crate::styles::StyleKind;
use crate::xml;

/// Context a block inherits from its container.
#[derive(Debug, Clone, Copy, Default)]
struct Ctx {
    /// The first paragraph of a preamble (Ruby's stylesheet enlarges it).
    lead: bool,
    /// Inside a quote block.
    quote: bool,
    /// Inside an abstract.
    abstract_: bool,
}

/// Section breaks of a book whose body section has a distinct first page
/// (`w:titlePg`): every part and chapter starts a section, so its opening
/// page gets the first-page header and footer.
pub struct ChapterSections {
    /// The template's body section properties.
    sect_pr: String,
    breaks: usize,
}

impl ChapterSections {
    pub fn new(sect_pr: String) -> Self {
        Self { sect_pr, breaks: 0 }
    }

    /// The break ending the current section: the first keeps the template's
    /// page numbering restart, later ones continue the numbering.
    fn next(&mut self) -> String {
        let sect_pr = if self.breaks == 0 {
            self.sect_pr.clone()
        } else {
            continue_numbering(&self.sect_pr)
        };
        self.breaks += 1;
        format!("<w:p><w:pPr>{sect_pr}</w:pPr></w:p>")
    }

    /// The final section properties when breaks were inserted.
    pub fn final_sect_pr(&self) -> Option<String> {
        (self.breaks > 0).then(|| continue_numbering(&self.sect_pr))
    }
}

/// Section properties without a page-number restart.
pub fn continue_numbering(sect_pr: &str) -> String {
    let Some(tag) = xml::tags(sect_pr, 0).find(|t| t.name == "w:pgNumType" && t.opens()) else {
        return sect_pr.to_string();
    };
    let tag_text = &sect_pr[tag.range.clone()];
    let Some(at) = tag_text.find(" w:start=") else {
        return sect_pr.to_string();
    };
    let quote = tag_text.as_bytes()[at + 9];
    let end = tag_text[at + 10..]
        .find(quote as char)
        .map_or(tag_text.len(), |e| at + 11 + e);
    let rewritten = format!("{}{}", &tag_text[..at], &tag_text[end..]);
    format!(
        "{}{rewritten}{}",
        &sect_pr[..tag.range.start],
        &sect_pr[tag.range.end..]
    )
}

/// Work on the explicit stack.
enum Task<'c> {
    Block(&'c Block, Ctx),
    /// A quote's attribution, after its blocks.
    Attribution(&'c Attribution, &'static str),
}

pub struct Writer<'c, 't> {
    pub cx: &'c mut Cx<'t>,
    doc: &'c Document,
    out: String,
    /// Block ids waiting for the next paragraph's bookmark.
    pending: Vec<String>,
    chapters: Option<&'c mut ChapterSections>,
    book: bool,
}

impl<'c, 't> Writer<'c, 't> {
    pub fn new(
        cx: &'c mut Cx<'t>,
        doc: &'c Document,
        chapters: Option<&'c mut ChapterSections>,
    ) -> Self {
        Self {
            cx,
            doc,
            out: String::new(),
            pending: Vec::new(),
            chapters,
            book: doc.attributes.get("doctype") == Some("book"),
        }
    }

    /// Write the body (`title` first, when the template has no title
    /// marker) and return its markup.
    pub fn body(mut self, title: Option<String>) -> String {
        if let Some(title) = title {
            self.out.push_str(&title);
        }
        let blocks = &self.doc.blocks[usize::from(self.doc.has_title)..];
        let mut stack: Vec<Task<'c>> = blocks
            .iter()
            .rev()
            .map(|b| Task::Block(b, Ctx::default()))
            .collect();
        while let Some(task) = stack.pop() {
            let next = match task {
                Task::Block(block, ctx) => self.block(block, ctx),
                Task::Attribution(attribution, style) => {
                    self.attribution(attribution, style);
                    Vec::new()
                }
            };
            stack.extend(next.into_iter().rev());
        }
        self.out
    }

    /// Write `block`; return its children to write next, in order.
    fn block(&mut self, block: &'c Block, ctx: Ctx) -> Vec<Task<'c>> {
        let inherit = |blocks: &'c [Block], ctx: Ctx| -> Vec<Task<'c>> {
            blocks.iter().map(|b| Task::Block(b, ctx)).collect()
        };
        match &block.kind {
            BlockKind::Section { level, blocks } => {
                if self.book && *level <= 1 && !self.out.is_empty() {
                    if let Some(chapters) = self.chapters.as_deref_mut() {
                        let page = chapters.next();
                        self.out.push_str(&page);
                    }
                }
                inherit(blocks, Ctx::default())
            }
            BlockKind::Heading { level, inlines } => {
                self.heading(block, *level, inlines);
                Vec::new()
            }
            BlockKind::Preamble { blocks } => {
                let mut children = inherit(blocks, Ctx::default());
                if let Some(Task::Block(first, ctx)) = children.first_mut() {
                    ctx.lead = matches!(first.kind, BlockKind::Paragraph { .. })
                        && first.roles.is_empty()
                        && first.attrs.style.is_none();
                }
                children
            }
            BlockKind::Paragraph { inlines } => {
                self.title(block, "Block Title");
                let style = if block.roles.iter().any(|r| r == "lead") || ctx.lead {
                    "Lead"
                } else if ctx.abstract_ {
                    "Abstract"
                } else if ctx.quote {
                    "Quote"
                } else {
                    "Normal"
                };
                self.inline_paragraph(block, style, inlines, Newlines::Space);
                Vec::new()
            }
            BlockKind::Open { blocks } => {
                self.title(block, "Block Title");
                self.anchor(block);
                let abstract_ = block.attrs.style.as_deref() == Some("abstract");
                inherit(
                    blocks,
                    Ctx {
                        abstract_: ctx.abstract_ || abstract_,
                        ..ctx
                    },
                )
            }
            BlockKind::Example { blocks } => {
                self.title(block, "Block Title");
                self.anchor(block);
                inherit(blocks, ctx)
            }
            BlockKind::Sidebar { blocks } => {
                self.title(block, "Sidebar Title");
                self.anchor(block);
                inherit(blocks, ctx)
            }
            BlockKind::Admonition { label, blocks, .. } => {
                self.title(block, "Block Title");
                self.anchor(block);
                let mut runs = Runs::new(self.cx, Newlines::Space);
                runs.parts(label);
                let runs = runs.finish();
                self.paragraph("Admonition Label", "", &runs);
                inherit(blocks, ctx)
            }
            BlockKind::Quote {
                attribution,
                blocks,
            } => {
                self.title(block, "Block Title");
                self.anchor(block);
                // The attribution follows the quoted blocks.
                let mut children = inherit(blocks, Ctx { quote: true, ..ctx });
                if let Some(attribution) = attribution {
                    children.push(Task::Attribution(attribution, "Quote Attribution"));
                }
                children
            }
            BlockKind::Verse {
                attribution,
                inlines,
            } => {
                self.title(block, "Block Title");
                self.inline_paragraph(block, "Verse", inlines, Newlines::Break);
                if let Some(attribution) = attribution {
                    self.attribution(attribution, "Verse Attribution");
                }
                Vec::new()
            }
            BlockKind::Listing { text, inlines, .. } => {
                self.title(block, "Block Title");
                self.verbatim(block, "Code", text, inlines.as_deref());
                Vec::new()
            }
            BlockKind::Literal { text, inlines, .. } => {
                self.title(block, "Block Title");
                self.verbatim(block, "Literal", text, inlines.as_deref());
                Vec::new()
            }
            BlockKind::Stem { inlines, .. } => {
                self.title(block, "Block Title");
                self.inline_paragraph(block, "Code", inlines, Newlines::Break);
                Vec::new()
            }
            BlockKind::Passthrough { .. } => {
                self.cx
                    .warnings
                    .once("docx backend drops passthrough blocks (raw output has no DOCX form)");
                Vec::new()
            }
            BlockKind::ThematicBreak => {
                self.paragraph("Horizontal Rule", "", "");
                Vec::new()
            }
            BlockKind::PageBreak => {
                self.out
                    .push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
                Vec::new()
            }
            BlockKind::List { .. } => self.not_yet("lists"),
            BlockKind::CalloutList { .. } => self.not_yet("callout lists"),
            BlockKind::DescriptionList { .. } => self.not_yet("description lists"),
            BlockKind::Table(_) => self.not_yet("tables"),
            BlockKind::Image { .. } => self.not_yet("block images"),
            BlockKind::Video { .. } | BlockKind::Audio { .. } => self.not_yet("video and audio"),
            BlockKind::Toc => self.not_yet("the table of contents"),
        }
    }

    fn not_yet(&mut self, what: &str) -> Vec<Task<'c>> {
        self.cx
            .warnings
            .once(format!("docx backend does not convert {what} yet"));
        Vec::new()
    }

    /// The block's id becomes a bookmark in its next paragraph.
    fn anchor(&mut self, block: &Block) {
        if let Some(id) = &block.id {
            self.pending.push(id.clone());
        }
    }

    fn heading(&mut self, block: &Block, level: u8, inlines: &[Inline]) {
        let n = level.clamp(1, 6);
        let style = match block.attrs.style.as_deref() {
            Some("discrete" | "float") => format!("Floating Title {n}"),
            _ if level == 0 => "Part Title".to_string(),
            Some("appendix") => format!("Appendix Heading {n}"),
            _ => format!("Heading {n}"),
        };
        self.anchor(block);
        let mut runs = Runs::new(self.cx, Newlines::Space);
        if !block.caption_parts.is_empty() {
            runs.parts(&block.caption_parts);
        } else if let Some(caption) = &block.caption {
            runs.text(caption);
        }
        runs.inlines(inlines);
        let runs = runs.finish();
        self.paragraph(&style, "", &runs);
    }

    /// A block's `.Title` with its caption (`Example 1. `).
    fn title(&mut self, block: &Block, style: &str) {
        let Some(title) = &block.title else { return };
        let mut runs = Runs::new(self.cx, Newlines::Space);
        if !block.caption_parts.is_empty() {
            runs.parts(&block.caption_parts);
        } else if let Some(caption) = &block.caption {
            runs.text(caption);
        }
        runs.inlines(title);
        let runs = runs.finish();
        self.paragraph(style, "", &runs);
    }

    fn inline_paragraph(&mut self, block: &Block, style: &str, inlines: &[Inline], nl: Newlines) {
        self.anchor(block);
        let mut runs = Runs::new(self.cx, nl);
        runs.inlines(inlines);
        let runs = runs.finish();
        self.paragraph(style, &alignment(&block.roles), &runs);
    }

    /// A verbatim block, one paragraph per line (custom substitutions give
    /// an inline tree, written as one paragraph with line breaks).
    fn verbatim(&mut self, block: &Block, style: &str, text: &str, inlines: Option<&[Inline]>) {
        self.anchor(block);
        if let Some(inlines) = inlines {
            let mut runs = Runs::new(self.cx, Newlines::Break);
            runs.inlines(inlines);
            let runs = runs.finish();
            self.paragraph(style, "", &runs);
            return;
        }
        for line in text.split('\n') {
            let mut runs = Runs::new(self.cx, Newlines::Break);
            runs.text(line);
            let runs = runs.finish();
            self.paragraph(style, "", &runs);
        }
    }

    /// `— author, cite`: Ruby's attribution, as a paragraph after the
    /// quoted content.
    fn attribution(&mut self, attribution: &Attribution, style: &str) {
        let mut runs = Runs::new(self.cx, Newlines::Space);
        if let Some(author) = &attribution.author {
            runs.text("\u{2014} ");
            runs.inlines(author);
            if attribution.citetitle.is_some() {
                runs.line_break();
            }
        }
        if let Some(cite) = &attribution.citetitle {
            runs.inlines(cite);
        }
        runs.inlines(&attribution.trailing);
        let runs = runs.finish();
        self.paragraph(style, "", &runs);
    }

    /// A paragraph in style `name` (a missing custom style falls back to
    /// the template's default paragraph style), with the pending bookmarks.
    fn paragraph(&mut self, name: &str, extra_ppr: &str, runs: &str) {
        let id = self.cx.style(StyleKind::Paragraph, name);
        let mut ppr = String::new();
        if let Some(id) = &id {
            ppr.push_str(&format!(r#"<w:pStyle w:val="{}"/>"#, xml::escape_attr(id)));
            // A heading style numbered by the template: core already wrote
            // the number (Ruby's), so Word's must not add another.
            if self.cx.styles.num_id(id).is_some() {
                ppr.push_str(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="0"/></w:numPr>"#);
            }
        }
        ppr.push_str(extra_ppr);
        self.out.push_str("<w:p>");
        if !ppr.is_empty() {
            self.out.push_str(&format!("<w:pPr>{ppr}</w:pPr>"));
        }
        for id in std::mem::take(&mut self.pending) {
            let mark = self.cx.bookmarks.point(&id);
            self.out.push_str(&mark);
        }
        self.out.push_str(runs);
        self.out.push_str("</w:p>");
    }
}

/// A paragraph in style `name` outside the body flow (the title when the
/// template has no title marker).
pub fn simple_paragraph(cx: &mut Cx<'_>, name: &str, runs: &str) -> String {
    match cx.style(StyleKind::Paragraph, name) {
        Some(id) => format!(
            r#"<w:p><w:pPr><w:pStyle w:val="{}"/></w:pPr>{runs}</w:p>"#,
            xml::escape_attr(&id)
        ),
        None => format!("<w:p>{runs}</w:p>"),
    }
}

/// `w:jc` for Ruby's `text-*` alignment roles.
fn alignment(roles: &[String]) -> String {
    let jc = roles.iter().rev().find_map(|role| match role.as_str() {
        "text-left" => Some("left"),
        "text-center" => Some("center"),
        "text-right" => Some("right"),
        "text-justify" => Some("both"),
        _ => None,
    });
    jc.map(|v| format!(r#"<w:jc w:val="{v}"/>"#))
        .unwrap_or_default()
}

/// Split a document title at the last `title-separator` (default `:`)
/// followed by a space, like Ruby's `Document#doctitle partition: true`:
/// the main title and the subtitle.
pub fn partition_title(inlines: &[Inline], separator: &str) -> (Vec<Inline>, Option<Vec<Inline>>) {
    let sep = format!("{separator} ");
    for (i, inline) in inlines.iter().enumerate().rev() {
        let split =
            match inline {
                Inline::Text { value, span } => value.rfind(&sep).map(|at| {
                    (
                        Inline::Text {
                            value: value[..at].trim_end().to_string(),
                            span: *span,
                        },
                        Inline::Text {
                            value: value[at + sep.len()..].to_string(),
                            span: *span,
                        },
                    )
                }),
                Inline::Substituted { value, parts, span } => parts
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(p, part)| match part {
                        AttributePart::Text(text) => text.rfind(&sep).map(|at| {
                            let mut head = parts[..p].to_vec();
                            head.push(AttributePart::Text(text[..at].trim_end().to_string()));
                            let mut tail =
                                vec![AttributePart::Text(text[at + sep.len()..].to_string())];
                            tail.extend_from_slice(&parts[p + 1..]);
                            let sub = |parts: Vec<AttributePart>| Inline::Substituted {
                                value: value.clone(),
                                parts,
                                span: *span,
                            };
                            (sub(head), sub(tail))
                        }),
                        _ => None,
                    }),
                _ => None,
            };
        if let Some((head, tail)) = split {
            let mut main = inlines[..i].to_vec();
            main.push(head);
            let mut sub = vec![tail];
            sub.extend_from_slice(&inlines[i + 1..]);
            return (main, Some(sub));
        }
    }
    (inlines.to_vec(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_chapter_sections_continue_page_numbers() {
        let sect = r#"<w:sectPr><w:pgNumType w:fmt="decimal" w:start="1"/><w:titlePg/></w:sectPr>"#;
        assert_eq!(
            continue_numbering(sect),
            r#"<w:sectPr><w:pgNumType w:fmt="decimal"/><w:titlePg/></w:sectPr>"#
        );
        let mut chapters = ChapterSections::new(sect.to_string());
        assert!(chapters.final_sect_pr().is_none());
        assert!(chapters.next().contains(r#"w:start="1""#));
        assert!(!chapters.next().contains("w:start"));
        assert!(!chapters.final_sect_pr().unwrap().contains("w:start"));
        assert_eq!(continue_numbering("<w:sectPr/>"), "<w:sectPr/>");
    }

    #[test]
    fn alignment_roles_set_justification() {
        assert_eq!(
            alignment(&["x".into(), "text-center".into()]),
            r#"<w:jc w:val="center"/>"#
        );
        assert_eq!(
            alignment(&["text-justify".into()]),
            r#"<w:jc w:val="both"/>"#
        );
        assert_eq!(alignment(&[]), "");
    }
}
