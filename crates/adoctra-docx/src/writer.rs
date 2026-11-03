//! Body markup from the document's blocks, walked on an explicit stack (no
//! recursion over block nesting, like the parser and the other backends).

use adoctra_core::ast::{
    AdmonitionKind, Attribution, Block, BlockKind, Callout, CalloutListItem, CellContent,
    CellStyle, DescriptionListItem, HAlign, ListItem, ListTerm, Table, TableCell, VAlign,
};
use adoctra_core::{AttributeFormat, AttributePart, Document, Inline};

use crate::inline::{Cx, Newlines, Runs};
use crate::numbering::{self, Level, LIST_INDENT};
use crate::styles::StyleKind;
use crate::table::{self, Slot};
use crate::xml;

/// The paragraph after a block's table: it keeps adjacent tables apart (Word
/// would join them) and stands for Ruby's block margin.
const SPACER: &str = r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="160" w:lineRule="exact"/></w:pPr></w:p>"#;
/// Ruby's stripe background.
const STRIPE: &str = "F8F8F7";

/// Context a block inherits from its container.
#[derive(Debug, Clone, Copy, Default)]
struct Ctx {
    /// The first paragraph of a preamble (Ruby's stylesheet enlarges it).
    lead: bool,
    /// Inside a quote block.
    quote: bool,
    /// Inside an abstract.
    abstract_: bool,
    /// Left indent of the enclosing content (twips): list items and
    /// descriptions indent their blocks.
    indent: u32,
    /// Unordered lists this block is nested in (the default bullet).
    bullets: u8,
}

/// A description's indent from its term (twips).
const DESCRIPTION_INDENT: u32 = 360;

/// The text width of a page when the template's section does not say (A4
/// with 2 cm margins, twips).
pub const DEFAULT_TEXT_WIDTH: u32 = 9638;

/// The text width (twips) of section properties: page width minus margins
/// and gutter.
pub fn text_width(sect_pr: &str) -> u32 {
    let attr = |tag: &str, name: &str| {
        xml::tags(sect_pr, 0)
            .find(|t| t.name == tag && t.opens())
            .and_then(|t| t.attr(name))
            .and_then(|v| v.parse::<i64>().ok())
    };
    let Some(page) = attr("w:pgSz", "w:w") else {
        return DEFAULT_TEXT_WIDTH;
    };
    let margins = ["w:left", "w:right", "w:gutter"]
        .iter()
        .map(|m| attr("w:pgMar", m).unwrap_or(0))
        .sum::<i64>();
    u32::try_from(page - margins)
        .ok()
        .filter(|w| *w > 0)
        .unwrap_or(DEFAULT_TEXT_WIDTH)
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
    Attribution(&'c Attribution, &'static str, Ctx),
    /// A list item: its numbered paragraph, then its blocks.
    Item {
        item: &'c ListItem,
        num: u32,
        check: Option<&'static str>,
        ctx: Ctx,
    },
    /// Description list terms; the first carries the list number (qanda).
    Terms {
        terms: &'c [ListTerm],
        style: &'static str,
        num: Option<u32>,
        ctx: Ctx,
    },
    /// A description: its text in `style` (indented by `para_indent`), then
    /// its blocks (indented by `ctx`).
    Description {
        item: &'c ListItem,
        style: &'static str,
        para_indent: u32,
        ctx: Ctx,
    },
    /// A callout list item with its number.
    Callout {
        item: &'c CalloutListItem,
        number: usize,
        ctx: Ctx,
    },
    /// Literal markup (table structure around nested content).
    Markup(String),
    /// A table cell's paragraph.
    Inlines {
        inlines: &'c [Inline],
        format: Option<AttributeFormat>,
        jc: &'static str,
    },
    /// A literal table cell.
    Literal(&'c str, &'static str),
    /// A table of contents (after a preamble).
    Toc(&'c adoctra_core::Toc, Option<&'c Block>),
    /// The end of a table cell, which must end with a paragraph.
    CloseCell,
}

pub struct Writer<'c, 't> {
    pub cx: &'c mut Cx<'t>,
    doc: &'c Document,
    out: String,
    /// Block ids waiting for the next paragraph's bookmark.
    pending: Vec<String>,
    chapters: Option<&'c mut ChapterSections>,
    book: bool,
    /// The indent of the paragraphs being written (twips).
    indent: u32,
    /// The next paragraph keeps with the one after it.
    keep_next: bool,
    text_width: u32,
}

impl<'c, 't> Writer<'c, 't> {
    pub fn new(
        cx: &'c mut Cx<'t>,
        doc: &'c Document,
        chapters: Option<&'c mut ChapterSections>,
        text_width: u32,
    ) -> Self {
        Self {
            cx,
            doc,
            out: String::new(),
            pending: Vec::new(),
            chapters,
            book: doc.attributes.get("doctype") == Some("book"),
            indent: 0,
            keep_next: false,
            text_width,
        }
    }

    /// Write the body (`title` first, when the template has no title
    /// marker) and return its markup.
    pub fn body(mut self, title: Option<String>) -> String {
        if let Some(title) = title {
            self.out.push_str(&title);
        }
        // The automatic table of contents (Ruby's header placement).
        if let Some(toc) = &self.doc.toc {
            self.toc(toc, None);
        }
        let blocks = &self.doc.blocks[usize::from(self.doc.has_title)..];
        let mut stack: Vec<Task<'c>> = blocks
            .iter()
            .rev()
            .map(|b| Task::Block(b, Ctx::default()))
            .collect();
        while let Some(task) = stack.pop() {
            let next = match task {
                Task::Block(block, ctx) => {
                    self.indent = ctx.indent;
                    self.block(block, ctx)
                }
                Task::Attribution(attribution, style, ctx) => {
                    self.indent = ctx.indent;
                    self.attribution(attribution, style);
                    Vec::new()
                }
                Task::Item {
                    item,
                    num,
                    check,
                    ctx,
                } => self.item(item, num, check, ctx),
                Task::Terms {
                    terms,
                    style,
                    num,
                    ctx,
                } => {
                    self.indent = ctx.indent;
                    for (i, term) in terms.iter().enumerate() {
                        let mut runs = Runs::new(self.cx, Newlines::Space);
                        runs.inlines(&term.inlines);
                        let runs = runs.finish();
                        let number = num.filter(|_| i == 0).map(list_number);
                        self.paragraph_with(style, number.as_deref(), "", &runs);
                    }
                    Vec::new()
                }
                Task::Description {
                    item,
                    style,
                    para_indent,
                    ctx,
                } => {
                    if !item.inlines.is_empty() {
                        self.indent = para_indent;
                        let mut runs = Runs::new(self.cx, Newlines::Space);
                        runs.inlines(&item.inlines);
                        let runs = runs.finish();
                        self.paragraph(style, "", &runs);
                    }
                    item.blocks.iter().map(|b| Task::Block(b, ctx)).collect()
                }
                Task::Callout { item, number, ctx } => self.callout(item, number, ctx),
                Task::Markup(markup) => {
                    self.out.push_str(&markup);
                    Vec::new()
                }
                Task::Inlines {
                    inlines,
                    format,
                    jc,
                } => {
                    self.indent = 0;
                    let mut runs = Runs::new(self.cx, Newlines::Space);
                    match format {
                        Some(format) => runs.with(format, |r| r.inlines(inlines)),
                        None => runs.inlines(inlines),
                    }
                    let runs = runs.finish();
                    self.paragraph("Normal", jc, &runs);
                    Vec::new()
                }
                Task::Toc(toc, block) => {
                    self.indent = 0;
                    self.toc(toc, block);
                    Vec::new()
                }
                Task::Literal(text, jc) => {
                    self.indent = 0;
                    let mut runs = Runs::new(self.cx, Newlines::Break);
                    runs.text(text);
                    let runs = runs.finish();
                    self.paragraph("Literal", jc, &runs);
                    Vec::new()
                }
                Task::CloseCell => {
                    if !self.out.ends_with("</w:p>") {
                        self.out.push_str("<w:p/>");
                    }
                    self.out.push_str("</w:tc>");
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
                // `toc-placement=preamble`: the TOC ends the preamble.
                if let Some(toc) = &block.toc {
                    children.push(Task::Toc(toc, None));
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
                // Ruby's caption and title stand above the box.
                self.title(block, "Block Title");
                self.anchor(block);
                let open = self.frame("Example Table", ctx, None);
                self.out.push_str(&open);
                self.framed(inherit(blocks, cell_ctx(ctx)))
            }
            BlockKind::Sidebar { blocks } => {
                self.anchor(block);
                let open = self.frame("Sidebar Table", ctx, None);
                self.out.push_str(&open);
                self.indent = 0;
                self.title(block, "Sidebar Title");
                self.framed(inherit(blocks, cell_ctx(ctx)))
            }
            BlockKind::Admonition {
                kind,
                label,
                blocks,
                ..
            } => {
                self.anchor(block);
                let specific = format!("Admonition {} Table", admonition_name(*kind));
                let style = if self.cx.styles.has(StyleKind::Table, &specific) {
                    specific
                } else {
                    "Admonition Table".to_string()
                };
                let available = self.text_width.saturating_sub(ctx.indent);
                let label_width = available / 6;
                let open = self.frame(&style, ctx, Some(label_width));
                self.out.push_str(&open);
                self.indent = 0;
                let mut runs = Runs::new(self.cx, Newlines::Space);
                runs.parts(label);
                let runs = runs.finish();
                self.paragraph("Admonition Label", "", &runs);
                self.out.push_str(&format!(
                    r#"</w:tc><w:tc><w:tcPr><w:tcW w:w="{}" w:type="dxa"/></w:tcPr>"#,
                    available - label_width
                ));
                // Ruby puts the title in the content cell.
                self.title(block, "Block Title");
                self.framed(inherit(blocks, cell_ctx(ctx)))
            }
            BlockKind::Quote {
                attribution,
                blocks,
            } => {
                self.title(block, "Block Title");
                self.anchor(block);
                let open = self.frame("Quote Table", ctx, None);
                self.out.push_str(&open);
                // The attribution follows the quoted blocks.
                let inner = Ctx {
                    quote: true,
                    ..cell_ctx(ctx)
                };
                let mut children = inherit(blocks, inner);
                if let Some(attribution) = attribution {
                    children.push(Task::Attribution(attribution, "Quote Attribution", inner));
                }
                self.framed(children)
            }
            BlockKind::Verse {
                attribution,
                inlines,
            } => {
                self.title(block, "Block Title");
                let open = self.frame("Verse Table", ctx, None);
                self.out.push_str(&open);
                self.indent = 0;
                self.inline_paragraph(block, "Verse", inlines, Newlines::Break);
                if let Some(attribution) = attribution {
                    self.attribution(attribution, "Verse Attribution");
                }
                self.framed(Vec::new())
            }
            BlockKind::Listing {
                text,
                callouts,
                inlines,
                ..
            } => {
                self.title(block, "Block Title");
                let open = self.frame("Code Table", ctx, None);
                self.out.push_str(&open);
                self.indent = 0;
                self.verbatim(block, "Code", text, callouts, inlines.as_deref());
                self.framed(Vec::new())
            }
            BlockKind::Literal {
                text,
                callouts,
                inlines,
            } => {
                self.title(block, "Block Title");
                let open = self.frame("Literal Table", ctx, None);
                self.out.push_str(&open);
                self.indent = 0;
                self.verbatim(block, "Literal", text, callouts, inlines.as_deref());
                self.framed(Vec::new())
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
            BlockKind::List {
                ordered,
                items,
                icons,
            } => {
                self.title(block, "Block Title");
                self.anchor(block);
                self.list(block, *ordered, items, icons.as_deref(), ctx)
            }
            BlockKind::CalloutList { items } => {
                self.title(block, "Block Title");
                self.anchor(block);
                items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| Task::Callout {
                        item,
                        number: i + 1,
                        ctx,
                    })
                    .collect()
            }
            BlockKind::DescriptionList { items } => {
                self.title(block, "Block Title");
                self.anchor(block);
                match block.attrs.style.as_deref() {
                    Some("horizontal") => self.horizontal_list(block, items, ctx),
                    Some("qanda") => self.qanda(items, ctx),
                    _ => {
                        let mut tasks = Vec::new();
                        for item in items {
                            tasks.push(Task::Terms {
                                terms: &item.terms,
                                style: "Term",
                                num: None,
                                ctx,
                            });
                            if let Some(description) = &item.description {
                                let inner = ctx.indent + DESCRIPTION_INDENT;
                                tasks.push(Task::Description {
                                    item: description,
                                    style: "Description",
                                    // At the top level the style indents.
                                    para_indent: if ctx.indent == 0 { 0 } else { inner },
                                    ctx: Ctx {
                                        indent: inner,
                                        ..ctx
                                    },
                                });
                            }
                        }
                        tasks
                    }
                }
            }
            BlockKind::Table(table) => self.table(block, table, ctx),
            BlockKind::Image { src, alt, .. } => {
                self.block_image(block, src, alt, ctx);
                Vec::new()
            }
            BlockKind::Video { src, target, .. } | BlockKind::Audio { src, target } => {
                self.cx
                    .warnings
                    .once("docx backend links video and audio instead of embedding them");
                self.title(block, "Block Title");
                self.anchor(block);
                let mut runs = Runs::new(self.cx, Newlines::Space);
                runs.link(src, |r| r.text(target));
                let runs = runs.finish();
                self.paragraph("Normal", "", &runs);
                Vec::new()
            }
            BlockKind::Toc => {
                if let Some(toc) = &block.toc {
                    self.anchor(block);
                    self.toc(toc, Some(block));
                }
                Vec::new()
            }
        }
    }

    /// An ordered or unordered list (or checklist): one numbering
    /// definition, a task per item.
    fn list(
        &mut self,
        block: &Block,
        ordered: bool,
        items: &'c [ListItem],
        icons: Option<&str>,
        ctx: Ctx,
    ) -> Vec<Task<'c>> {
        let option = |name: &str| {
            block.attrs.options.iter().any(|o| o == name)
                || block.attrs.named.contains_key(&format!("{name}-option"))
        };
        let style = block.attrs.style.as_deref();
        let checklist = !ordered && option("checklist");
        let left = ctx.indent + LIST_INDENT;
        let level = if checklist {
            Level {
                format: "none",
                text: String::new(),
                start: 1,
                left,
                no_suffix: true,
            }
        } else if ordered {
            if style == Some("lowergreek") {
                self.cx
                    .warnings
                    .once("docx backend numbers lowergreek lists with latin letters");
            }
            if option("reversed") {
                self.cx
                    .warnings
                    .once("docx backend does not support reversed lists");
            }
            let start = block
                .attrs
                .named
                .get("start")
                .and_then(|s| s.trim().parse::<i64>().ok())
                .map_or(1, |n| u32::try_from(n.max(0)).unwrap_or(1));
            match numbering::number_format(style) {
                Some(format) => Level {
                    format,
                    text: "%1.".into(),
                    start,
                    left,
                    no_suffix: false,
                },
                None => Level {
                    format: "none",
                    text: String::new(),
                    start,
                    left,
                    no_suffix: true,
                },
            }
        } else {
            match numbering::bullet(style, ctx.bullets) {
                Some(bullet) => Level {
                    format: "bullet",
                    text: bullet.into(),
                    start: 1,
                    left,
                    no_suffix: false,
                },
                None => Level {
                    format: "none",
                    text: String::new(),
                    start: 1,
                    left,
                    no_suffix: true,
                },
            }
        };
        let num = self.cx.numbering.add(&level);
        // Ruby's checkbox glyphs: text marks, or icon-font squares.
        let (checked, unchecked) = if icons == Some("font") {
            ("\u{2611} ", "\u{2610} ")
        } else {
            ("\u{2713} ", "\u{274f} ")
        };
        let inner = Ctx {
            indent: left,
            bullets: ctx.bullets + u8::from(!ordered),
            lead: false,
            ..ctx
        };
        items
            .iter()
            .map(|item| Task::Item {
                item,
                num,
                check: match item.checkbox.filter(|_| checklist) {
                    Some(true) => Some(checked),
                    Some(false) => Some(unchecked),
                    None => None,
                },
                ctx: inner,
            })
            .collect()
    }

    /// A list item's numbered paragraph; its blocks follow, indented.
    fn item(
        &mut self,
        item: &'c ListItem,
        num: u32,
        check: Option<&'static str>,
        ctx: Ctx,
    ) -> Vec<Task<'c>> {
        let mut runs = Runs::new(self.cx, Newlines::Space);
        if let Some(check) = check {
            runs.text(check);
        }
        runs.inlines(&item.inlines);
        let runs = runs.finish();
        self.indent = 0;
        self.paragraph_with("List Paragraph", Some(&list_number(num)), "", &runs);
        item.blocks.iter().map(|b| Task::Block(b, ctx)).collect()
    }

    /// A callout list item: the number in the `Callout` style, a tab to the
    /// hanging indent, the text; its blocks follow, indented.
    fn callout(&mut self, item: &'c CalloutListItem, number: usize, ctx: Ctx) -> Vec<Task<'c>> {
        let left = ctx.indent + LIST_INDENT;
        let mut runs = Runs::new(self.cx, Newlines::Space);
        runs.styled("Callout", &number.to_string());
        runs.text("\t");
        runs.inlines(&item.item.inlines);
        let runs = runs.finish();
        self.indent = 0;
        let ind = format!(
            r#"<w:ind w:left="{left}" w:hanging="{}"/>"#,
            numbering::HANGING
        );
        self.paragraph_with("List Paragraph", None, &ind, &runs);
        let inner = Ctx {
            indent: left,
            ..ctx
        };
        item.item
            .blocks
            .iter()
            .map(|b| Task::Block(b, inner))
            .collect()
    }

    /// A question and answer list (Ruby's `qanda`): numbered questions in
    /// `Question`, answers in `Answer`, aligned with the question text.
    fn qanda(&mut self, items: &'c [DescriptionListItem], ctx: Ctx) -> Vec<Task<'c>> {
        let left = ctx.indent + LIST_INDENT;
        let num = self.cx.numbering.add(&Level {
            format: "decimal",
            text: "%1.".into(),
            start: 1,
            left,
            no_suffix: false,
        });
        let inner = Ctx {
            indent: left,
            ..ctx
        };
        let mut tasks = Vec::new();
        for item in items {
            tasks.push(Task::Terms {
                terms: &item.terms,
                style: "Question",
                num: Some(num),
                ctx: inner,
            });
            if let Some(description) = &item.description {
                tasks.push(Task::Description {
                    item: description,
                    style: "Answer",
                    para_indent: left,
                    ctx: inner,
                });
            }
        }
        tasks
    }

    /// A horizontal description list: Ruby's two-column table, borderless,
    /// with `labelwidth`/`itemwidth` percentages.
    fn horizontal_list(
        &mut self,
        block: &Block,
        items: &'c [DescriptionListItem],
        ctx: Ctx,
    ) -> Vec<Task<'c>> {
        let available = self.text_width.saturating_sub(ctx.indent);
        let percent = |key: &str| {
            block
                .attrs
                .named
                .get(key)
                .and_then(|w| w.trim().trim_end_matches('%').parse::<u32>().ok())
                .filter(|w| (1..100).contains(w))
        };
        let (label, item, fixed) = match (percent("labelwidth"), percent("itemwidth")) {
            (Some(l), Some(i)) => (l, i, true),
            (Some(l), None) => (l, 100 - l, true),
            (None, Some(i)) => (100 - i, i, true),
            (None, None) => (25, 75, false),
        };
        let label_w = available * label / 100;
        let item_w = available * item / 100;
        let none = ["top", "left", "bottom", "right", "insideH", "insideV"]
            .iter()
            .map(|side| format!(r#"<w:{side} w:val="nil"/>"#))
            .collect::<String>();
        let layout = if fixed { "fixed" } else { "autofit" };
        let mut tasks = vec![Task::Markup(format!(
            r#"<w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/><w:tblInd w:w="{}" w:type="dxa"/><w:tblBorders>{none}</w:tblBorders><w:tblLayout w:type="{layout}"/><w:tblCellMar><w:left w:w="0" w:type="dxa"/><w:right w:w="160" w:type="dxa"/></w:tblCellMar><w:tblLook w:val="0000" w:firstRow="0" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="1" w:noVBand="1"/></w:tblPr><w:tblGrid><w:gridCol w:w="{label_w}"/><w:gridCol w:w="{item_w}"/></w:tblGrid>"#,
            ctx.indent
        ))];
        let cell = Ctx {
            indent: 0,
            lead: false,
            ..ctx
        };
        for entry in items {
            tasks.push(Task::Markup(format!(
                r#"<w:tr><w:tc><w:tcPr><w:tcW w:w="{label_w}" w:type="dxa"/></w:tcPr>"#
            )));
            tasks.push(Task::Terms {
                terms: &entry.terms,
                style: "Term",
                num: None,
                ctx: cell,
            });
            tasks.push(Task::CloseCell);
            tasks.push(Task::Markup(format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="{item_w}" w:type="dxa"/></w:tcPr>"#
            )));
            if let Some(description) = &entry.description {
                tasks.push(Task::Description {
                    item: description,
                    style: "Normal",
                    para_indent: 0,
                    ctx: cell,
                });
            }
            tasks.push(Task::CloseCell);
            tasks.push(Task::Markup("</w:tr>".into()));
        }
        tasks.push(Task::Markup("</w:tbl>".into()));
        tasks
    }

    /// Open the one-row frame table of a delimited block in table style
    /// `style`: one cell, or a label cell `label` twips wide and a content
    /// cell (an admonition). Returns the markup up to the (first) cell's
    /// content.
    fn frame(&mut self, style: &str, ctx: Ctx, label: Option<u32>) -> String {
        let mut available = self.text_width.saturating_sub(ctx.indent);
        // Quotes are inset by their style.
        if style == "Quote Table" || style == "Verse Table" {
            available = available.saturating_sub(360);
        }
        let style_id = self.cx.style(StyleKind::Table, style);
        let mut open = String::from("<w:tbl><w:tblPr>");
        if let Some(id) = style_id {
            open.push_str(&format!(
                r#"<w:tblStyle w:val="{}"/>"#,
                xml::escape_attr(&id)
            ));
        }
        open.push_str(&format!(r#"<w:tblW w:w="{available}" w:type="dxa"/>"#));
        if ctx.indent > 0 {
            open.push_str(&format!(r#"<w:tblInd w:w="{}" w:type="dxa"/>"#, ctx.indent));
        }
        let first_column = u8::from(label.is_some());
        open.push_str(&format!(
            r#"<w:tblLayout w:type="fixed"/><w:tblLook w:val="0000" w:firstRow="0" w:lastRow="0" w:firstColumn="{first_column}" w:lastColumn="0" w:noHBand="1" w:noVBand="1"/></w:tblPr><w:tblGrid>"#
        ));
        let first = label.unwrap_or(available);
        match label {
            Some(label) => open.push_str(&format!(
                r#"<w:gridCol w:w="{label}"/><w:gridCol w:w="{}"/>"#,
                available - label
            )),
            None => open.push_str(&format!(r#"<w:gridCol w:w="{available}"/>"#)),
        }
        open.push_str(&format!(
            r#"</w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="{first}" w:type="dxa"/></w:tcPr>"#
        ));
        open
    }

    /// The tasks of a frame's content, then the end of its table.
    fn framed(&mut self, mut content: Vec<Task<'c>>) -> Vec<Task<'c>> {
        content.push(Task::CloseCell);
        content.push(Task::Markup(format!("</w:tr></w:tbl>{SPACER}")));
        content
    }

    /// A table: Ruby's columns, spans, frame, grid, stripes, header and
    /// footer rows, cell styles and alignment, on the `Table` style.
    fn table(&mut self, block: &'c Block, table: &'c Table, ctx: Ctx) -> Vec<Task<'c>> {
        // Ruby's caption stands above the table.
        self.title(block, "Caption");
        self.anchor(block);
        let available = self.text_width.saturating_sub(ctx.indent);
        let option = |name: &str| {
            block.attrs.options.iter().any(|o| o == name)
                || block.attrs.named.contains_key(&format!("{name}-option"))
        };
        let autowidth = (option("autowidth") && !block.attrs.named.contains_key("width"))
            || table.columns.iter().any(|c| c.autowidth);
        let width = if autowidth {
            available
        } else {
            available * table.width.clamp(1, 100) / 100
        };
        let widths = table::column_widths(table, width);
        let mut open = String::from("<w:tbl><w:tblPr>");
        if let Some(id) = self.cx.style(StyleKind::Table, "Table") {
            open.push_str(&format!(
                r#"<w:tblStyle w:val="{}"/>"#,
                xml::escape_attr(&id)
            ));
        }
        if autowidth {
            open.push_str(r#"<w:tblW w:w="0" w:type="auto"/>"#);
        } else {
            open.push_str(&format!(r#"<w:tblW w:w="{width}" w:type="dxa"/>"#));
        }
        if ctx.indent > 0 {
            open.push_str(&format!(r#"<w:tblInd w:w="{}" w:type="dxa"/>"#, ctx.indent));
        }
        if let Some(borders) = table::borders(&table.frame, &table.grid) {
            open.push_str(&borders);
        }
        open.push_str(&format!(
            r#"<w:tblLayout w:type="{}"/><w:tblLook w:val="0000" w:firstRow="{}" w:lastRow="{}" w:firstColumn="0" w:lastColumn="0" w:noHBand="1" w:noVBand="1"/></w:tblPr><w:tblGrid>"#,
            if autowidth { "autofit" } else { "fixed" },
            u8::from(!table.head.is_empty()),
            u8::from(!table.foot.is_empty()),
        ));
        for w in &widths {
            open.push_str(&format!(r#"<w:gridCol w:w="{w}"/>"#));
        }
        open.push_str("</w:tblGrid>");
        let mut tasks = vec![Task::Markup(open)];
        let rows: Vec<(&[TableCell], Section)> = table
            .head
            .iter()
            .map(|r| (r.as_slice(), Section::Head))
            .chain(table.body.iter().map(|r| (r.as_slice(), Section::Body)))
            .chain(table.foot.iter().map(|r| (r.as_slice(), Section::Foot)))
            .collect();
        let slots = table::place(
            &rows.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            table.columns.len(),
        );
        let mut body_index = 0;
        for ((_, section), row) in rows.iter().zip(slots) {
            let striped = *section == Section::Body
                && match table.stripes.as_deref() {
                    Some("all") => true,
                    Some("even") => body_index % 2 == 1,
                    Some("odd") => body_index % 2 == 0,
                    _ => false,
                };
            if *section == Section::Body {
                body_index += 1;
            }
            let mut tr = String::from("<w:tr>");
            if *section == Section::Head {
                tr.push_str("<w:trPr><w:tblHeader/></w:trPr>");
            }
            tasks.push(Task::Markup(tr));
            for slot in row {
                let span_width: u32 = widths[slot.column()..slot.column() + slot.span()]
                    .iter()
                    .sum();
                let mut tc_pr = format!(r#"<w:tcW w:w="{span_width}" w:type="dxa"/>"#);
                if slot.span() > 1 {
                    tc_pr.push_str(&format!(r#"<w:gridSpan w:val="{}"/>"#, slot.span()));
                }
                match slot {
                    Slot::Cell { cell, merge, .. } => {
                        if merge {
                            tc_pr.push_str(r#"<w:vMerge w:val="restart"/>"#);
                        }
                        let fill = cell
                            .background
                            .as_deref()
                            .and_then(table::fill)
                            .or_else(|| striped.then(|| STRIPE.to_string()));
                        if let Some(fill) = fill {
                            tc_pr.push_str(&format!(
                                r#"<w:shd w:val="clear" w:color="auto" w:fill="{fill}"/>"#
                            ));
                        }
                        match cell.valign {
                            VAlign::Top => {}
                            VAlign::Middle => tc_pr.push_str(r#"<w:vAlign w:val="center"/>"#),
                            VAlign::Bottom => tc_pr.push_str(r#"<w:vAlign w:val="bottom"/>"#),
                        }
                        tasks.push(Task::Markup(format!("<w:tc><w:tcPr>{tc_pr}</w:tcPr>")));
                        tasks.extend(cell_content(cell, *section == Section::Head, ctx));
                        tasks.push(Task::CloseCell);
                    }
                    Slot::Continue { .. } => tasks.push(Task::Markup(format!(
                        r#"<w:tc><w:tcPr>{tc_pr}<w:vMerge/></w:tcPr><w:p/></w:tc>"#
                    ))),
                    Slot::Empty { .. } => tasks.push(Task::Markup(format!(
                        "<w:tc><w:tcPr>{tc_pr}</w:tcPr><w:p/></w:tc>"
                    ))),
                }
            }
            tasks.push(Task::Markup("</w:tr>".into()));
        }
        tasks.push(Task::Markup(format!("</w:tbl>{SPACER}")));
        tasks
    }

    /// A block image: a paragraph holding the picture (aligned by `align`,
    /// or floating with `float`), the caption below, like Ruby's figure.
    /// An image that cannot be read shows its alt text, with a warning.
    fn block_image(&mut self, block: &Block, src: &str, alt: &[AttributePart], ctx: Ctx) {
        self.anchor(block);
        let alt = crate::text::decode_references(&crate::inline::parts_text(alt));
        let src = crate::text::decode_references(src);
        let named = &block.attrs.named;
        let picture = self.cx.media.picture(&src, self.cx.rels, self.cx.allow_uri);
        let runs = match picture {
            Ok(picture) => {
                let available = self.text_width.saturating_sub(ctx.indent);
                let size = crate::media::draw_size(named, picture.pixels, available);
                let placement = match named.get("float").map(String::as_str) {
                    Some("left") => crate::media::Placement::Float("left"),
                    Some("right") => crate::media::Placement::Float("right"),
                    _ => crate::media::Placement::Inline,
                };
                let id = self.cx.media.doc_pr();
                let name = src.rsplit('/').next().unwrap_or_default().to_string();
                let drawing = crate::media::drawing(&picture, size, id, &name, &alt, placement);
                let mut runs = Runs::new(self.cx, Newlines::Space);
                match named.get("link") {
                    Some(link) => {
                        let link = crate::text::decode_references(link);
                        runs.link(&link, |r| r.markup(&drawing));
                    }
                    None => runs.markup(&drawing),
                }
                runs.finish()
            }
            Err(reason) => {
                self.cx.warnings.once(format!("{reason}: {src}"));
                let mut runs = Runs::new(self.cx, Newlines::Space);
                runs.text(&format!("[{alt}]"));
                runs.finish()
            }
        };
        let jc = match named.get("align").map(String::as_str) {
            Some("center") => r#"<w:jc w:val="center"/>"#,
            Some("right") => r#"<w:jc w:val="right"/>"#,
            _ => "",
        };
        // A captioned picture stays on the page of its caption.
        if block.title.is_some() {
            self.paragraph_keep_next("Normal", jc, &runs);
        } else {
            self.paragraph("Normal", jc, &runs);
        }
        self.title(block, "Caption");
    }

    /// A table of contents: Word's `TOC` field (a content control of the
    /// Table of Contents gallery) whose result is filled in with links to
    /// the sections core outlined, so it reads before Word updates it (page
    /// numbers come with the update). The title is the block's, else
    /// `toc-title`.
    fn toc(&mut self, toc: &adoctra_core::Toc, block: Option<&Block>) {
        let levels = toc.levels.clamp(1, 9);
        let mut runs = Runs::new(self.cx, Newlines::Space);
        match block.and_then(|b| b.title.as_ref()) {
            Some(title) => runs.inlines(title),
            None => match self.doc.attributes.raw_parts("toc-title") {
                Some(parts) => runs.parts(parts),
                None => runs.text(self.doc.attributes.get("toc-title").unwrap_or_default()),
            },
        }
        let title = runs.finish();
        // The listed sections: (depth, caption parts or heading, heading).
        let mut entries: Vec<(usize, &Block, Option<&[AttributePart]>)> = Vec::new();
        let mut stack: Vec<OutlineFrame> =
            vec![(sections(&self.doc.blocks), Some(&toc.entries[..]), 0)];
        while let Some((list, toc_entries, depth)) = stack.pop() {
            let mut list = list.into_iter().enumerate();
            if let Some((index, (level, heading, children))) = list.next() {
                let entry = toc_entries.and_then(|e| e.get(index));
                entries.push((depth + 1, heading, entry.and_then(|e| e.caption.as_deref())));
                let rest: Vec<_> = list.map(|(_, s)| s).collect();
                // The siblings after this section's subsections.
                stack.push((
                    rest,
                    toc_entries.map(|e| &e[(index + 1).min(e.len())..]),
                    depth,
                ));
                if i128::from(level) < levels {
                    stack.push((
                        sections(children),
                        entry.map(|e| &e.sections[..]),
                        depth + 1,
                    ));
                }
            }
        }
        let mut sdt = String::from(
            r#"<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val="Table of Contents"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent>"#,
        );
        let heading = simple_paragraph(self.cx, "TOC Heading", &title);
        sdt.push_str(&heading);
        let begin = format!(
            r#"<w:r><w:fldChar w:fldCharType="begin" w:dirty="true"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-{levels}" \h \z \u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#
        );
        let end = r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#;
        if entries.is_empty() {
            sdt.push_str(&format!("<w:p>{begin}{end}</w:p>"));
        }
        let count = entries.len();
        for (i, (depth, heading, caption)) in entries.into_iter().enumerate() {
            let mut runs = Runs::new(self.cx, Newlines::Space).outline();
            let id = heading.id.clone().unwrap_or_default();
            runs.anchor_link(&id, |r| {
                match caption {
                    Some(parts) => r.parts(parts),
                    None if !heading.caption_parts.is_empty() => r.parts(&heading.caption_parts),
                    None => {
                        if let Some(caption) = &heading.caption {
                            r.text(caption);
                        }
                    }
                }
                if let BlockKind::Heading { inlines, .. } = &heading.kind {
                    r.inlines(inlines);
                }
            });
            let mut runs = runs.finish();
            if i == 0 {
                runs.insert_str(0, &begin);
            }
            if i + 1 == count {
                runs.push_str(end);
            }
            let paragraph = simple_paragraph(self.cx, &format!("TOC {}", depth.min(9)), &runs);
            sdt.push_str(&paragraph);
        }
        sdt.push_str("</w:sdtContent></w:sdt>");
        self.out.push_str(&sdt);
    }

    /// The block's id becomes a bookmark in its next paragraph.
    fn anchor(&mut self, block: &Block) {
        if let Some(id) = &block.id {
            self.pending.push(id.clone());
        }
    }

    fn heading(&mut self, block: &'c Block, level: u8, inlines: &[Inline]) {
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
        // Ruby's `[index]` section: Word builds the index from the `XE`
        // fields when it updates the field.
        if block.attrs.style.as_deref() == Some("index") {
            let field = r#"<w:r><w:fldChar w:fldCharType="begin" w:dirty="true"/></w:r><w:r><w:instrText xml:space="preserve"> INDEX \c "2" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#;
            self.paragraph("Normal", "", field);
        }
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
    /// Callout marks (`callouts`, byte ranges of `text`) keep their line
    /// comment guard and show the number in the `Callout` style.
    fn verbatim(
        &mut self,
        block: &Block,
        style: &str,
        text: &str,
        callouts: &[Callout],
        inlines: Option<&[Inline]>,
    ) {
        self.anchor(block);
        if let Some(inlines) = inlines {
            let mut runs = Runs::new(self.cx, Newlines::Break);
            runs.inlines(inlines);
            let runs = runs.finish();
            self.paragraph(style, "", &runs);
            return;
        }
        let mut marks = callouts.iter().peekable();
        let mut start = 0;
        for line in text.split('\n') {
            let end = start + line.len();
            let mut runs = Runs::new(self.cx, Newlines::Break);
            let mut copied = start;
            while let Some(mark) = marks.next_if(|m| m.start >= start && m.end <= end) {
                runs.text(&text[copied..mark.start]);
                if mark.xml {
                    runs.text("<!--");
                } else if let Some(guard) = &mark.guard {
                    runs.text(guard);
                }
                runs.styled("Callout", &mark.number);
                if mark.xml {
                    runs.text("-->");
                }
                copied = mark.end;
            }
            runs.text(&text[copied..end]);
            let runs = runs.finish();
            self.paragraph(style, "", &runs);
            start = end + 1;
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

    /// A paragraph kept with the next (a picture with its caption below).
    fn paragraph_keep_next(&mut self, name: &str, extra_ppr: &str, runs: &str) {
        self.keep_next = true;
        self.paragraph_with(name, None, extra_ppr, runs);
        self.keep_next = false;
    }

    /// A paragraph in style `name` (a missing custom style falls back to
    /// the template's default paragraph style), indented like its
    /// container, with the pending bookmarks.
    fn paragraph(&mut self, name: &str, extra_ppr: &str, runs: &str) {
        self.paragraph_with(name, None, extra_ppr, runs);
    }

    /// A paragraph with list numbering `num` (`w:numPr`) or the container's
    /// indent. `extra_ppr` follows in schema order (`w:ind`, `w:jc`).
    fn paragraph_with(&mut self, name: &str, num: Option<&str>, extra_ppr: &str, runs: &str) {
        let id = self.cx.style(StyleKind::Paragraph, name);
        let mut ppr = String::new();
        if let Some(id) = &id {
            ppr.push_str(&format!(r#"<w:pStyle w:val="{}"/>"#, xml::escape_attr(id)));
        }
        if self.keep_next {
            ppr.push_str("<w:keepNext/>");
        }
        match num {
            Some(num) => ppr.push_str(num),
            // A style numbered by the template (a heading): core already
            // wrote the number (Ruby's), so Word's must not add another.
            None if id
                .as_deref()
                .is_some_and(|id| self.cx.styles.num_id(id).is_some()) =>
            {
                ppr.push_str(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="0"/></w:numPr>"#);
            }
            None => {}
        }
        if num.is_none() && self.indent > 0 && !extra_ppr.contains("<w:ind ") {
            ppr.push_str(&format!(r#"<w:ind w:left="{}"/>"#, self.indent));
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

/// A level of the outline walk: the sections left to list, the matching
/// entries of core's TOC and the depth.
type OutlineFrame<'b> = (
    Vec<(u8, &'b Block, &'b [Block])>,
    Option<&'b [adoctra_core::TocEntry]>,
    usize,
);

/// The sections among `blocks`: level, heading and content.
fn sections(blocks: &[Block]) -> Vec<(u8, &Block, &[Block])> {
    blocks
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Section { level, blocks } if !blocks.is_empty() => {
                Some((*level, &blocks[0], &blocks[1..]))
            }
            _ => None,
        })
        .collect()
}

/// A table row's section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Head,
    Body,
    Foot,
}

/// The context of a frame's or cell's content: no outer indent.
fn cell_ctx(ctx: Ctx) -> Ctx {
    Ctx {
        indent: 0,
        lead: false,
        ..ctx
    }
}

fn admonition_name(kind: AdmonitionKind) -> &'static str {
    match kind {
        AdmonitionKind::Note => "Note",
        AdmonitionKind::Tip => "Tip",
        AdmonitionKind::Important => "Important",
        AdmonitionKind::Warning => "Warning",
        AdmonitionKind::Caution => "Caution",
    }
}

/// The tasks of a table cell's content: paragraphs with the cell style's
/// formatting and alignment, a literal cell, or an AsciiDoc cell's blocks.
fn cell_content<'c>(cell: &'c TableCell, head: bool, ctx: Ctx) -> Vec<Task<'c>> {
    let jc = match cell.halign {
        HAlign::Left => "",
        HAlign::Center => r#"<w:jc w:val="center"/>"#,
        HAlign::Right => r#"<w:jc w:val="right"/>"#,
    };
    let format = match cell.style {
        _ if head => None,
        Some(CellStyle::Strong | CellStyle::Header) => Some(AttributeFormat::Strong),
        Some(CellStyle::Emphasis) => Some(AttributeFormat::Emphasis),
        Some(CellStyle::Monospaced) => Some(AttributeFormat::Monospace),
        _ => None,
    };
    match &cell.content {
        CellContent::Paragraphs(paragraphs) => paragraphs
            .iter()
            .map(|inlines| Task::Inlines {
                inlines,
                format,
                jc,
            })
            .collect(),
        CellContent::Literal(text) => vec![Task::Literal(text, jc)],
        CellContent::Blocks {
            blocks,
            has_title,
            attributes,
            ..
        } => {
            // The nested document's title shows unless `notitle` (its default).
            let skip = usize::from(*has_title && attributes.get("notitle").is_some());
            blocks[skip.min(blocks.len())..]
                .iter()
                .map(|b| Task::Block(b, cell_ctx(ctx)))
                .collect()
        }
    }
}

/// The `w:numPr` of a list's paragraph.
fn list_number(num: u32) -> String {
    format!(r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{num}"/></w:numPr>"#)
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
    fn text_width_is_the_page_minus_margins() {
        assert_eq!(
            text_width(
                r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:left="1134" w:right="1134" w:gutter="100"/></w:sectPr>"#
            ),
            9538
        );
        assert_eq!(text_width("<w:sectPr/>"), DEFAULT_TEXT_WIDTH);
        assert_eq!(
            text_width(r#"<w:sectPr><w:pgSz w:w="100"/><w:pgMar w:left="200"/></w:sectPr>"#),
            DEFAULT_TEXT_WIDTH
        );
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
