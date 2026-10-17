//! Parser: builds a [`ParseResult`] from source text.
//!
//! Error-tolerant by design: it never returns `Err` for malformed syntax.
//! Whatever can be parsed becomes blocks (with spans); problems become
//! [`Diagnostic`]s. This is what lets an editor parse on every keystroke while
//! the buffer is half-written.

use crate::ast::{
    AdmonitionKind, Attribution, Block, BlockAttrs, BlockKind, Document, Inline, ListItem, Span,
};
use crate::attributes::Attributes;
use crate::block_attributes::parse_block_attr_list;
use crate::comments::{comment_fence, is_line_comment, skip_comment};
use crate::context::{consume_attribute_entry, ParseContext, ParseOptions};
use crate::diagnostic::{Diagnostic, ParseResult, Severity};
use crate::header::{collect_header, setext_level};
use crate::inline::scan_inlines_report;
use crate::reader::Reader;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Metadata accumulated from `[[id]]` anchor and `[style, #id.role, name=value]`
/// attribute lines that precede a block, transferred onto that block once it is
/// parsed. `id` is last-wins across all forms; `roles` accumulate in source
/// order; `attrs` merges across consecutive lines (style last-wins, named/options
/// accumulate).
#[derive(Default)]
pub(crate) struct PendingMeta {
    pub id: Option<String>,
    pub reftext: Option<crate::reftext::Read>,
    pub roles: Vec<String>,
    pub title: Option<PendingTitle>,
    pub attrs: BlockAttrs,
    pub list: crate::block_attributes::BlockList,
}

/// A pending block title from a `.Title` line or a `title=` attribute. Source
/// text starts at `base`; a value transformed by the attribute-list a-stage
/// has no exact offsets, so its nodes and diagnostics take `origin`.
#[derive(Clone)]
pub(crate) struct PendingTitle {
    pub text: String,
    pub base: usize,
    pub origin: Option<Span>,
}

/// The metadata fields of a block macro's attribute list: Ruby merges them
/// into the block attributes over the preceding metadata lines.
#[derive(Default)]
pub(crate) struct MacroMeta {
    pub id: Option<String>,
    pub roles: Option<Vec<String>>,
    pub title: Option<PendingTitle>,
    pub reftext: Option<crate::reftext::Read>,
    pub named: Vec<(String, String)>,
    pub options: Vec<String>,
}

impl MacroMeta {
    /// The fields of a macro list read like an attribute line.
    fn from_list(list: PendingMeta) -> Self {
        MacroMeta {
            id: list.id,
            roles: (!list.roles.is_empty()).then_some(list.roles),
            title: list.title,
            reftext: list.reftext,
            named: list.attrs.named.into_iter().collect(),
            options: list.attrs.options,
        }
    }

    /// Replace the metadata of a block without preceding metadata lines (a
    /// list continuation).
    fn apply_alone(self, block: &mut Block) {
        block.id = self.id;
        block.roles = self.roles.unwrap_or_default();
        block.attrs.named.extend(self.named);
        block.attrs.options = self.options;
    }
}

/// Ruby's `:image` branch of `next_block`: the macro list (positional `alt`,
/// `width`, `height`) gets the early a-stage like an attribute line, then the
/// target its attribute references. A target emptied by `drop-line` drops
/// the block and its metadata.
fn read_image_macro(
    line: &str,
    off: usize,
    (target, list): (std::ops::Range<usize>, std::ops::Range<usize>),
    context: &mut ParseContext,
    includes: &crate::xref_target::Includes,
) -> Option<Block> {
    let attr_line = crate::block_attributes::AttrLine {
        inner: line[list.clone()].to_owned(),
        base: off + list.start,
    };
    let (meta, _) = crate::block_attributes::macro_list(
        attr_line,
        &["alt", "width", "height"],
        context,
        includes,
    );
    let mut value = line[target.clone()].to_owned();
    if value.contains('{') {
        let source = crate::image::target_text(&value, off + target.start);
        let (expanded, refs) = context.block_attribute_line(source);
        if expanded.text.is_empty() && refs.iter().any(|r| r.drops_line) {
            return None;
        }
        value = expanded.text;
    }
    context.block_macro = Some(MacroMeta::from_list(meta));
    Some(Block::new(
        Span::new(off, off + line.len()),
        BlockKind::Image {
            target: value,
            src: String::new(),
            alt: Vec::new(),
        },
    ))
}

/// Complete an image once its metadata is merged: Ruby drops the style
/// (which becomes the alt text without an explicit one) and makes a unitless
/// `scaledwidth` a percentage.
fn complete_image(block: &mut Block, attributes: &Attributes) {
    let BlockKind::Image { target, src, alt } = &mut block.kind else {
        return;
    };
    let style = block.attrs.style.take();
    let (text, default) = match (block.attrs.named.get("alt"), style) {
        (Some(alt), _) => (alt.clone(), false),
        (None, Some(style)) => (style, false),
        (None, None) => (crate::image::default_alt(target), true),
    };
    *alt = crate::image::alt_parts(&text, default);
    if let Some(width) = block.attrs.named.remove("scaledwidth") {
        if !width.is_empty() {
            let width = crate::image::scaled_width(&width);
            block.attrs.named.insert("scaledwidth".to_owned(), width);
        }
    }
    *src = crate::image::web_path(target, attributes.get("imagesdir"));
}

/// Ruby's `BlockTocMacroRx` (`^toc::\[(.+)?\]$`). Like its
/// `parse_attributes` without `sub_input`, the list gets no substitutions;
/// positional fields have no meaning for this macro.
pub(crate) fn parse_toc_macro(line: &str, off: usize) -> Option<MacroMeta> {
    let inner = line.strip_prefix("toc::[")?.strip_suffix(']')?;
    let base = off + "toc::[".len();
    let mut meta = MacroMeta::default();
    for field in crate::attribute_list::parse(inner) {
        let (Some(name), Some(text)) = (field.name.clone(), field.text(inner)) else {
            continue;
        };
        match &inner[name] {
            "id" => meta.id = Some(text),
            "role" => meta.roles = Some(text.split_whitespace().map(str::to_owned).collect()),
            "title" => {
                let range = field.value.clone().unwrap();
                let exact = inner[range.clone()] == text;
                meta.title = Some(PendingTitle {
                    text,
                    base: base + range.start,
                    origin: (!exact).then(|| Span::new(base + range.start, base + range.end)),
                });
            }
            name => meta.named.push((name.to_owned(), text)),
        }
    }
    Some(meta)
}

pub fn parse_with_options(source: &str, options: &ParseOptions) -> ParseResult {
    parse_reader(options, Reader::new(source)).0
}

pub(crate) fn parse_reader<'a>(
    options: &ParseOptions,
    mut lines: Reader<'a>,
) -> (ParseResult, Reader<'a>) {
    lines.set_primary_id(options.source_id);
    let mut context = ParseContext::new(options);
    let mut header = collect_header(&mut lines, &mut context);
    let mut doc = Document {
        attributes: (*context.attributes).clone(),
        header: header.metadata,
        blocks: Vec::new(),
        has_title: header.effective_title.is_some(),
    };
    let mut diagnostics = Vec::new();
    if let Some(effective) = header.effective_title {
        let inlines = context
            .body_subs
            .defer_header(&effective, &context.attributes);
        // Resolve title text against the complete header, but retain the level
        // identified before subsequent header attributes changed leveloffset.
        let mut title = Block::new(
            doc.header
                .as_ref()
                .map_or(effective.span, |header| header.title_span),
            BlockKind::Heading { level: 0, inlines },
        );
        if header.metadata_for_title {
            let metadata = std::mem::take(&mut header.pending);
            title.id = metadata.id.or(header.title_anchor.take());
            if let Some(value) = metadata.reftext {
                context.attach_reftext(&mut title, value);
            }
        }
        title.attribute_context = Arc::clone(&context.attributes);
        doc.blocks.push(title);
    }
    context.start_body();
    let book = context.book;
    let mut i = header.body_start;
    let body = collect_blocks(
        &mut lines,
        &mut i,
        &mut context,
        &mut diagnostics,
        None,
        header.pending,
    );
    context.close_sections(0, usize::MAX);
    let body = nest_sections(
        body,
        doc.has_title,
        book,
        doc.attributes.get("preface-title").filter(|_| book),
        &mut context.sections,
        &mut diagnostics,
    );
    check_open_styles(&body, OpenParent::Document, book, &mut diagnostics);
    doc.blocks.extend(body);

    let mut references = Vec::new();
    let registry = context.register_references(&mut doc, &lines.include_catalog, &mut references);
    context.execute_body(&mut doc, registry, &lines.include_catalog, &mut references);

    // All grammar operates on monotonically increasing reader coordinates.
    // Restore physical source ranges before resolution copies metadata spans.
    struct Origin<'a, 'b> {
        reader: &'a Reader<'b>,
        provenance: &'a mut Vec<Vec<Span>>,
    }
    impl crate::visit::VisitorMut for Origin<'_, '_> {
        fn visit_span(&mut self, span: &mut Span) {
            *span = self.reader.map_span(*span, self.provenance);
        }
    }
    let mut provenance = Vec::new();
    crate::visit::walk_document_mut(
        &mut Origin {
            reader: &lines,
            provenance: &mut provenance,
        },
        &mut doc,
    );
    lines.finish();
    diagnostics.append(&mut lines.diagnostics);
    diagnostics.append(&mut context.comment_diagnostics);
    diagnostics.append(&mut context.inline_diagnostics);
    context.comments.sort_by_key(|span| span.start);
    lines.comments = context.comments.clone();
    for diagnostic in &mut diagnostics {
        diagnostic.span = lines.map_span(diagnostic.span, &mut provenance);
    }
    // Selection warnings already refer to physical lines, including omitted tags.
    diagnostics.append(&mut lines.resource_diagnostics);
    for mut diagnostic in references {
        diagnostic.span = lines.map_span(diagnostic.span, &mut provenance);
        diagnostics.push(diagnostic);
    }
    let physical = |spans: &[Span]| {
        spans
            .iter()
            .flat_map(|span| lines.original_spans(*span))
            .collect()
    };
    let comments = physical(&context.comments);
    let conditional_directives = physical(&lines.directives);
    let inactive = physical(&lines.inactive);
    let include_directives = physical(&lines.includes);
    (
        ParseResult {
            document: doc,
            diagnostics,
            comments,
            conditional_directives,
            inactive,
            include_directives,
            provenance,
        },
        lines,
    )
}

/// Collect a run of blocks, draining `lines` from `*i` until `stop` matches a
/// non-blank line (the caller consumes that line) or input ends. This is the
/// shared body of the document and of every delimited container (e.g. an example
/// block's `====` … `====`), so block metadata (`[[id]]` anchors, `[#id.role]`
/// shorthand, `[style]` lines) and type-changing style fixups work at any depth.
///
/// A `[[id]]` / `[[id,reftext]]` anchor line *or* a `[#id.role]` shorthand
/// attribute line attaches metadata to the next block. asciidoctor lets blank
/// lines sit between such a line and its block, so we keep the metadata *pending*
/// and let the loop skip blanks. The id is last-wins across both forms; roles
/// accumulate in source order across consecutive lines.
fn collect_blocks(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    diagnostics: &mut Vec<Diagnostic>,
    stop: Option<&str>,
    mut pending: PendingMeta,
) -> Vec<Block> {
    // Ruby first preprocesses and captures a container up to its closing fence,
    // then parses the body. A nested comment/verbatim region cannot hide that fence.
    let previous_limit = stop.map(|fence| lines.bound(*i, context, |line| line.trim() == fence));
    let mut blocks = Vec::new();
    while lines.has(*i, context) {
        let (_, line_buffer) = lines.line(*i);
        let line = line_buffer.as_ref();
        if line.trim().is_empty() {
            *i += 1;
            continue;
        }
        if consume_block_metadata(lines, i, context, &mut pending) {
            continue;
        }
        let metadata = std::mem::take(&mut pending);
        let mut carried = None;
        if let Some(mut block) = parse_block(
            lines,
            i,
            context,
            diagnostics,
            if stop.is_none() {
                BlockCtx::Section
            } else {
                BlockCtx::TopLevel
            },
            Some(&metadata.attrs),
            metadata.id.is_some(),
        ) {
            let pending = metadata;
            // A section-title anchor supplies the id only without one from
            // metadata, but its reftext replaces a pending reftext.
            block.id = pending.id.or(block.id.take());
            let mut reftext = context.section_reftext.take().or(pending.reftext);
            block.roles = pending.roles;
            let mut title = pending.title;
            let mut macro_meta = context.block_macro.take();
            if let Some(meta) = &mut macro_meta {
                if let Some(value) = meta.reftext.take() {
                    reftext = Some(value);
                }
                if let Some(id) = &meta.id {
                    block.id = Some(id.clone());
                }
                if let Some(roles) = &meta.roles {
                    block.roles.clone_from(roles);
                }
                if meta.title.is_some() {
                    title.clone_from(&meta.title);
                }
            }
            if matches!(block.kind, BlockKind::Heading { .. }) {
                // Ruby keeps a block title for the next content block rather
                // than a section; a discrete heading discards it.
                let discrete = matches!(pending.attrs.style.as_deref(), Some("discrete" | "float"));
                carried = title.take().filter(|_| !discrete);
            }
            block.title = title.as_ref().map(|title| {
                vec![Inline::Text {
                    value: title.text.clone(),
                    span: title
                        .origin
                        .unwrap_or(Span::new(title.base, title.base + title.text.len())),
                }]
            });
            block.attrs = pending.attrs;
            if let Some(meta) = macro_meta {
                block.attrs.named.extend(meta.named);
                for option in meta.options {
                    if !block.attrs.options.contains(&option) {
                        block.attrs.options.push(option);
                    }
                }
            }
            complete_image(&mut block, &context.attributes);
            let attributes = inline_attributes(&context.render_attributes(), Some(&block.attrs));
            apply_style_fixup(&mut block, lines, &attributes, &pending.list, context);
            if let Some(value) = reftext {
                context.attach_reftext(&mut block, value);
            }
            // Caption counters are assigned on completion, before an anchored
            // title is cached. Unanchored titles run during conversion instead.
            context.assign_caption(&mut block);
            if let Some(title) = title {
                if block.id.is_some() && title.text.contains('{') {
                    let span = title
                        .origin
                        .unwrap_or(Span::new(title.base, title.base + title.text.len()));
                    block.title = Some(context.attribute_value(
                        &title.text,
                        span,
                        title.origin.is_none(),
                        &lines.include_catalog,
                    ));
                } else if !matches!(block.kind, BlockKind::Passthrough { .. }) {
                    block.title = Some(context.body_subs.defer_title(
                        &title.text,
                        title.base,
                        title.origin,
                        &context.attributes,
                    ));
                }
                // An unrendered passthrough title retains its source text; it
                // must neither execute expressions nor issue inline warnings.
            }
            block.attribute_context = Arc::clone(&context.attributes);
            blocks.push(block);
        }
        if let Some(title) = carried.take() {
            pending.title = Some(title);
        }
    }
    if let Some(limit) = previous_limit {
        lines.limit = limit;
    }
    blocks
}

/// Ruby's `next_section`: nest the document-level blocks under their section
/// titles. A title opens a subsection of the innermost open section with a lower
/// level and closes the others; levels that skip one warn, and a level-0 section
/// outside a book is an error but still nests under the document. Blocks before
/// the first section form a preamble when the document has a title, unless there
/// are no sections at all (except for a book, as in Ruby).
fn nest_sections(
    blocks: Vec<Block>,
    has_header: bool,
    book: bool,
    preface_title: Option<&str>,
    sections: &mut BTreeMap<(usize, usize), crate::section_numbers::Section>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Block> {
    use crate::section_numbers::Ordinals;
    // Close the innermost open section into its parent (or the top level).
    // Like Ruby, a book part must end with a nested section.
    fn close(
        open: &mut Vec<Block>,
        top: &mut Vec<Block>,
        book: bool,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let mut section = open.pop().unwrap();
        if let BlockKind::Section { level, blocks } = &section.kind {
            let last = blocks.last().unwrap();
            if book && *level == 0 && !matches!(last.kind, BlockKind::Section { .. }) {
                diagnostics.push(Diagnostic {
                    span: blocks[0].span,
                    severity: Severity::Error,
                    message: "invalid part, must have at least one section (e.g., chapter, appendix, etc.)"
                        .into(),
                });
            }
            section.span.end = section.span.end.max(last.span.end);
            section.attribute_context = Arc::clone(&last.attribute_context);
        }
        match open.last_mut() {
            Some(Block {
                kind: BlockKind::Section { blocks, .. },
                ..
            }) => blocks.push(section),
            _ => top.push(section),
        }
    }
    fn section_level(block: &Block) -> Option<u8> {
        match block.kind {
            BlockKind::Heading { level, .. }
                if !matches!(block.attrs.style.as_deref(), Some("discrete" | "float")) =>
            {
                Some(level)
            }
            _ => None,
        }
    }
    // Without a header, a body that starts with a section title makes Ruby
    // create every top-level section directly, without checking its level.
    let unchecked = !has_header && blocks.first().and_then(section_level).is_some();
    // Ruby also opens a preamble for a book without a header, unless its first
    // block is an abstract.
    let preamble_open = has_header
        || (book
            && blocks.first().map_or(true, |block| {
                block.attrs.style.as_deref() != Some("abstract")
            }));
    let mut top = Vec::new();
    let mut preamble = Vec::new();
    // Open sections, innermost last, with their children's ordinals and
    // their own `sectnum` path.
    let mut open: Vec<Block> = Vec::new();
    let mut numbering: Vec<(Ordinals, Vec<String>, Vec<usize>)> = Vec::new();
    // The partintro state of each open section (only parts use it).
    let mut intros: Vec<Intro> = Vec::new();
    let mut top_ordinals = Ordinals::default();
    for block in blocks {
        let level = match section_level(&block) {
            Some(level) => level,
            None => {
                match open.last_mut() {
                    Some(Block {
                        kind: BlockKind::Section { level: 0, blocks },
                        ..
                    }) if book => {
                        part_content(blocks, intros.last_mut().unwrap(), block, diagnostics)
                    }
                    Some(Block {
                        kind: BlockKind::Section { blocks, .. },
                        ..
                    }) => blocks.push(block),
                    _ if preamble_open => preamble.push(block),
                    _ => top.push(block),
                }
                continue;
            }
        };
        // The title line's level decides nesting; a special section's own
        // level may differ (see section_numbers.rs).
        let key = (block.span.start, block.span.end);
        let raw = sections
            .get(&key)
            .map_or(level, |section| section.raw_level);
        while matches!(
            open.last(),
            Some(Block { kind: BlockKind::Section { level: current, .. }, .. }) if raw <= *current
        ) {
            close(&mut open, &mut top, book, diagnostics);
            numbering.pop();
            intros.pop();
        }
        let parent = open.last().map(|parent| match &parent.kind {
            BlockKind::Section { level, blocks } => (
                *level,
                sections.get(&(blocks[0].span.start, blocks[0].span.end)),
            ),
            _ => unreachable!("open blocks are sections"),
        });
        let expected = match parent {
            Some((_, Some(parent))) if !parent.allows_nesting() => {
                diagnostics.push(Diagnostic {
                    span: block.span,
                    severity: Severity::Error,
                    message: format!(
                        "{} sections do not support nested sections",
                        parent.sectname
                    ),
                });
                None
            }
            Some((level, _)) => Some(level.saturating_add(1)),
            _ if unchecked => None,
            _ if raw == 0 => {
                if !book {
                    diagnostics.push(Diagnostic {
                        span: block.span,
                        severity: Severity::Error,
                        message: "level 0 sections can only be used when doctype is book".into(),
                    });
                }
                None
            }
            _ => Some(1),
        };
        if let Some(expected) = expected.filter(|expected| *expected != raw) {
            let condition = if book && open.is_empty() {
                format!("expected levels 0 or {expected}")
            } else {
                format!("expected level {expected}")
            };
            diagnostics.push(Diagnostic {
                span: block.span,
                severity: Severity::Warning,
                message: format!("section title out of sequence: {condition}, got level {raw}"),
            });
        }
        // Ruby's `assign_numeral` among the parent's sections.
        let (mut path, mut closes) = (Vec::new(), Vec::new());
        if let Some(section) = sections.get_mut(&key) {
            let ordinals = numbering
                .last_mut()
                .map_or(&mut top_ordinals, |(ordinals, ..)| ordinals);
            let numeral = ordinals.assign(section);
            if let (true, Some((_, parent, parent_closes))) = (level > 1, numbering.last()) {
                path.clone_from(parent);
                closes.clone_from(parent_closes);
            }
            path.push(numeral.unwrap_or_default());
            closes.push(section.closed_at);
            section.path.clone_from(&path);
            section.closes.clone_from(&closes);
        }
        numbering.push((Ordinals::default(), path, closes));
        intros.push(Intro::None);
        let (span, context) = (block.span, Arc::clone(&block.attribute_context));
        let blocks = vec![block];
        let mut section = Block::new(span, BlockKind::Section { level, blocks });
        section.attribute_context = context;
        open.push(section);
    }
    while !open.is_empty() {
        close(&mut open, &mut top, book, diagnostics);
    }
    if preamble.is_empty() {
        return top;
    }
    // Ruby unwraps a standalone preamble (a document without sections), except
    // for a book.
    if top.is_empty() && !book {
        return preamble;
    }
    let last = preamble.last().unwrap();
    let span = Span {
        end: last.span.end,
        ..preamble[0].span
    };
    let context = Arc::clone(&last.attribute_context);
    let mut wrapper = Block::new(span, BlockKind::Preamble { blocks: preamble });
    wrapper.attribute_context = context;
    // Ruby's `preface-title` from the header names a book's preamble. HTML
    // never displays it, so its source value is kept without substitutions.
    wrapper.title = preface_title.map(|value| {
        vec![Inline::Text {
            value: value.to_string(),
            span: Span {
                end: span.start,
                ..span
            },
        }]
    });
    let mut result = vec![wrapper];
    result.extend(top);
    result
}

/// Ruby's partintro bookkeeping for an open section: whether later content
/// joins an intro container, or the part starts with an `[open]` paragraph
/// that Ruby rebuilds into one when a second block arrives.
#[derive(Clone, Copy, PartialEq)]
enum Intro {
    None,
    Container,
    OpenParagraph,
}

/// Add a content block to a book part, wrapping its intro in a `partintro`
/// open block like Ruby's `next_section`. `blocks[0]` is the part title.
fn part_content(
    blocks: &mut Vec<Block>,
    intro: &mut Intro,
    mut block: Block,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let partintro = || Some("partintro".to_string());
    if blocks.len() == 1 {
        let style = block.attrs.style.as_deref();
        match (&block.kind, style) {
            // A `[partintro]` paragraph is already an open block holding it.
            (_, Some("partintro")) => {}
            (BlockKind::Open { blocks: children }, None | Some("open")) => {
                // adoctra wraps an `[open]` paragraph in an open block with
                // the paragraph's span; Ruby keeps one simple block.
                if let [child] = children.as_slice() {
                    if child.span == block.span && matches!(child.kind, BlockKind::Paragraph { .. })
                    {
                        *intro = Intro::OpenParagraph;
                    }
                }
                block.attrs.style = partintro();
            }
            _ => {
                let mut wrapper = Block::new(block.span, BlockKind::Open { blocks: Vec::new() });
                wrapper.attrs.style = partintro();
                blocks.push(wrapper);
                *intro = Intro::Container;
            }
        }
    } else if blocks.len() == 2 {
        match *intro {
            Intro::None => diagnostics.push(Diagnostic {
                span: block.span,
                severity: Severity::Error,
                message: "illegal block content outside of partintro block".into(),
            }),
            // Ruby moves the `[open]` paragraph, metadata included, into a new
            // partintro container as an ordinary paragraph.
            Intro::OpenParagraph => {
                let first = &mut blocks[1];
                if let BlockKind::Open { blocks: children } = &mut first.kind {
                    let paragraph = &mut children[0];
                    paragraph.id = first.id.take();
                    paragraph.reftext = first.reftext.take();
                    paragraph.roles = std::mem::take(&mut first.roles);
                    paragraph.title = first.title.take();
                    paragraph.attrs = BlockAttrs {
                        style: None,
                        ..std::mem::take(&mut first.attrs)
                    };
                    first.attrs.style = partintro();
                }
                *intro = Intro::Container;
            }
            Intro::Container => {}
        }
    }
    if *intro == Intro::Container {
        let wrapper = blocks.last_mut().unwrap();
        wrapper.span.end = block.span.end;
        wrapper.attribute_context = Arc::clone(&block.attribute_context);
        if let BlockKind::Open { blocks } = &mut wrapper.kind {
            blocks.push(block);
        }
    } else {
        blocks.push(block);
    }
}

/// Where an open block sits, for Ruby's converter checks of its style.
#[derive(Clone, Copy, PartialEq)]
enum OpenParent {
    Document,
    Part,
    Other,
}

/// Report the abstract and partintro blocks that Ruby's HTML converter
/// excludes: an abstract directly in a book document (no preamble) and a
/// partintro outside a book part. Their content is not visited.
fn check_open_styles(
    blocks: &[Block],
    parent: OpenParent,
    book: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for block in blocks {
        let children = match &block.kind {
            BlockKind::Open { blocks } => {
                let excluded = match block.attrs.style.as_deref() {
                    Some("abstract") if book && parent == OpenParent::Document => Some((
                        Severity::Warning,
                        "abstract block cannot be used in a document without a doctitle when doctype is book. Excluding block content.",
                    )),
                    Some("partintro") if !book || parent != OpenParent::Part => Some((
                        Severity::Error,
                        "partintro block can only be used when doctype is book and must be a child of a book part. Excluding block content.",
                    )),
                    _ => None,
                };
                if let Some((severity, message)) = excluded {
                    diagnostics.push(Diagnostic {
                        span: block.span,
                        severity,
                        message: message.into(),
                    });
                    continue;
                }
                blocks
            }
            BlockKind::Section { level: 0, blocks } => {
                check_open_styles(blocks, OpenParent::Part, book, diagnostics);
                continue;
            }
            BlockKind::List { items, .. } => {
                for item in items {
                    check_open_styles(&item.blocks, OpenParent::Other, book, diagnostics);
                }
                continue;
            }
            BlockKind::Admonition { blocks, .. }
            | BlockKind::Example { blocks }
            | BlockKind::Quote { blocks, .. }
            | BlockKind::Sidebar { blocks }
            | BlockKind::Section { blocks, .. }
            | BlockKind::Preamble { blocks } => blocks,
            _ => continue,
        };
        check_open_styles(children, OpenParent::Other, book, diagnostics);
    }
}

/// Consume metadata once, both before header detection and at body boundaries.
pub(crate) fn consume_block_metadata(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    pending: &mut PendingMeta,
) -> bool {
    let (_, line_buffer) = lines.line(*i);
    let line = line_buffer.as_ref();
    if skip_comment(lines, i, context) {
        return true;
    }
    if consume_attribute_entry(lines, i, context) {
        return true;
    }
    if let Some((id, reftext)) = parse_block_anchor(line) {
        pending.id = Some(id.to_string());
        if let Some(rt) = reftext {
            let value = context.block_reftext(rt, lines[*i].0 + abs_offset(line, rt));
            lines
                .metadata_references
                .extend(value.references.iter().copied());
            pending.reftext = Some(value);
        }
        *i += 1;
        return true;
    }
    if let Some(parsed) = parse_block_attr_list(line, lines[*i].0) {
        let span = Span::new(lines[*i].0, lines[*i].0 + line.len());
        let references =
            crate::block_attributes::merge(pending, parsed, span, context, &lines.include_catalog);
        lines.metadata_references.extend(references);
        *i += 1;
        return true;
    }
    // A `.Title` line sets the next block's title (asciidoctor's
    // [`BlockTitleRx`]). Like the anchor / attribute-list lines it is *pending*
    // metadata that attaches to the block that follows (blank lines may sit
    // between). Keep the source until block completion determines whether
    // to cache the inlines or defer them to conversion. A second `.Title`
    // overrides the first without evaluating it (last-wins).
    if let Some(title) = parse_block_title(line) {
        pending.title = Some(PendingTitle {
            text: title.into(),
            base: lines[*i].0 + abs_offset(line, title),
            origin: None,
        });
        *i += 1;
        return true;
    }
    false
}

/// Apply a type-changing block *style* to a freshly parsed block. A `[literal]`,
/// `[example]` or admonition (`[NOTE]`, …) style on a *paragraph* rewrites its
/// kind; on any other block kind these styles are inert (left on `attrs.style`).
fn apply_style_fixup(
    block: &mut Block,
    lines: &Reader<'_>,
    attrs: &Attributes,
    list: &crate::block_attributes::BlockList,
    context: &mut ParseContext,
) {
    let child_attributes = Arc::clone(&block.attribute_context);
    let new_child = |span, kind| {
        let mut child = Block::new(span, kind);
        child.attribute_context = Arc::clone(&child_attributes);
        child
    };
    let Some(style) = block.attrs.style.as_deref() else {
        return;
    };
    // An uppercase admonition style (`[NOTE]`, …) wraps the block's body into an
    // admonition container. On a paragraph the already-scanned inlines become a
    // single child paragraph; on a `====` example or `____` quote block its
    // children move over unchanged. Unlike `[literal]`/`[example]` below it also
    // fires on a container, so it is handled before the paragraph-only guard.
    if let Some(kind) = AdmonitionKind::from_style(style) {
        match &mut block.kind {
            BlockKind::Paragraph { inlines } => {
                let inlines = std::mem::take(inlines);
                let child = new_child(block.span, BlockKind::Paragraph { inlines });
                block.kind = BlockKind::Admonition {
                    kind,
                    label: context.admonition_label(kind, Some(&block.attrs)),
                    blocks: vec![child],
                };
            }
            // A `====` example, a `____` quote and a `--` open block all fold
            // their parsed children into the admonition. A `****` sidebar is the
            // sole exception (`[NOTE]` on `****` is inert), so it is absent here.
            BlockKind::Example { blocks }
            | BlockKind::Quote { blocks, .. }
            | BlockKind::Open { blocks } => {
                let blocks = std::mem::take(blocks);
                block.kind = BlockKind::Admonition {
                    kind,
                    label: context.admonition_label(kind, Some(&block.attrs)),
                    blocks,
                };
            }
            _ => {}
        }
        return;
    }
    // Paragraph-only type-changing styles (`[literal]`, `[example]`, `[quote]`,
    // `[verse]`). A `____` fence already produced a `Quote`/`Verse` in
    // `parse_block`; these arms cover the *paragraph* shorthand. On any other
    // block kind the style is inert (left on `attrs.style`).
    if matches!(block.kind, BlockKind::Paragraph { .. }) {
        match style {
            // The literal body is the reader text, not the already-scanned
            // inlines (which are discarded).
            "literal" => {
                let text = lines.content(block.span.start, block.span.end).text;
                block.kind = BlockKind::Literal { text };
            }
            // `[pass]` on a paragraph: the raw source slice, like `[literal]` but
            // emitted unescaped and unwrapped by the backend (the inlines are
            // discarded). The delimited `++++` and `--` masquerade forms are built
            // directly at their fences in `parse_block`.
            "pass" => {
                let text = crate::comments::filtered_text(
                    &lines.content(block.span.start, block.span.end).text,
                    0,
                )
                .text;
                block.kind = BlockKind::Passthrough { text };
            }
            // `[example]` on a paragraph wraps it into a single-child example block.
            // The container keeps the block's id/roles/attrs; the child is a plain
            // paragraph carrying just its kind and span.
            "example" => {
                let para = new_child(
                    block.span,
                    std::mem::replace(&mut block.kind, BlockKind::Example { blocks: Vec::new() }),
                );
                if let BlockKind::Example { blocks } = &mut block.kind {
                    blocks.push(para);
                }
            }
            // `[sidebar]` on a paragraph wraps it the same single-child way as
            // `[example]`; the `****` fence form is built directly in `parse_block`.
            "sidebar" => {
                let para = new_child(
                    block.span,
                    std::mem::replace(&mut block.kind, BlockKind::Sidebar { blocks: Vec::new() }),
                );
                if let BlockKind::Sidebar { blocks } = &mut block.kind {
                    blocks.push(para);
                }
            }
            // `[quote]` on a paragraph wraps it like `[example]` — a single-child
            // quote container (attribution folded in below).
            "quote" => {
                let para = new_child(
                    block.span,
                    std::mem::replace(
                        &mut block.kind,
                        BlockKind::Quote {
                            attribution: None,
                            blocks: Vec::new(),
                        },
                    ),
                );
                if let BlockKind::Quote { blocks, .. } = &mut block.kind {
                    blocks.push(para);
                }
            }
            // `[verse]` on a paragraph: re-scan the paragraph's source *without*
            // folding newlines (verse renders in a `<pre>`, so a wrapped multi-line
            // paragraph must keep its breaks). The already-scanned (folded) inlines
            // are discarded.
            "verse" => {
                let inlines = lines.scan(block.span.start, block.span.end, attrs, false, context);
                block.kind = BlockKind::Verse {
                    attribution: None,
                    inlines,
                };
            }
            // `[open]` on a paragraph wraps it the same single-child way as
            // `[example]`/`[sidebar]`; the `--` fence form is built in `parse_block`.
            // `[abstract]`/`[partintro]` are open blocks too in Ruby (the style
            // stays on `attrs.style`).
            "open" | "abstract" | "partintro" => {
                let para = new_child(
                    block.span,
                    std::mem::replace(&mut block.kind, BlockKind::Open { blocks: Vec::new() }),
                );
                if let BlockKind::Open { blocks } = &mut block.kind {
                    blocks.push(para);
                }
            }
            _ => {}
        }
    }
    // A `--` open block "masquerades" as its *container* style: move its parsed
    // children into that container kind. The admonition styles are handled by the
    // early-return block above; the verbatim styles (`source`/`literal`/`verse`)
    // are handled earlier, at the `--` fence in `parse_block` (they switch the
    // content model, so such a block never reaches here as `Open`). `[open]` and no
    // style stay an open block.
    if let BlockKind::Open { blocks } = &mut block.kind {
        match style {
            "quote" => {
                let blocks = std::mem::take(blocks);
                block.kind = BlockKind::Quote {
                    attribution: None,
                    blocks,
                };
            }
            "example" => {
                let blocks = std::mem::take(blocks);
                block.kind = BlockKind::Example { blocks };
            }
            "sidebar" => {
                let blocks = std::mem::take(blocks);
                block.kind = BlockKind::Sidebar { blocks };
            }
            _ => {}
        }
    }
    // Rekey the positional attributes (`[quote, author, work]`) into the quote /
    // verse attribution — both the `____` fence form and the paragraph form land
    // here. Consumed from `attrs.positional` so they are not also surfaced raw.
    if matches!(
        block.kind,
        BlockKind::Quote { .. } | BlockKind::Verse { .. }
    ) {
        block.attrs.positional.clear();
        let attribution = build_attribution(list.value(2), list.value(3));
        if let BlockKind::Quote {
            attribution: slot, ..
        }
        | BlockKind::Verse {
            attribution: slot, ..
        } = &mut block.kind
        {
            *slot = attribution;
        }
    }
}

/// Build a quote / verse [`Attribution`] from Ruby's second and third
/// positional attributes (`[quote, author, citetitle]`). A nil field (empty
/// and unquoted, or absent) is `None`; a quoted empty value is an empty part,
/// as in asciidoctor. An attribution with neither part collapses to `None`.
/// The parts were substituted while reading the attribute line: single-quoted
/// values with normal substitutions, others as raw source and references.
fn build_attribution(
    author: Option<&crate::block_attributes::AttrValue>,
    citetitle: Option<&crate::block_attributes::AttrValue>,
) -> Option<Attribution> {
    let author = author.map(|v| v.inlines.clone());
    let citetitle = citetitle.map(|v| v.inlines.clone());
    if author.is_none() && citetitle.is_none() {
        None
    } else {
        Some(Attribution { author, citetitle })
    }
}

/// Paragraph styles that Ruby builds outside its ordinary paragraph branch,
/// which is the only one that catalogs inline anchors.
fn changes_paragraph_type(style: &str) -> bool {
    AdmonitionKind::from_style(style).is_some()
        || matches!(
            style,
            "abstract"
                | "comment"
                | "example"
                | "listing"
                | "literal"
                | "open"
                | "partintro"
                | "pass"
                | "quote"
                | "sidebar"
                | "source"
                | "stem"
                | "latexmath"
                | "asciimath"
                | "verse"
        )
}

/// Capture the verbatim body of a delimited block whose opening `fence` has just
/// been consumed (so `*i` points at the first body line). Scans lines until the
/// matching closing `fence`, returning `(body_start, body_end, block_end)` as byte
/// offsets into the source. The body slice is taken verbatim — the caller decides
/// whether to inline-scan it (verse) or keep it raw (literal/source). An
/// unterminated block pushes a `block_name` diagnostic but is never a failure.
/// Shared by the `____` verse branch and the `--` verbatim masquerades.
fn capture_verbatim_body(
    lines: &mut Reader<'_>,
    i: &mut usize,
    opening: Span,
    fence: &str,
    block_name: &str,
    context: &ParseContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> (usize, usize, usize) {
    let body_start = if lines.has(*i, context) {
        lines[*i].0
    } else {
        opening.end
    };
    let mut body_end = body_start;
    let mut end = opening.end;
    let mut closed = false;
    while lines.has(*i, context) {
        let (loff, lline_buffer) = lines.line(*i);
        let lline = lline_buffer.as_ref();
        if lline.trim() == fence {
            end = loff + lline.len();
            *i += 1;
            closed = true;
            break;
        }
        body_end = loff + lline.len();
        end = body_end;
        *i += 1;
    }
    if !closed {
        diagnostics.push(Diagnostic {
            span: Span::new(opening.start, opening.start + fence.len()),
            severity: Severity::Error,
            message: format!("unterminated {block_name} block (missing closing `{fence}`)"),
        });
    }
    (body_start, body_end, end)
}

/// Where a block is being parsed. The only behavioural difference is how a
/// paragraph *ends*: at the top level it runs until a blank line, a heading or a
/// column-0 list marker; inside a list item (after a `+` continuation) it
/// additionally ends at a lone `+` line or an *indented* list marker, so an
/// attached paragraph does not swallow the next continuation or sibling item.
/// `Section` is the top level of the document or a section, the only place where
/// Ruby's `next_section` looks for (setext) section titles; container bodies use
/// `TopLevel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockCtx {
    Section,
    TopLevel,
    InListItem,
}

/// Parse exactly one block starting at `lines[*i]` (which the caller has ensured
/// is non-blank), advancing `*i` past every line it consumes. Returns `None`
/// only defensively, if a blank line slipped through. Shared by the top-level
/// loop and by list-item `+` continuations, so attached blocks reuse the very
/// same block grammar.
fn parse_block(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    diagnostics: &mut Vec<Diagnostic>,
    ctx: BlockCtx,
    pending_attrs: Option<&BlockAttrs>,
    has_id: bool,
) -> Option<Block> {
    let mut block =
        parse_block_content(lines, i, context, diagnostics, ctx, pending_attrs, has_id)?;
    block.attribute_context = Arc::clone(&context.attributes);
    Some(block)
}

/// Block options affect this block's inline substitutions, not the document
/// snapshot, nested container blocks or the principal text of list items.
fn inline_attributes(attributes: &Arc<Attributes>, block: Option<&BlockAttrs>) -> Arc<Attributes> {
    let mut attributes = Arc::clone(attributes);
    if block.is_some_and(|b| {
        b.options.iter().any(|o| o == "hardbreaks") || b.named.contains_key("hardbreaks-option")
    }) {
        Arc::make_mut(&mut attributes).local_hardbreaks = true;
    }
    attributes
}

fn parse_block_content(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    diagnostics: &mut Vec<Diagnostic>,
    ctx: BlockCtx,
    // The *pending* block style (from a preceding `[verse]` / `[quote]` attribute
    // line). A `____` fence needs it to choose its content model: `verse` is
    // verbatim (like `....`), every other style — including the bare default — is
    // a parsed `quote` container (like `====`).
    pending_attrs: Option<&BlockAttrs>,
    // Whether metadata already supplied an id (a section-title anchor then stays text).
    has_id: bool,
) -> Option<Block> {
    let pending_style = pending_attrs.and_then(|attrs| attrs.style.as_deref());
    let attrs = inline_attributes(&context.render_attributes(), pending_attrs);
    let (off, line_buffer) = lines.line(*i);
    let line = line_buffer.as_ref();
    let trimmed = line.trim_end();

    if trimmed.trim().is_empty() {
        *i += 1;
        return None;
    }

    if skip_comment(lines, i, context) {
        return None;
    }

    // Heading. ATX takes precedence over a setext title. Ruby recognizes both
    // forms only where it looks for sections, and a discrete heading anywhere;
    // elsewhere (and within a paragraph) the lines are ordinary text.
    let discrete = matches!(pending_style, Some("discrete" | "float"));
    let sections = ctx == BlockCtx::Section || discrete;
    let heading = parse_heading(trimmed)
        .filter(|_| sections)
        .map(|(level, title)| {
            lines.heading_lines.push(Span::new(off, off + line.len()));
            (level, title, off + line.len(), 1)
        })
        .or_else(|| {
            if !sections {
                return None;
            }
            // Ruby peeks a comment-styled block's next line without preprocessing.
            let has_next = if pending_style == Some("comment") {
                lines.raw_has(*i + 1)
            } else {
                lines.has(*i + 1, context)
            };
            if !has_next {
                return None;
            }
            let (underline_off, underline) = lines.line(*i + 1);
            let level = setext_level(line, &underline)?;
            lines.heading_lines.extend([
                Span::new(off, off + line.len()),
                Span::new(underline_off, underline_off + underline.len()),
            ]);
            Some((level, line, underline_off + underline.len(), 2))
        });
    if let Some((level, title, end, consumed)) = heading {
        let adjusted = context.section_level(level);
        if adjusted > u8::MAX as i128 {
            diagnostics.push(Diagnostic {
                span: Span::new(off, end),
                severity: Severity::Warning,
                message: "section level exceeds the supported maximum of 255".into(),
            });
        }
        let level = adjusted.clamp(0, u8::MAX as i128) as u8;
        // `title` is a subslice of this detached reader line.
        // Without an id from metadata, a trailing ` [[id,reftext]]` names the
        // section and is removed from its title.
        let anchor = (!has_id).then(|| section_anchor(title)).flatten();
        let title = anchor.as_ref().map_or(title, |a| a.title);
        let base = lines[*i].0 + abs_offset(line, title);
        // Ruby decides the section's kind and level before converting its title.
        let level = if discrete {
            level
        } else {
            context.close_sections(level, off);
            context.read_section((off, end), pending_style, level)
        };
        let heading_attrs = inline_attributes(&context.attributes, pending_attrs);
        let inlines = context.reading_title(title, base, &heading_attrs, &lines.include_catalog);
        // `Block.id` is otherwise assigned later, by the resolve pass, once the
        // whole document is known (it deduplicates ids across all sections).
        let mut block = Block::new(Span::new(off, end), BlockKind::Heading { level, inlines });
        if let Some(anchor) = anchor {
            block.id = Some(anchor.id.to_owned());
            context.section_reftext = anchor.reftext.map(|reftext| {
                let base = lines[*i].0 + abs_offset(line, reftext);
                crate::reftext::Read::source(reftext, base)
            });
        }
        *i += consumed;
        return Some(block);
    }

    // Block macros are recognized at a block start whatever the style, except
    // that a verbatim style keeps the line as paragraph text (Ruby's
    // `strict_verbatim_paragraphs`). Their brackets supply metadata.
    let verbatim = matches!(
        pending_style,
        Some("literal" | "listing" | "source" | "verse")
    );
    if let Some(found) = crate::image::match_macro(trimmed).filter(|_| !verbatim) {
        *i += 1;
        return read_image_macro(trimmed, off, found, context, &lines.include_catalog);
    }
    if let Some(meta) = parse_toc_macro(trimmed, off).filter(|_| !verbatim) {
        context.block_macro = Some(meta);
        *i += 1;
        return Some(Block::new(
            Span::new(off, off + trimmed.len()),
            BlockKind::Toc,
        ));
    }

    // Listing block (verbatim, fenced by `----`).
    if trimmed.trim() == "----" {
        let start = off;
        let mut end = off + line.len();
        *i += 1;
        let mut text = String::new();
        let mut closed = false;
        while lines.has(*i, context) {
            let (loff, lline_buffer) = lines.line(*i);
            let lline = lline_buffer.as_ref();
            if lline.trim() == "----" {
                end = loff + lline.len();
                *i += 1;
                closed = true;
                break;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(lline);
            end = loff + lline.len();
            *i += 1;
        }
        if !closed {
            diagnostics.push(Diagnostic {
                span: Span::new(start, start + 4),
                severity: Severity::Error,
                message: "unterminated listing block (missing closing `----`)".to_string(),
            });
        }
        return Some(Block::new(
            Span::new(start, end),
            BlockKind::Listing { text },
        ));
    }

    // Literal block (verbatim, fenced by `....`). Same shape as the `----`
    // listing above, but yields a `Literal` (rendered as a `literalblock`).
    if trimmed.trim() == "...." {
        let start = off;
        let mut end = off + line.len();
        *i += 1;
        let mut text = String::new();
        let mut closed = false;
        while lines.has(*i, context) {
            let (loff, lline_buffer) = lines.line(*i);
            let lline = lline_buffer.as_ref();
            if lline.trim() == "...." {
                end = loff + lline.len();
                *i += 1;
                closed = true;
                break;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(lline);
            end = loff + lline.len();
            *i += 1;
        }
        if !closed {
            diagnostics.push(Diagnostic {
                span: Span::new(start, start + 4),
                severity: Severity::Error,
                message: "unterminated literal block (missing closing `....`)".to_string(),
            });
        }
        return Some(Block::new(
            Span::new(start, end),
            BlockKind::Literal { text },
        ));
    }

    // Passthrough block (verbatim, fenced by `++++`). Like `....`/`----` the body
    // is captured raw with no inline substitutions; what sets it apart is the
    // backend, which emits the text *completely raw* (no escaping, no wrapper).
    // The whole trimmed line must be exactly `++++`; inline passthrough
    // delimiters are handled only within parsed inline content.
    if trimmed.trim() == "++++" {
        let start = off;
        *i += 1;
        let (body_start, body_end, end) = capture_verbatim_body(
            lines,
            i,
            Span::new(start, off + line.len()),
            "++++",
            "passthrough",
            context,
            diagnostics,
        );
        return Some(Block::new(
            Span::new(start, end),
            BlockKind::Passthrough {
                text: lines.content(body_start, body_end).text,
            },
        ));
    }

    // Example block (a parsed container, fenced by `====`). Unlike the verbatim
    // `----`/`....` blocks above, the body is parsed *as blocks* via the shared
    // `collect_blocks` (so nested blocks, inline subs and anchors all apply). A
    // same-length `====` inside closes the outer block (delimiter-length nesting
    // is not implemented), matching the single-length limitation of `----`/`....`.
    if trimmed.trim() == "====" {
        let start = off;
        *i += 1;
        let blocks = collect_blocks(
            lines,
            i,
            context,
            diagnostics,
            Some("===="),
            PendingMeta::default(),
        );
        let end;
        if lines.has(*i, context) && lines[*i].1.trim() == "====" {
            let (loff, lline_buffer) = lines.line(*i);
            let lline = lline_buffer.as_ref();
            end = loff + lline.len();
            *i += 1;
        } else {
            // Unterminated: span ends at the last child (or the opening fence when
            // empty); the missing close is a diagnostic, not a parse failure.
            end = blocks.last().map_or(off + line.len(), |b| b.span.end);
            diagnostics.push(Diagnostic {
                span: Span::new(start, start + 4),
                severity: Severity::Error,
                message: "unterminated example block (missing closing `====`)".to_string(),
            });
        }
        return Some(Block::new(
            Span::new(start, end),
            BlockKind::Example { blocks },
        ));
    }

    // Quote / verse block, fenced by `____`. asciidoctor's default style for a
    // `____` block is `quote`, whose body is parsed *as blocks* (like `====`); a
    // `[verse]` style instead makes it verbatim (like `....`) while still applying
    // inline substitutions. The style arrives via `pending_style` (the `[verse]`
    // attribute line precedes the fence). The positional attribution
    // (`[quote, author, work]`) is folded in afterwards by `apply_style_fixup`.
    if trimmed.trim() == "____" {
        let start = off;
        // Verse: verbatim body. Capture the exact source slice (so internal line
        // breaks survive) and inline-scan it — verse keeps newlines but still
        // formats `*bold*`, unlike the no-subs `Literal`/`Listing`.
        if pending_style == Some("verse") {
            *i += 1;
            let (body_start, body_end, end) = capture_verbatim_body(
                lines,
                i,
                Span::new(start, off + line.len()),
                "____",
                "verse",
                context,
                diagnostics,
            );
            // Verse keeps its line breaks — scan without folding newlines.
            let inlines = lines.scan(body_start, body_end, &attrs, false, context);
            return Some(Block::new(
                Span::new(start, end),
                BlockKind::Verse {
                    attribution: None,
                    inlines,
                },
            ));
        }
        // Quote (default): a parsed container, mirror of the `====` example block.
        *i += 1;
        let blocks = collect_blocks(
            lines,
            i,
            context,
            diagnostics,
            Some("____"),
            PendingMeta::default(),
        );
        let end;
        if lines.has(*i, context) && lines[*i].1.trim() == "____" {
            let (loff, lline_buffer) = lines.line(*i);
            let lline = lline_buffer.as_ref();
            end = loff + lline.len();
            *i += 1;
        } else {
            end = blocks.last().map_or(off + line.len(), |b| b.span.end);
            diagnostics.push(Diagnostic {
                span: Span::new(start, start + 4),
                severity: Severity::Error,
                message: "unterminated quote block (missing closing `____`)".to_string(),
            });
        }
        return Some(Block::new(
            Span::new(start, end),
            BlockKind::Quote {
                attribution: None,
                blocks,
            },
        ));
    }

    // Sidebar block (a parsed container, fenced by `****`). A mirror of the `====`
    // example above: the body is parsed *as blocks* via `collect_blocks`. The check
    // requires the *whole* trimmed line to be `****`, so it precedes (and is
    // distinct from) the `**` unordered-list marker runs — a `**** item` line keeps
    // its content and stays a 4-level list item. A same-length `****` inside closes
    // the outer block (delimiter-length nesting is not implemented).
    if trimmed.trim() == "****" {
        let start = off;
        *i += 1;
        let blocks = collect_blocks(
            lines,
            i,
            context,
            diagnostics,
            Some("****"),
            PendingMeta::default(),
        );
        let end;
        if lines.has(*i, context) && lines[*i].1.trim() == "****" {
            let (loff, lline_buffer) = lines.line(*i);
            let lline = lline_buffer.as_ref();
            end = loff + lline.len();
            *i += 1;
        } else {
            // Unterminated: span ends at the last child (or the opening fence when
            // empty); the missing close is a diagnostic, not a parse failure.
            end = blocks.last().map_or(off + line.len(), |b| b.span.end);
            diagnostics.push(Diagnostic {
                span: Span::new(start, start + 4),
                severity: Severity::Error,
                message: "unterminated sidebar block (missing closing `****`)".to_string(),
            });
        }
        return Some(Block::new(
            Span::new(start, end),
            BlockKind::Sidebar { blocks },
        ));
    }

    // Open block (a parsed container, fenced by `--`). The *generic* container:
    // the body is parsed *as blocks* via `collect_blocks`, exactly like the
    // `====`/`****` blocks, and a *container* style applied to it masquerades it
    // into that kind later, in `apply_style_fixup`. The check requires the whole
    // trimmed line to be exactly `--` (two hyphens): `----` is already consumed
    // above as a listing fence, `---` falls through to a paragraph, and an inline
    // `--` (em dash) never reaches here — it lives inside paragraph text, scanned
    // after block detection. A same-length `--` inside closes the outer block.
    if trimmed.trim() == "--" {
        if pending_style == Some("comment") {
            crate::comments::consume_delimited_comment(lines, i, context, "--");
            return None;
        }
        let start = off;
        // Verbatim masquerades: a `[source]` / `[literal]` / `[verse]` style
        // switches the open block's content model from "parsed blocks" to
        // *verbatim*, mirroring how `____` above picks Verse over Quote. The body
        // must not be parsed as blocks, so the choice is made here at the fence —
        // not later in `apply_style_fixup`, which only ever sees an already-parsed
        // body. (The `language` for `[source,lang]` and the verse attribution ride
        // on `attrs`, applied to the block after this returns.)
        if matches!(pending_style, Some("source" | "literal" | "verse" | "pass")) {
            *i += 1;
            let (body_start, body_end, end) = capture_verbatim_body(
                lines,
                i,
                Span::new(start, off + line.len()),
                "--",
                "open",
                context,
                diagnostics,
            );
            let body = lines.content(body_start, body_end);
            let kind = match pending_style {
                // Verse keeps its line breaks but still applies inline subs; the
                // positional attribution (`[verse, author, work]`) is folded in
                // afterwards by `apply_style_fixup`, exactly like the `____` form.
                Some("verse") => BlockKind::Verse {
                    attribution: None,
                    inlines: lines.scan(body_start, body_end, &attrs, false, context),
                },
                // Literal / source keep the body raw (no inline subs). For
                // `[source,lang]` the `source` style and `language` attribute ride
                // on `attrs` and drive the HTML backend's highlight rendering.
                Some("literal") => BlockKind::Literal { text: body.text },
                Some("source") => BlockKind::Listing { text: body.text },
                // `[pass]` on `--`: raw passthrough, emitted unescaped and
                // unwrapped by the backend (no inline subs), like the `++++` fence.
                Some("pass") => BlockKind::Passthrough { text: body.text },
                _ => unreachable!(),
            };
            return Some(Block::new(Span::new(start, end), kind));
        }
        *i += 1;
        let blocks = collect_blocks(
            lines,
            i,
            context,
            diagnostics,
            Some("--"),
            PendingMeta::default(),
        );
        let end;
        if lines.has(*i, context) && lines[*i].1.trim() == "--" {
            let (loff, lline_buffer) = lines.line(*i);
            let lline = lline_buffer.as_ref();
            end = loff + lline.len();
            *i += 1;
        } else {
            // Unterminated: span ends at the last child (or the opening fence when
            // empty); the missing close is a diagnostic, not a parse failure.
            end = blocks.last().map_or(off + line.len(), |b| b.span.end);
            diagnostics.push(Diagnostic {
                span: Span::new(start, start + 2),
                severity: Severity::Error,
                message: "unterminated open block (missing closing `--`)".to_string(),
            });
        }
        return Some(Block::new(
            Span::new(start, end),
            BlockKind::Open { blocks },
        ));
    }

    // List: a run of items that nests a level deeper whenever a *unique* marker
    // appears. A list only *starts* at column 0 (an indented marker elsewhere is
    // ordinary text); deeper levels may be indented, handled inside `parse_list`,
    // which consumes the whole (possibly nested) list.
    if parse_list_marker(trimmed).is_some() {
        return Some(parse_list(lines, i, context, diagnostics, &[]));
    }

    // Verbatim paragraph styles retain comment-looking lines.
    let verbatim = matches!(pending_style, Some("literal" | "source" | "verse"));

    // Paragraph: accumulate contiguous lines until this context's terminator.
    let start = off;
    let mut end = off + line.len();
    *i += 1;
    while if pending_style == Some("comment") {
        lines.raw_has(*i)
    } else {
        lines.has(*i, context)
    } {
        let (loff, lline_buffer) = lines.line(*i);
        let lline = lline_buffer.as_ref();
        let t = lline.trim_end();
        // A paragraph also ends at any block delimiter line, so it never spans
        // into a `----`/`....`/`====`/`____`/`****`/`--` block (notably the closing
        // fence of an enclosing container, which would otherwise be folded in as
        // text).
        let ends = (!verbatim && comment_fence(t).is_some())
            || t.trim().is_empty()
            || matches!(t.trim(), "----" | "...." | "====" | "____" | "****" | "--")
            || match ctx {
                BlockCtx::Section | BlockCtx::TopLevel => parse_list_marker(t).is_some(),
                BlockCtx::InListItem => t.trim() == "+" || list_marker_indented(t).is_some(),
            };
        if ends {
            break;
        }
        end = loff + lline.len();
        *i += 1;
    }
    if pending_style == Some("comment") {
        context.comments.push(Span::new(start, end));
        return None;
    }
    // An admonition label prefix (`NOTE: `, …) on the first line turns the whole
    // paragraph into an admonition: the prefix is stripped and the rest (incl. any
    // folded lines) is the body. The block span stays the whole paragraph. The
    // `[NOTE]` *style* form is handled later, in the pending-attrs fixup.
    if let Some((kind, prefix_len)) = AdmonitionKind::from_label_prefix(line) {
        let content_start = start + prefix_len;
        lines.catalog_anchors(content_start, end, false, context);
        let inlines = lines.scan_content(content_start, end, &attrs, context);
        // The body is a single child paragraph spanning the text after the prefix;
        // the admonition block still spans the whole paragraph (label included).
        let mut child = Block::new(
            Span::new(content_start, end),
            BlockKind::Paragraph { inlines },
        );
        child.attribute_context = Arc::clone(&attrs);
        return Some(Block::new(
            Span::new(start, end),
            BlockKind::Admonition {
                kind,
                label: context.admonition_label(kind, pending_attrs),
                blocks: vec![child],
            },
        ));
    }
    // Parsed text also removes comment lines; verbatim styles keep them.
    // Both paths map the preprocessed reader text to original byte spans.
    let inlines = if verbatim {
        let content = lines.content(start, end);
        let mut nodes = scan_inlines_report(&content.text, 0, &attrs, true, &mut Vec::new());
        content.restore_inlines(&mut nodes);
        nodes
    } else {
        // Ruby catalogs anchors only for ordinary paragraphs; a style that
        // changes the block type is built without cataloging.
        if !pending_style.is_some_and(changes_paragraph_type) {
            lines.catalog_anchors(start, end, false, context);
        }
        lines.scan_content(start, end, &attrs, context)
    };
    Some(Block::new(
        Span::new(start, end),
        BlockKind::Paragraph { inlines },
    ))
}

/// The role of a source line *relative to a list level* (its current `marker`
/// and the `ancestors` stack). Computing it in one place lets `parse_list` read
/// as a small state machine: for the next line it decides whether the line
/// continues the item's text, attaches a block, opens a sublist, or ends the
/// list.
#[derive(Debug, PartialEq, Eq)]
enum LineRole {
    /// Blank line — ends the list (the current simplification).
    Blank,
    /// A lone `+` — a list continuation; the next block attaches to the item.
    Continuation,
    /// A marker identical to this level's — the next sibling item.
    Sibling,
    /// A marker belonging to an enclosing list — hands control back up.
    Ancestor,
    /// A unique, deeper marker — opens a sublist under the current item.
    Nested,
    /// Anything else — principal-text continuation of the current item.
    Text,
}

/// Classify `line` against the current list `marker` and `ancestors`. Marker
/// detection is indent-aware ([`list_marker_indented`]): asciidoctor decides
/// nesting by marker *identity*, not indentation, so an indented `*` under a
/// `*` list is a sibling, while `**` opens a deeper level regardless of indent.
fn classify(line: &str, marker: MarkerId, ancestors: &[MarkerId]) -> LineRole {
    let t = line.trim_end();
    if t.trim().is_empty() || comment_fence(t).is_some() {
        return LineRole::Blank;
    }
    if t.trim() == "+" {
        return LineRole::Continuation;
    }
    match list_marker_indented(t) {
        Some((m, _, _)) if m == marker => LineRole::Sibling,
        Some((m, _, _)) if ancestors.contains(&m) => LineRole::Ancestor,
        Some(_) => LineRole::Nested,
        None => LineRole::Text,
    }
}

/// Parse a list — and any deeper sublists — starting at `lines[*i]`, which must
/// carry a list marker. Advances `*i` past every consumed line.
///
/// `ancestors` holds the marker identities of the enclosing lists. A line whose
/// marker matches an ancestor ends this list (returning control to that
/// ancestor); a *unique* marker opens a sublist under the current item — this is
/// asciidoctor's marker-stack nesting rule. Each item's principal text may wrap
/// over several plain lines, and a `+` continuation attaches following blocks
/// (paragraph, listing) to the item; both nested sublists and attached blocks
/// live in `item.blocks`, in source order.
fn parse_list(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    diagnostics: &mut Vec<Diagnostic>,
    ancestors: &[MarkerId],
) -> Block {
    let (marker, ordered, _) =
        list_marker_indented(lines[*i].1.trim_end()).expect("parse_list called on a non-list line");
    let start = lines[*i].0;
    let mut end = start;
    let mut items = Vec::new();

    // Only a sibling marker begins another item; everything else (a blank line,
    // an ancestor's marker) ends this list. Deeper markers and `+` lines are
    // folded into the item below, so they never reach this guard.
    while lines.has(*i, context) && classify(&lines[*i].1, marker, ancestors) == LineRole::Sibling {
        let (loff, lline_buffer) = lines.line(*i);
        let lline = lline_buffer.as_ref();
        let content_off = list_marker_indented(lline.trim_end())
            .map(|(_, _, off)| off)
            .expect("a sibling line carries a marker");
        let item_start = loff;
        let content_start = loff + content_off;
        let mut principal_end = loff + lline.len();
        *i += 1;

        // Multi-line principal text: fold following plain lines into this item.
        while lines.has(*i, context) && classify(&lines[*i].1, marker, ancestors) == LineRole::Text
        {
            let (toff, tline_buffer) = lines.line(*i);
            let tline = tline_buffer.as_ref();
            principal_end = toff + tline.len();
            *i += 1;
        }
        let attrs = context.render_attributes();
        lines.catalog_anchors(content_start, principal_end, true, context);
        let inlines = lines.scan_content(content_start, principal_end, &attrs, context);

        // Attached blocks (`+` continuations) and nested sublists, in source
        // order, all collected into `item.blocks`.
        let mut blocks = Vec::new();
        let mut item_end = principal_end;
        while lines.has(*i, context) {
            if is_line_comment(&lines[*i].1) {
                skip_comment(lines, i, context);
                continue;
            }
            match classify(&lines[*i].1, marker, ancestors) {
                LineRole::Continuation => {
                    *i += 1; // consume the `+`
                    if lines.has(*i, context) && is_line_comment(&lines[*i].1) {
                        while skip_comment(lines, i, context) {}
                        break;
                    }
                    if lines.has(*i, context) && !lines[*i].1.trim().is_empty() {
                        if let Some(mut b) = parse_block(
                            lines,
                            i,
                            context,
                            diagnostics,
                            BlockCtx::InListItem,
                            None,
                            false,
                        ) {
                            if let Some(value) = context.section_reftext.take() {
                                context.attach_reftext(&mut b, value);
                            }
                            if let Some(mut meta) = context.block_macro.take() {
                                if let Some(value) = meta.reftext.take() {
                                    context.attach_reftext(&mut b, value);
                                }
                                let title = meta.title.take();
                                meta.apply_alone(&mut b);
                                complete_image(&mut b, &context.attributes);
                                if let Some(title) = title {
                                    b.title = Some(context.body_subs.defer_title(
                                        &title.text,
                                        title.base,
                                        title.origin,
                                        &context.attributes,
                                    ));
                                    context.assign_caption(&mut b);
                                }
                            }
                            item_end = b.span.end;
                            blocks.push(b);
                        }
                    }
                }
                LineRole::Nested => {
                    let mut child_anc = ancestors.to_vec();
                    child_anc.push(marker);
                    let child = parse_list(lines, i, context, diagnostics, &child_anc);
                    item_end = child.span.end;
                    blocks.push(child);
                }
                _ => break,
            }
        }

        end = item_end;
        items.push(ListItem {
            inlines,
            blocks,
            span: Span::new(item_start, item_end),
        });
    }

    let mut block = Block::new(Span::new(start, end), BlockKind::List { ordered, items });
    block.attribute_context = Arc::clone(&context.attributes);
    block
}

/// Byte offset of subslice `sub` within `source`. `sub` must be a slice that
/// points into `source` (e.g. produced by `trim`/`strip_prefix` on a line).
fn abs_offset(source: &str, sub: &str) -> usize {
    sub.as_ptr() as usize - source.as_ptr() as usize
}

/// Identity of a list marker. Two markers open distinct nesting levels iff they
/// differ — this is asciidoctor's rule ("a new level for each unique marker"),
/// so nesting works across kinds (e.g. a `.` item inside a `*` list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkerId {
    /// A run-marker: its leading byte and how many times that byte repeats. `-`
    /// is always width 1; `*` and `.` may repeat (`**`, `...`) to mark a deeper
    /// level.
    Run { byte: u8, width: usize },
    /// An explicit-numbered ordered marker (`1.`, `2.`, `10.`, …). All of them
    /// share one identity: the digit *value* and *count* don't affect nesting,
    /// and the numbers are rendered positionally — so this is distinct from the
    /// implicit ordered marker `Run { byte: b'.', .. }`, matching asciidoctor
    /// (a `1.` item after a `.` list nests rather than continuing it).
    OrderedExplicit,
}

/// A list marker at the very start of `line`: a run of `*` or `.`, a single
/// `-`, or an explicit-numbered `<digits>.` — each followed by a space. Returns
/// `(marker, ordered, marker_len_bytes)`, where `marker_len_bytes` is the byte
/// offset at which item content begins. Markers must be at column 0: leading
/// whitespace is not accepted. (All marker bytes are ASCII, so a run's width
/// equals its byte length.)
pub(crate) fn parse_list_marker(line: &str) -> Option<(MarkerId, bool, usize)> {
    let bytes = line.as_bytes();
    let first = *bytes.first()?;

    // Explicit-numbered ordered marker: a run of ASCII digits, then `.`, then a
    // space (e.g. `1. `, `10. `). Collapses to a single identity regardless of
    // the digits — `1.` and `10.` are siblings of one list.
    if first.is_ascii_digit() {
        let mut digits = 1;
        while matches!(bytes.get(digits), Some(b) if b.is_ascii_digit()) {
            digits += 1;
        }
        if bytes.get(digits) != Some(&b'.') || bytes.get(digits + 1) != Some(&b' ') {
            return None;
        }
        return Some((MarkerId::OrderedExplicit, true, digits + 2));
    }

    let (ordered, repeatable) = match first {
        b'*' => (false, true),
        b'.' => (true, true),
        b'-' => (false, false),
        _ => return None,
    };
    let mut width = 1;
    if repeatable {
        while bytes.get(width) == Some(&first) {
            width += 1;
        }
    }
    if bytes.get(width) != Some(&b' ') {
        return None;
    }
    Some((MarkerId::Run { byte: first, width }, ordered, width + 1))
}

/// Like [`parse_list_marker`] but tolerant of leading ASCII spaces. Indentation
/// is cosmetic in asciidoctor — nesting is decided by marker *identity*, not
/// indent — so we strip leading spaces before matching and fold their width into
/// the returned content offset. The [`MarkerId`] comes from the de-indented
/// marker, so an indented `*` is the *same* level as a column-0 `*` (a sibling),
/// while `**` opens a new level regardless of indent.
pub(crate) fn list_marker_indented(line: &str) -> Option<(MarkerId, bool, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let (marker, ordered, mlen) = parse_list_marker(&line[indent..])?;
    Some((marker, ordered, indent + mlen))
}

pub(crate) fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let line = line.trim_start();
    let eqs = line.chars().take_while(|&c| c == '=').count();
    if eqs == 0 || eqs > 6 {
        return None;
    }
    let rest = &line[eqs..];
    let title = rest.strip_prefix(' ')?;
    Some(((eqs - 1) as u8, title.trim()))
}

/// A trailing section-title anchor, split off its title.
pub(crate) struct SectionAnchor<'a> {
    pub title: &'a str,
    pub id: &'a str,
    pub reftext: Option<&'a str>,
}

/// Ruby's `InlineSectionAnchorRx`: ` [[id]]` or ` [[id, reftext]]` ending a
/// section title, at its leftmost match. An escaped (`\[[`) match keeps the
/// title unchanged.
pub(crate) fn section_anchor(title: &str) -> Option<SectionAnchor<'_>> {
    if !title.ends_with("]]") {
        return None;
    }
    for (space, _) in title.match_indices(' ') {
        let rest = &title[space + 1..];
        let escaped = rest.starts_with("\\[[");
        let Some(inner) = rest.strip_prefix("\\").unwrap_or(rest).strip_prefix("[[") else {
            continue;
        };
        let mut chars = inner.char_indices();
        if !chars
            .next()
            .is_some_and(|(_, c)| c.is_alphabetic() || matches!(c, '_' | ':'))
        {
            continue;
        }
        let id_end = chars
            .find(|(_, c)| !crate::unicode_word::is_word(*c) && !matches!(c, '-' | ':' | '.'))
            .map_or(inner.len(), |(i, _)| i);
        let after = &inner[id_end..];
        let reftext = if after == "]]" {
            None
        } else if let Some(text) = after.strip_prefix(',') {
            // `, *(.+)` greedy to the final `]]`; the last space may be the text.
            let text = text.strip_suffix("]]")?;
            let trimmed = text.trim_start_matches(' ');
            let text = if trimmed.is_empty() {
                &text[text.len().saturating_sub(1)..]
            } else {
                trimmed
            };
            if text.is_empty() {
                continue;
            }
            Some(text)
        } else {
            continue;
        };
        if escaped {
            return None;
        }
        return Some(SectionAnchor {
            title: &title[..space],
            id: &inner[..id_end],
            reftext,
        });
    }
    None
}

/// Recognise a *block anchor* line — `[[id]]` or `[[id,reftext]]` occupying the
/// whole (trimmed) line — and return `(id, reftext?)` as slices of `line`. The
/// reftext is the trimmed, non-empty remainder after the first comma. An empty
/// `[[]]` or an id that is not well-formed (see [`is_valid_anchor_id`]) returns
/// `None`, so the line falls through to be parsed as ordinary text — matching
/// asciidoctor, which treats only a well-formed anchor specially.
///
/// An *inline* `[[id]]` embedded mid-line is a different construct (an empty
/// `<a id>`); this matches only a line that is wholly an anchor.
pub(crate) fn parse_block_anchor(line: &str) -> Option<(&str, Option<&str>)> {
    let inner = line.trim().strip_prefix("[[")?.strip_suffix("]]")?;
    let (id, reftext) = match inner.find(',') {
        Some(c) => {
            let rt = inner[c + 1..].trim();
            (&inner[..c], (!rt.is_empty()).then_some(rt))
        }
        None => (inner, None),
    };
    is_valid_anchor_id(id).then_some((id, reftext))
}

/// Recognise a *block title* line — a single leading `.` immediately followed by
/// a non-space, non-`.` character (after at most one extra leading `.`), the rest
/// of the line being the title. This is asciidoctor's `BlockTitleRx`
/// (`^\.(\.?[^.\s].*)$`): it sets the title of the block that follows.
///
/// The leading `.` marker is stripped; the returned slice is the title *text*
/// (trailing whitespace trimmed) and remains a sub-slice of the source line, so
/// callers can recover its byte offset with [`abs_offset`]. The optional second
/// dot is *part of the title* (`..foo` → title `.foo`), matching asciidoctor.
///
/// The leading-character rule is what disambiguates a title from the look-alikes
/// that also start with `.`: the ordered-list marker `. ` (dot + space → space
/// fails), the `....` literal delimiter and `..`/`...` nested list markers (a
/// second/third `.` fails). A line like `.NET release` *is* a title here, exactly
/// as asciidoctor parses it.
pub(crate) fn parse_block_title(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('.')?;
    let mut bytes = rest.bytes();
    let first = bytes.next()?;
    // An optional single dot may lead the title text; the *next* byte is the one
    // that must be significant (non-dot, non-whitespace).
    let significant = if first == b'.' { bytes.next()? } else { first };
    (significant != b'.' && !significant.is_ascii_whitespace()).then(|| rest.trim_end())
}

/// Whether `id` is a well-formed anchor id (asciidoctor's `[\p{Word}:][\p{Word}:.-]*`):
/// a leading alphanumeric / `_` / `:`, then those plus `.` and `-`, no whitespace.
/// Shared with the inline scanner, which validates *inline* `[[id]]` anchors.
pub(crate) fn is_valid_anchor_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_alphanumeric() || c == '_' || c == ':' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Inline, Span};
    use crate::parse;

    #[test]
    fn parses_title_and_paragraph() {
        let r = parse("= Title\n\nHello world");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.document.flat_blocks().len(), 2);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Heading { level: 0, .. }
        ));
    }

    #[test]
    fn heading_span_covers_the_line() {
        let r = parse("= Title");
        assert_eq!(r.document.flat_blocks()[0].span, Span::new(0, 7));
    }

    #[test]
    fn unterminated_listing_is_a_diagnostic_not_a_failure() {
        let r = parse("----\nlet x = 1;");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);
    }

    #[test]
    fn parses_header_attributes() {
        let r = parse(":author: Alexey\n:lang: ru\n\n= Title");
        assert_eq!(r.document.attributes.get("author"), Some("Alexey"));
        assert_eq!(r.document.attributes.get("lang"), Some("ru"));
    }

    /// Helper: the first inline of the first block, whatever its kind.
    fn first_inline(src: &str) -> Inline {
        match &parse(src).document.into_flat_blocks()[0].kind {
            BlockKind::Paragraph { inlines } | BlockKind::Heading { inlines, .. } => {
                inlines[0].clone()
            }
            other => panic!("unexpected block: {other:?}"),
        }
    }

    #[test]
    fn inline_strong_span_points_at_real_source() {
        let src = "Hi *there*";
        let inlines = match &parse(src).document.into_flat_blocks()[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            _ => unreachable!(),
        };
        let strong = inlines
            .iter()
            .find(|n| matches!(n, Inline::Strong { .. }))
            .unwrap();
        let s = strong.span();
        assert_eq!(&src[s.start..s.end], "*there*");
    }

    #[test]
    fn inline_span_is_accurate_across_a_line_break() {
        // The paragraph is scanned over the original source slice, so the
        // strong marker on the *second* line still gets real byte offsets.
        let src = "first line\nsecond *strong* line";
        let inlines = match &parse(src).document.into_flat_blocks()[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            _ => unreachable!(),
        };
        let strong = inlines
            .iter()
            .find(|n| matches!(n, Inline::Strong { .. }))
            .unwrap();
        let s = strong.span();
        assert_eq!(&src[s.start..s.end], "*strong*");
    }

    #[test]
    fn link_span_is_accurate_across_a_line_break() {
        // Like strong, a link on a wrapped line keeps real byte offsets.
        let src = "first line\nsee link:https://example.org[here] now";
        let inlines = match &parse(src).document.into_flat_blocks()[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            _ => unreachable!(),
        };
        let link = inlines
            .iter()
            .find(|n| matches!(n, Inline::Substituted { .. }))
            .unwrap();
        let s = link.span();
        assert_eq!(&src[s.start..s.end], "link:https://example.org[here]");
    }

    #[test]
    fn attribute_reference_is_resolved_in_core() {
        let src = ":v: 9\n\nVersion {v}";
        let attr = match first_inline(src) {
            Inline::Text { .. } => {
                // {v} is the second inline; grab the block again
                let blocks = parse(src).document.into_flat_blocks();
                match &blocks[0].kind {
                    BlockKind::Paragraph { inlines } => inlines[1].clone(),
                    _ => unreachable!(),
                }
            }
            other => other,
        };
        match attr {
            Inline::AttributeRef {
                name, value, span, ..
            } => {
                assert_eq!(name, "v");
                assert_eq!(value, Some("9".to_string()));
                // span points at `{v}` in the body, not at the resolved value
                assert_eq!(&src[span.start..span.end], "{v}");
            }
            other => panic!("expected AttributeRef, got {other:?}"),
        }
    }

    #[test]
    fn missing_attribute_keeps_no_value() {
        let r = parse("Ref to {unknown} here");
        let inlines = match &r.document.flat_blocks()[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            _ => unreachable!(),
        };
        assert!(inlines
            .iter()
            .any(|n| matches!(n, Inline::AttributeRef { value: None, .. })));
    }

    /// The list block of `src`, or panic if the first block is not a list.
    fn list(src: &str) -> (bool, Vec<ListItem>) {
        match &parse(src).document.into_flat_blocks()[0].kind {
            BlockKind::List { ordered, items } => (*ordered, items.clone()),
            other => panic!("expected List, got {other:?}"),
        }
    }

    /// The single nested list held by `item`, or panic.
    fn sublist(item: &ListItem) -> (bool, Vec<ListItem>) {
        assert_eq!(item.blocks.len(), 1, "expected exactly one nested block");
        match &item.blocks[0].kind {
            BlockKind::List { ordered, items } => (*ordered, items.clone()),
            other => panic!("expected nested List, got {other:?}"),
        }
    }

    /// The principal text of `item` (its first inline, asserted to be text).
    fn item_text(item: &ListItem) -> String {
        match &item.inlines[0] {
            Inline::Text { value, .. } => value.clone(),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn parses_unordered_star_list() {
        let r = parse("* one\n* two");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.document.flat_blocks().len(), 1);
        let (ordered, items) = list("* one\n* two");
        assert!(!ordered);
        assert_eq!(items.len(), 2);
        // Item content is scanned with the right base offset.
        assert!(matches!(&items[0].inlines[0], Inline::Text { value, span }
            if value == "one" && *span == Span::new(2, 5)));
    }

    #[test]
    fn dash_is_an_unordered_marker() {
        let (ordered, items) = list("- a\n- b");
        assert!(!ordered);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn parses_ordered_dot_list() {
        let (ordered, items) = list(". first\n. second");
        assert!(ordered);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn list_block_and_item_spans_cover_their_lines() {
        let src = "* one\n* two";
        let block = &parse(src).document.into_flat_blocks()[0];
        assert_eq!(block.span, Span::new(0, 11));
        let items = match &block.kind {
            BlockKind::List { items, .. } => items,
            _ => unreachable!(),
        };
        assert_eq!(items[0].span, Span::new(0, 5));
        assert_eq!(items[1].span, Span::new(6, 11));
    }

    #[test]
    fn list_stops_at_blank_line() {
        let r = parse("* one\n\nA paragraph");
        assert_eq!(r.document.flat_blocks().len(), 2);
        let (_, items) = list("* one\n\nA paragraph");
        assert_eq!(items.len(), 1);
        assert!(matches!(
            r.document.flat_blocks()[1].kind,
            BlockKind::Paragraph { .. }
        ));
    }

    #[test]
    fn different_marker_nests_as_sublist() {
        // asciidoctor's rule: a *unique* marker opens a deeper level. So `. b`
        // (ordered) nests inside the `* a` (unordered) item — it does not start
        // a second top-level list. (Confirmed against asciidoctor 2.0.23.)
        let r = parse("* a\n. b");
        assert_eq!(r.document.flat_blocks().len(), 1);
        let (ordered, items) = list("* a\n. b");
        assert!(!ordered);
        assert_eq!(items.len(), 1);
        assert_eq!(item_text(&items[0]), "a");
        let (sub_ordered, sub_items) = sublist(&items[0]);
        assert!(sub_ordered);
        assert_eq!(sub_items.len(), 1);
        assert_eq!(item_text(&sub_items[0]), "b");
    }

    #[test]
    fn doubled_marker_nests_under_its_item() {
        // `*` then `**` then back to `*`.
        let (ordered, items) = list("* one\n** a\n** b\n* two");
        assert!(!ordered);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "one");
        let (sub_ordered, sub_items) = sublist(&items[0]);
        assert!(!sub_ordered);
        assert_eq!(sub_items.len(), 2);
        assert_eq!(item_text(&sub_items[0]), "a");
        assert_eq!(item_text(&sub_items[1]), "b");
        // The second top-level item is a sibling with no nested content.
        assert_eq!(item_text(&items[1]), "two");
        assert!(items[1].blocks.is_empty());
    }

    #[test]
    fn three_levels_nest_and_unwind() {
        // `*` / `**` / `***`, then `**` (back one level), then `*` (back to top).
        let (_, items) = list("* one\n** a\n*** x\n** b\n* two");
        assert_eq!(items.len(), 2);
        let (_, lvl2) = sublist(&items[0]);
        assert_eq!(lvl2.len(), 2); // a (with a deeper child), b
        assert_eq!(item_text(&lvl2[0]), "a");
        let (_, lvl3) = sublist(&lvl2[0]);
        assert_eq!(lvl3.len(), 1);
        assert_eq!(item_text(&lvl3[0]), "x");
        assert_eq!(item_text(&lvl2[1]), "b");
        assert!(lvl2[1].blocks.is_empty());
        assert!(items[1].blocks.is_empty());
    }

    #[test]
    fn ordered_marker_run_nests() {
        let (ordered, items) = list(". one\n.. a\n.. b\n. two");
        assert!(ordered);
        assert_eq!(items.len(), 2);
        let (sub_ordered, sub_items) = sublist(&items[0]);
        assert!(sub_ordered);
        assert_eq!(sub_items.len(), 2);
    }

    #[test]
    fn mixed_ordered_unordered_from_docs() {
        // The hybrid-list example from the asciidoctor docs: an ordered list
        // whose items each hold a nested unordered list.
        let (ordered, items) = list(". Linux\n* Fedora\n* Ubuntu\n. BSD\n* FreeBSD");
        assert!(ordered);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "Linux");
        let (o0, distros0) = sublist(&items[0]);
        assert!(!o0);
        assert_eq!(distros0.len(), 2);
        assert_eq!(item_text(&distros0[0]), "Fedora");
        assert_eq!(item_text(&distros0[1]), "Ubuntu");
        assert_eq!(item_text(&items[1]), "BSD");
        let (o1, distros1) = sublist(&items[1]);
        assert!(!o1);
        assert_eq!(distros1.len(), 1);
        assert_eq!(item_text(&distros1[0]), "FreeBSD");
    }

    #[test]
    fn item_span_covers_its_nested_sublist() {
        // "* one\n** a": item "one" spans the whole construct (0..10); the
        // nested list block spans just its own line (6..10).
        let src = "* one\n** a";
        let (_, items) = list(src);
        assert_eq!(items[0].span, Span::new(0, 10));
        assert_eq!(&src[items[0].span.start..items[0].span.end], "* one\n** a");
        assert_eq!(items[0].blocks[0].span, Span::new(6, 10));
    }

    #[test]
    fn star_without_space_is_not_a_list() {
        // `*bold*` has no space after the marker → it stays a paragraph.
        let r = parse("*bold*");
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Paragraph { .. }
        ));
    }

    #[test]
    fn a_list_interrupts_a_paragraph_without_a_blank_line() {
        let r = parse("text\n* item");
        assert_eq!(r.document.flat_blocks().len(), 2);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Paragraph { .. }
        ));
        assert!(matches!(
            r.document.flat_blocks()[1].kind,
            BlockKind::List { .. }
        ));
    }

    /// `item_text` with interior whitespace collapsed — handy where a multi-line
    /// item folds its line break (and any indent) to spaces.
    fn item_text_squeezed(item: &ListItem) -> String {
        item_text(item)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The single nested/attached block of `item`, asserted to be a paragraph,
    /// returning its principal text.
    fn paragraph_text(block: &Block) -> String {
        match &block.kind {
            BlockKind::Paragraph { inlines } => match &inlines[0] {
                Inline::Text { value, .. } => value.clone(),
                other => panic!("expected Text, got {other:?}"),
            },
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn item_text_wraps_over_several_lines() {
        // A plain line after the marker folds into the same item's text — its
        // line break (and indent) become spaces, but the item span covers both
        // source lines. (Confirmed against asciidoctor 2.0.23: one <li>.)
        let src = "* one\n  two\n* three";
        let (_, items) = list(src);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text_squeezed(&items[0]), "one two");
        assert_eq!(item_text(&items[1]), "three");
        assert_eq!(&src[items[0].span.start..items[0].span.end], "* one\n  two");
    }

    #[test]
    fn indented_same_marker_is_a_sibling_not_nested() {
        // asciidoctor decides nesting by marker identity, not indent: an
        // indented `*` under a `*` list is a *sibling*, not a child.
        let src = "* one\n  * nested";
        let (_, items) = list(src);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "one");
        assert_eq!(item_text(&items[1]), "nested");
        assert!(items[0].blocks.is_empty());
    }

    #[test]
    fn indented_unique_marker_nests() {
        // A unique marker (`**`) opens a deeper level even when indented.
        let src = "* one\n  ** nested";
        let (_, items) = list(src);
        assert_eq!(items.len(), 1);
        let (_, sub) = sublist(&items[0]);
        assert_eq!(sub.len(), 1);
        assert_eq!(item_text(&sub[0]), "nested");
    }

    #[test]
    fn continuation_attaches_a_paragraph() {
        // `+` attaches the following paragraph as a block inside the item.
        let src = "* one\n+\nattached para\n* two";
        let (_, items) = list(src);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "one");
        assert_eq!(items[0].blocks.len(), 1);
        assert_eq!(paragraph_text(&items[0].blocks[0]), "attached para");
        assert_eq!(item_text(&items[1]), "two");
        assert!(items[1].blocks.is_empty());
    }

    #[test]
    fn continuation_attaches_a_listing() {
        let src = "* one\n+\n----\ncode\n----\n* two";
        let (_, items) = list(src);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].blocks.len(), 1);
        assert!(matches!(
            &items[0].blocks[0].kind,
            BlockKind::Listing { text } if text == "code"
        ));
        assert_eq!(item_text(&items[1]), "two");
    }

    #[test]
    fn double_continuation_attaches_two_blocks_in_order() {
        let src = "* one\n+\npara A\n+\npara B";
        let (_, items) = list(src);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].blocks.len(), 2);
        assert_eq!(paragraph_text(&items[0].blocks[0]), "para A");
        assert_eq!(paragraph_text(&items[0].blocks[1]), "para B");
    }

    #[test]
    fn item_span_covers_an_attached_block() {
        let src = "* one\n+\nattached";
        let (_, items) = list(src);
        assert_eq!(items.len(), 1);
        assert_eq!(
            &src[items[0].span.start..items[0].span.end],
            "* one\n+\nattached"
        );
    }

    #[test]
    fn explicit_numbered_marker_is_an_ordered_list() {
        let src = "1. one\n2. two\n3. three";
        let (ordered, items) = list(src);
        assert!(ordered);
        assert_eq!(items.len(), 3);
        assert_eq!(item_text(&items[0]), "one");
        assert_eq!(item_text(&items[1]), "two");
        assert_eq!(item_text(&items[2]), "three");
        // Item content is scanned after the `1. ` marker (3 bytes).
        assert!(matches!(&items[0].inlines[0], Inline::Text { value, span }
            if value == "one" && *span == Span::new(3, 6)));
    }

    #[test]
    fn multi_digit_numbers_are_siblings_not_nested() {
        // `9.`, `10.`, `11.` collapse to one marker identity, so they form a
        // single flat list — the digit count must not leak into nesting.
        let (ordered, items) = list("9. nine\n10. ten\n11. eleven");
        assert!(ordered);
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|it| it.blocks.is_empty()));
        assert_eq!(item_text(&items[2]), "eleven");
    }

    #[test]
    fn explicit_number_nests_under_implicit_dot() {
        // A `1.` marker is a *unique* identity vs the implicit `.`, so it opens
        // a sublist under the preceding `.` item rather than continuing the
        // list. (Confirmed against asciidoctor 2.0.23.)
        let (ordered, items) = list(". a\n. b\n1. c");
        assert!(ordered);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "a");
        assert_eq!(item_text(&items[1]), "b");
        let (sub_ordered, sub) = sublist(&items[1]);
        assert!(sub_ordered);
        assert_eq!(sub.len(), 1);
        assert_eq!(item_text(&sub[0]), "c");
    }

    #[test]
    fn implicit_dot_nests_under_explicit_number() {
        // The symmetric case: a `.` after a `1.` item nests, since the two
        // markers are distinct identities. (Confirmed against asciidoctor 2.0.23.)
        let (ordered, items) = list("1. a\n. b");
        assert!(ordered);
        assert_eq!(items.len(), 1);
        assert_eq!(item_text(&items[0]), "a");
        let (sub_ordered, sub) = sublist(&items[0]);
        assert!(sub_ordered);
        assert_eq!(item_text(&sub[0]), "b");
    }

    #[test]
    fn number_without_space_or_dot_is_not_a_list() {
        // `1.text` (no space) and `1) item` (no dot) are not markers → paragraph.
        assert!(matches!(
            parse("1.text").document.into_flat_blocks()[0].kind,
            BlockKind::Paragraph { .. }
        ));
        assert!(matches!(
            parse("1) item").document.into_flat_blocks()[0].kind,
            BlockKind::Paragraph { .. }
        ));
    }

    #[test]
    fn parse_block_anchor_recognises_well_formed_lines() {
        assert_eq!(parse_block_anchor("[[id]]"), Some(("id", None)));
        assert_eq!(
            parse_block_anchor("[[id,ref text]]"),
            Some(("id", Some("ref text")))
        );
        // Surrounding whitespace and a trimmed-empty reftext are tolerated.
        assert_eq!(parse_block_anchor("  [[id]]  "), Some(("id", None)));
        assert_eq!(parse_block_anchor("[[id, ]]"), Some(("id", None)));
        // Not anchors: empty id, a space in the id, embedded (inline) anchor,
        // and a single-bracket `[id]`.
        assert_eq!(parse_block_anchor("[[]]"), None);
        assert_eq!(parse_block_anchor("[[a b]]"), None);
        assert_eq!(parse_block_anchor("text [[id]]"), None);
        assert_eq!(parse_block_anchor("[id]"), None);
    }

    #[test]
    fn block_anchor_attaches_id_to_paragraph() {
        let b = &parse("[[pa]]\nHello.").document.into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("pa"));
        assert!(b.reftext.is_none());
        assert!(matches!(b.kind, BlockKind::Paragraph { .. }));
    }

    #[test]
    fn block_anchor_reftext_parses_inline_markup() {
        // asciidoctor renders `[[pa,*Bold* ref]]` reftext with the markup.
        let b = &parse("[[pa,*Bold* ref]]\nHello.")
            .document
            .into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("pa"));
        let reftext = b.reftext.as_ref().expect("reftext present");
        assert!(
            reftext.iter().any(|n| matches!(n, Inline::Strong { .. })),
            "expected strong in reftext, got {reftext:?}"
        );
    }

    #[test]
    fn block_anchor_floats_across_a_blank_line() {
        // asciidoctor lets a blank line sit between the anchor and its block.
        let b = &parse("[[pa]]\n\nHello.").document.into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("pa"));
    }

    #[test]
    fn block_anchor_before_heading_sets_block_id() {
        // The explicit anchor overrides the auto-generated section id.
        let b = &parse("[[custom]]\n== Real Title\n\nbody")
            .document
            .into_flat_blocks()[0];
        assert!(matches!(b.kind, BlockKind::Heading { level: 1, .. }));
        assert_eq!(b.id.as_deref(), Some("custom"));
    }

    #[test]
    fn block_anchor_before_listing_sets_block_id() {
        let b = &parse("[[code1]]\n----\nx = 1\n----")
            .document
            .into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("code1"));
        assert!(matches!(b.kind, BlockKind::Listing { .. }));
    }

    #[test]
    fn second_anchor_before_a_block_wins() {
        // Stacked anchors: the one nearest the block (last seen) attaches.
        let b = &parse("[[first]]\n[[second]]\nHello.")
            .document
            .into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("second"));
    }

    #[test]
    fn whole_line_anchor_is_block_not_inline() {
        // A line that is *wholly* `[[id]]` stays a block anchor on the next block
        // (no inline `Anchor` node in the paragraph text), unlike an anchor that
        // shares its line with other text. Matches asciidoctor's disambiguation.
        let b = &parse("[[pa]]\nHello.").document.into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("pa"));
        let BlockKind::Paragraph { inlines } = &b.kind else {
            panic!("expected paragraph");
        };
        assert!(
            !inlines.iter().any(|n| matches!(n, Inline::Anchor { .. })),
            "whole-line anchor must not also appear inline, got {inlines:?}"
        );
    }

    #[test]
    fn anchor_sharing_its_line_is_inline() {
        // `[[id]]` with other text on the line is an *inline* anchor: the block
        // gets no id, and the paragraph carries an `Anchor` node.
        let b = &parse("[[id]] and more text").document.into_flat_blocks()[0];
        assert!(b.id.is_none(), "inline anchor does not set the block id");
        let BlockKind::Paragraph { inlines } = &b.kind else {
            panic!("expected paragraph");
        };
        assert!(matches!(
            inlines.first(),
            Some(Inline::Anchor { id, .. }) if id == "id"
        ));
    }

    #[test]
    fn block_attr_id_sets_block_id_and_excludes_line_from_span() {
        let b = &parse("[#intro]\nHello.").document.into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("intro"));
        assert!(b.roles.is_empty());
        assert!(matches!(b.kind, BlockKind::Paragraph { .. }));
        // The block's span starts at the paragraph, not the `[#intro]\n` line,
        // matching how `[[id]]` anchor lines are excluded from the block span.
        assert_eq!(
            b.span,
            Span::new("[#intro]\n".len(), "[#intro]\nHello.".len())
        );
    }

    #[test]
    fn block_attr_role_sets_roles() {
        let b = &parse("[.lead]\nHello.").document.into_flat_blocks()[0];
        assert!(b.id.is_none());
        assert_eq!(b.roles, ["lead"]);
    }

    #[test]
    fn block_attr_combined_sets_id_and_roles() {
        let b = &parse("[#intro.lead.big]\nHi.").document.into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("intro"));
        assert_eq!(b.roles, ["lead", "big"]);
    }

    #[test]
    fn block_attr_roles_accumulate_across_lines() {
        let b = &parse("[.a]\n[.b]\nHi.").document.into_flat_blocks()[0];
        assert_eq!(b.roles, ["a", "b"]);
    }

    #[test]
    fn block_attr_floats_across_a_blank_line() {
        let b = &parse("[.lead]\n\nHello.").document.into_flat_blocks()[0];
        assert_eq!(b.roles, ["lead"]);
    }

    #[test]
    fn block_attr_id_overrides_section_auto_id() {
        // `[#id]` on a section must survive the resolve pass (which would
        // otherwise assign the auto-id `_real_title`), just like `[[id]]`.
        let b = &parse("[#custom]\n== Real Title\n\nbody")
            .document
            .into_flat_blocks()[0];
        assert!(matches!(b.kind, BlockKind::Heading { level: 1, .. }));
        assert_eq!(b.id.as_deref(), Some("custom"));
    }

    #[test]
    fn block_attr_role_on_section_keeps_auto_id() {
        let b = &parse("[.notice]\n== Hello World\n\nbody")
            .document
            .into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("_hello_world"));
        assert_eq!(b.roles, ["notice"]);
    }

    #[test]
    fn anchor_and_shorthand_id_last_wins_both_orders() {
        // Shorthand after anchor wins; anchor after shorthand wins; and a role
        // line followed by an anchor keeps the roles (anchor only sets the id).
        assert_eq!(
            parse("[[a1]]\n[#a2]\nHi.").document.into_flat_blocks()[0]
                .id
                .as_deref(),
            Some("a2")
        );
        assert_eq!(
            parse("[#a1]\n[[a2]]\nHi.").document.into_flat_blocks()[0]
                .id
                .as_deref(),
            Some("a2")
        );
        let b = &parse("[.lead]\n[[a2]]\nHi.").document.into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("a2"));
        assert_eq!(b.roles, ["lead"]);
    }

    #[test]
    fn attr_list_line_is_consumed_and_carried_on_block() {
        // `[source,ruby]` is now a recognised attribute list (it was a documented
        // limitation before): the line is consumed — not shown as text — and its
        // style/language ride on the following block. Here that is a paragraph,
        // since there is no `----` listing; id/roles stay empty.
        let doc = parse("[source,ruby]\nx = 1").document;
        assert_eq!(doc.flat_blocks().len(), 1);
        let b = &doc.flat_blocks()[0];
        assert!(b.id.is_none());
        assert!(b.roles.is_empty());
        assert_eq!(b.attrs.style.as_deref(), Some("source"));
        assert_eq!(
            b.attrs.named.get("language").map(String::as_str),
            Some("ruby")
        );
        assert!(matches!(b.kind, BlockKind::Paragraph { .. }));
        // The attribute line is excluded from the block span, like an anchor line.
        assert_eq!(
            b.span,
            Span::new("[source,ruby]\n".len(), "[source,ruby]\nx = 1".len())
        );
    }

    #[test]
    fn attr_list_attaches_style_and_language_to_listing() {
        let doc = parse("[source,ruby]\n----\nputs 1\n----").document;
        let b = &doc.flat_blocks()[0];
        assert!(matches!(b.kind, BlockKind::Listing { .. }));
        assert_eq!(b.attrs.style.as_deref(), Some("source"));
        assert_eq!(
            b.attrs.named.get("language").map(String::as_str),
            Some("ruby")
        );
    }

    #[test]
    fn attr_list_named_and_options_merge_across_lines() {
        // Consecutive attribute lines merge: named accumulate (last-wins per key),
        // options accumulate, and the whole thing floats across a blank line.
        let b = &parse("[%hardbreaks]\n[width=40]\n\nHi.")
            .document
            .into_flat_blocks()[0];
        assert_eq!(b.attrs.options, ["hardbreaks"]);
        assert_eq!(b.attrs.named.get("width").map(String::as_str), Some("40"));
        assert!(matches!(b.kind, BlockKind::Paragraph { .. }));
    }

    #[test]
    fn delimited_literal_block_parses_verbatim() {
        let src = "....\nverbatim *not bold*\n....";
        let r = parse(src);
        assert!(r.diagnostics.is_empty());
        let b = &r.document.flat_blocks()[0];
        match &b.kind {
            BlockKind::Literal { text } => assert_eq!(text, "verbatim *not bold*"),
            other => panic!("expected literal, got {other:?}"),
        }
        // Span covers the whole block, opening through closing delimiter.
        assert_eq!(b.span, Span::new(0, src.len()));
    }

    #[test]
    fn literal_style_turns_paragraph_into_literal() {
        // `[literal]` makes the following paragraph a verbatim literal block: its
        // text is the raw source (markup like `*not bold*` is NOT processed).
        let b = &parse("[literal]\nverbatim *not bold* text")
            .document
            .into_flat_blocks()[0];
        match &b.kind {
            BlockKind::Literal { text } => assert_eq!(text, "verbatim *not bold* text"),
            other => panic!("expected literal, got {other:?}"),
        }
        assert_eq!(b.attrs.style.as_deref(), Some("literal"));
        // The attribute line is excluded from the block span, as elsewhere.
        assert_eq!(
            b.span,
            Span::new(
                "[literal]\n".len(),
                "[literal]\nverbatim *not bold* text".len()
            )
        );
    }

    #[test]
    fn unterminated_literal_is_a_diagnostic_not_a_failure() {
        let r = parse("....\nverbatim");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Literal { .. }
        ));
    }

    #[test]
    fn delimited_passthrough_block_parses_verbatim() {
        // `++++` captures its body raw: no inline subs, so `{author}` and `*b*`
        // stay literal (the backend later emits the text completely unescaped).
        let src = "++++\n<div>{author} *b*</div>\n++++";
        let r = parse(src);
        assert!(r.diagnostics.is_empty());
        let b = &r.document.flat_blocks()[0];
        match &b.kind {
            BlockKind::Passthrough { text } => assert_eq!(text, "<div>{author} *b*</div>"),
            other => panic!("expected passthrough, got {other:?}"),
        }
        // Span covers the whole block, opening through closing delimiter.
        assert_eq!(b.span, Span::new(0, src.len()));
    }

    #[test]
    fn pass_style_turns_paragraph_into_passthrough() {
        // `[pass]` makes the following paragraph a raw passthrough: its text is the
        // raw source (markup like `*b*` and `{x}` is NOT processed).
        let src = "[pass]\n<b>{x}</b> *b*";
        let b = &parse(src).document.into_flat_blocks()[0];
        match &b.kind {
            BlockKind::Passthrough { text } => assert_eq!(text, "<b>{x}</b> *b*"),
            other => panic!("expected passthrough, got {other:?}"),
        }
        assert_eq!(b.attrs.style.as_deref(), Some("pass"));
        // The attribute line is excluded from the block span, as elsewhere.
        assert_eq!(b.span, Span::new("[pass]\n".len(), src.len()));
    }

    #[test]
    fn pass_style_on_open_masquerades_to_passthrough() {
        // `[pass]` on a `--` block switches its content model to a verbatim
        // passthrough at the fence (like `[literal]`/`[source]`), body kept raw.
        let src = "[pass]\n--\n<i>raw</i> {x}\n--";
        let r = parse(src);
        assert!(r.diagnostics.is_empty());
        let b = &r.document.flat_blocks()[0];
        match &b.kind {
            BlockKind::Passthrough { text } => assert_eq!(text, "<i>raw</i> {x}"),
            other => panic!("expected passthrough, got {other:?}"),
        }
        let fence = src.find("--").unwrap();
        assert_eq!(b.span, Span::new(fence, src.len()));
    }

    #[test]
    fn unterminated_passthrough_is_a_diagnostic_not_a_failure() {
        let r = parse("++++\nverbatim");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Passthrough { .. }
        ));
    }

    /// The [`AdmonitionKind`] and the inlines of an admonition's single body
    /// paragraph — the shape the paragraph and label forms always produce.
    fn admonition_para(block: &Block) -> (AdmonitionKind, &[Inline]) {
        match &block.kind {
            BlockKind::Admonition { kind, blocks, .. } => {
                assert_eq!(blocks.len(), 1, "expected a single body paragraph");
                match &blocks[0].kind {
                    BlockKind::Paragraph { inlines } => (*kind, inlines),
                    other => panic!("expected Paragraph body, got {other:?}"),
                }
            }
            other => panic!("expected Admonition, got {other:?}"),
        }
    }

    /// Convenience: the kind and the first body inline's `Text` value.
    fn admonition(block: &Block) -> (AdmonitionKind, String) {
        let (kind, inlines) = admonition_para(block);
        let text = match &inlines[0] {
            Inline::Text { value, .. } => value.clone(),
            other => panic!("expected Text, got {other:?}"),
        };
        (kind, text)
    }

    #[test]
    fn note_style_turns_paragraph_into_admonition() {
        // `[NOTE]` on a paragraph makes it an admonition; inline markup is still
        // processed (unlike `[literal]`, which is verbatim).
        let b = &parse("[NOTE]\nThis is a *note*.")
            .document
            .into_flat_blocks()[0];
        let (kind, text) = admonition(b);
        assert_eq!(kind, AdmonitionKind::Note);
        assert_eq!(text, "This is a ");
        let (_, inlines) = admonition_para(b);
        assert!(inlines.iter().any(|n| matches!(n, Inline::Strong { .. })));
        assert_eq!(b.attrs.style.as_deref(), Some("NOTE"));
        // The attribute line is excluded from the block span, as elsewhere; the
        // synthetic child paragraph spans the same range.
        let span = Span::new("[NOTE]\n".len(), "[NOTE]\nThis is a *note*.".len());
        assert_eq!(b.span, span);
        match &b.kind {
            BlockKind::Admonition { blocks, .. } => assert_eq!(blocks[0].span, span),
            _ => unreachable!(),
        }
    }

    #[test]
    fn label_prefix_turns_paragraph_into_admonition() {
        // `NOTE: ` label form. The label is stripped from the content, but the
        // span still covers the whole paragraph (label included) for source
        // fidelity, and the content's first inline starts after the prefix.
        let src = "NOTE: This is a note.";
        let b = &parse(src).document.into_flat_blocks()[0];
        let (kind, text) = admonition(b);
        assert_eq!(kind, AdmonitionKind::Note);
        assert_eq!(text, "This is a note.");
        assert!(!text.contains("NOTE"));
        assert_eq!(b.span, Span::new(0, src.len()));
        match &b.kind {
            // The admonition spans the whole paragraph, but the body paragraph
            // spans only the text after the `NOTE: ` prefix.
            BlockKind::Admonition { blocks, .. } => {
                assert_eq!(blocks[0].span, Span::new("NOTE: ".len(), src.len()));
                match &blocks[0].kind {
                    BlockKind::Paragraph { inlines } => match &inlines[0] {
                        Inline::Text { span, .. } => assert_eq!(span.start, "NOTE: ".len()),
                        other => panic!("expected Text, got {other:?}"),
                    },
                    other => panic!("expected Paragraph, got {other:?}"),
                }
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn label_prefix_inline_markup_is_processed() {
        let b = &parse("TIP: be *bold*").document.into_flat_blocks()[0];
        let (kind, inlines) = admonition_para(b);
        assert_eq!(kind, AdmonitionKind::Tip);
        assert!(inlines.iter().any(|n| matches!(n, Inline::Strong { .. })));
    }

    #[test]
    fn all_admonition_label_types_recognised() {
        for (src, want) in [
            ("NOTE: x", AdmonitionKind::Note),
            ("TIP: x", AdmonitionKind::Tip),
            ("IMPORTANT: x", AdmonitionKind::Important),
            ("WARNING: x", AdmonitionKind::Warning),
            ("CAUTION: x", AdmonitionKind::Caution),
        ] {
            let b = &parse(src).document.into_flat_blocks()[0];
            let (kind, _) = admonition(b);
            assert_eq!(kind, want, "for {src:?}");
        }
    }

    #[test]
    fn lowercase_style_and_label_are_not_admonitions() {
        // Case-sensitive, matching asciidoctor: `[note]` / `note:` stay paragraphs,
        // and a missing space after the colon (`NOTE:text`) is not an admonition.
        for src in ["[note]\ntext", "note: text", "NOTE:text"] {
            assert!(
                matches!(
                    parse(src).document.into_flat_blocks()[0].kind,
                    BlockKind::Paragraph { .. }
                ),
                "{src:?} should stay a paragraph"
            );
        }
    }

    #[test]
    fn label_admonition_folds_multiple_lines() {
        // A wrapped line after `NOTE: ` folds into the admonition body, as
        // asciidoctor does; the span covers both source lines.
        let src = "NOTE: first line\nsecond line";
        let b = &parse(src).document.into_flat_blocks()[0];
        assert_eq!(admonition(b).0, AdmonitionKind::Note);
        assert_eq!(b.span, Span::new(0, src.len()));
        let (_, inlines) = admonition_para(b);
        match &inlines[0] {
            Inline::Text { value, span } => {
                assert!(value.contains("first line"));
                assert!(value.contains("second line"));
                assert_eq!(span.start, "NOTE: ".len());
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    /// The kind and child blocks of an admonition container.
    fn admonition_blocks(block: &Block) -> (AdmonitionKind, &[Block]) {
        match &block.kind {
            BlockKind::Admonition { kind, blocks, .. } => (*kind, blocks),
            other => panic!("expected Admonition, got {other:?}"),
        }
    }

    #[test]
    fn note_delimited_block_wraps_example_children() {
        // `[NOTE]` on a `====` block becomes a delimited admonition whose body is
        // the parsed children (not verbatim): the `*strong*` is a real inline.
        let src = "[NOTE]\n====\nbody *strong*.\n====";
        let r = parse(src);
        assert!(r.diagnostics.is_empty());
        let b = &r.document.flat_blocks()[0];
        let (kind, blocks) = admonition_blocks(b);
        assert_eq!(kind, AdmonitionKind::Note);
        assert_eq!(blocks.len(), 1);
        match &blocks[0].kind {
            BlockKind::Paragraph { inlines } => {
                assert!(inlines.iter().any(|n| matches!(n, Inline::Strong { .. })));
            }
            other => panic!("expected Paragraph, got {other:?}"),
        }
        // The span covers the whole block, `====` fences included; the leading
        // attribute line is excluded as elsewhere.
        assert_eq!(b.span, Span::new("[NOTE]\n".len(), src.len()));
    }

    #[test]
    fn delimited_admonition_holds_multiple_blocks() {
        // A delimited admonition can hold several child blocks — the gain over the
        // paragraph form.
        let src = "[WARNING]\n====\nIntro.\n\n----\ncode\n----\n====";
        let doc = parse(src).document;
        let (kind, blocks) = admonition_blocks(doc.flat_blocks()[0]);
        assert_eq!(kind, AdmonitionKind::Warning);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
        match &blocks[1].kind {
            BlockKind::Listing { text } => assert_eq!(text, "code"),
            other => panic!("expected listing, got {other:?}"),
        }
    }

    #[test]
    fn lowercase_style_on_delimited_stays_example() {
        // Case-sensitive: `[note]` on a `====` block is a custom style, not an
        // admonition, so the block stays a plain example.
        let src = "[note]\n====\ntext\n====";
        let b = &parse(src).document.into_flat_blocks()[0];
        assert!(matches!(b.kind, BlockKind::Example { .. }));
        assert_eq!(b.attrs.style.as_deref(), Some("note"));
    }

    #[test]
    fn delimited_admonition_id_and_roles_on_container() {
        // id/roles attach to the admonition container; the children carry none.
        let src = "[NOTE#warn.box]\n====\ntext\n====";
        let doc = parse(src).document;
        let b = &doc.flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("warn"));
        assert_eq!(b.roles, ["box"]);
        let (kind, blocks) = admonition_blocks(b);
        assert_eq!(kind, AdmonitionKind::Note);
        assert!(blocks[0].id.is_none() && blocks[0].roles.is_empty());
    }

    #[test]
    fn cross_reference_inside_delimited_admonition_resolves() {
        // The resolve pass descends into a delimited admonition just like an
        // example: a `<<_target>>` inside picks up the section title.
        let src = "== Target\n\n[NOTE]\n====\nSee <<_target>>.\n====";
        let doc = parse(src).document;
        let (_, blocks) = admonition_blocks(doc.flat_blocks()[1]);
        let inlines = match &blocks[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            other => panic!("expected paragraph, got {other:?}"),
        };
        let xref = inlines
            .iter()
            .find_map(|i| match i {
                Inline::CrossReference { children, .. } => Some(children.clone()),
                _ => None,
            })
            .expect("no cross-reference in admonition body");
        match &xref[0] {
            Inline::Text { value, .. } => assert_eq!(value, "Target"),
            other => panic!("expected resolved Text, got {other:?}"),
        }
    }

    /// Borrow an example block's children, panicking on any other kind.
    fn example_children(block: &Block) -> &[Block] {
        match &block.kind {
            BlockKind::Example { blocks } => blocks,
            other => panic!("expected example, got {other:?}"),
        }
    }

    #[test]
    fn example_block_parses_children_not_verbatim() {
        // Unlike `----`/`....`, the body is parsed as blocks: the `*strong*` is a
        // real inline, not raw text.
        let src = "====\nHello *world*.\n====";
        let r = parse(src);
        assert!(r.diagnostics.is_empty());
        let b = &r.document.flat_blocks()[0];
        let kids = example_children(b);
        assert_eq!(kids.len(), 1);
        assert!(matches!(kids[0].kind, BlockKind::Paragraph { .. }));
        // Span covers the whole block, opening through closing delimiter.
        assert_eq!(b.span, Span::new(0, src.len()));
    }

    #[test]
    fn example_block_holds_a_nested_listing() {
        let src = "====\nIntro.\n\n----\ncode\n----\n====";
        let kids = example_children(&parse(src).document.into_flat_blocks()[0]).to_vec();
        assert_eq!(kids.len(), 2);
        assert!(matches!(kids[0].kind, BlockKind::Paragraph { .. }));
        match &kids[1].kind {
            BlockKind::Listing { text } => assert_eq!(text, "code"),
            other => panic!("expected listing, got {other:?}"),
        }
    }

    #[test]
    fn example_style_wraps_paragraph_in_container() {
        // `[example]` on a paragraph yields a single-child example block; the
        // container keeps id/roles, the child is a plain paragraph.
        let b = &parse("[#ex.box]\n[example]\nJust text.")
            .document
            .into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("ex"));
        assert_eq!(b.roles, ["box"]);
        let kids = example_children(b);
        assert_eq!(kids.len(), 1);
        assert!(matches!(kids[0].kind, BlockKind::Paragraph { .. }));
        assert!(kids[0].id.is_none() && kids[0].roles.is_empty());
    }

    #[test]
    fn example_paragraph_does_not_swallow_closing_fence() {
        // Regression: a paragraph hugging the closing `====` (no blank line) must
        // end at the fence, not fold it into the body.
        let src = "====\ntext\n====\nafter";
        let doc = parse(src).document;
        assert_eq!(doc.flat_blocks().len(), 2);
        let kids = example_children(doc.flat_blocks()[0]);
        match &kids[0].kind {
            BlockKind::Paragraph { inlines } => match &inlines[0] {
                Inline::Text { value, .. } => assert_eq!(value, "text"),
                other => panic!("expected Text, got {other:?}"),
            },
            other => panic!("expected paragraph, got {other:?}"),
        }
        assert!(matches!(
            doc.flat_blocks()[1].kind,
            BlockKind::Paragraph { .. }
        ));
    }

    #[test]
    fn block_metadata_applies_inside_an_example() {
        // A `[source,rust]` style inside an example reaches the child listing —
        // proof that style fixups and pending metadata work at any depth.
        let src = "====\n[source,rust]\n----\nlet x = 1;\n----\n====";
        let kids = example_children(&parse(src).document.into_flat_blocks()[0]).to_vec();
        assert_eq!(kids.len(), 1);
        assert!(matches!(kids[0].kind, BlockKind::Listing { .. }));
        assert_eq!(kids[0].attrs.style.as_deref(), Some("source"));
        assert_eq!(
            kids[0].attrs.named.get("language").map(String::as_str),
            Some("rust")
        );
    }

    #[test]
    fn unterminated_example_is_a_diagnostic_not_a_failure() {
        let r = parse("====\nopen but never closed");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Example { .. }
        ));
    }

    #[test]
    fn cross_reference_inside_example_resolves_against_whole_document() {
        // The resolve pass must descend into an example: a `<<_target>>` inside it
        // picks up the section title, just like one at the top level.
        let src = "== Target\n\n====\nSee <<_target>>.\n====";
        let kids = example_children(&parse(src).document.into_flat_blocks()[1]).to_vec();
        let inlines = match &kids[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            other => panic!("expected paragraph, got {other:?}"),
        };
        let xref = inlines
            .iter()
            .find_map(|i| match i {
                Inline::CrossReference { children, .. } => Some(children.clone()),
                _ => None,
            })
            .expect("no cross-reference in example body");
        match &xref[0] {
            Inline::Text { value, .. } => assert_eq!(value, "Target"),
            other => panic!("expected resolved Text, got {other:?}"),
        }
    }

    // --- sidebar blocks (`****`, `[sidebar]`) ---

    /// Borrow a sidebar block's children, panicking on any other kind.
    fn sidebar_children(block: &Block) -> &[Block] {
        match &block.kind {
            BlockKind::Sidebar { blocks } => blocks,
            other => panic!("expected sidebar, got {other:?}"),
        }
    }

    #[test]
    fn sidebar_block_parses_children_not_verbatim() {
        // Like `====`, a `****` body is parsed as blocks: `*world*` is a real inline.
        let src = "****\nHello *world*.\n****";
        let r = parse(src);
        assert!(r.diagnostics.is_empty());
        let b = &r.document.flat_blocks()[0];
        let kids = sidebar_children(b);
        assert_eq!(kids.len(), 1);
        assert!(matches!(kids[0].kind, BlockKind::Paragraph { .. }));
        // Span covers the whole block, opening through closing delimiter.
        assert_eq!(b.span, Span::new(0, src.len()));
    }

    #[test]
    fn sidebar_block_holds_a_nested_listing() {
        let src = "****\nIntro.\n\n----\ncode\n----\n****";
        let kids = sidebar_children(&parse(src).document.into_flat_blocks()[0]).to_vec();
        assert_eq!(kids.len(), 2);
        assert!(matches!(kids[0].kind, BlockKind::Paragraph { .. }));
        match &kids[1].kind {
            BlockKind::Listing { text } => assert_eq!(text, "code"),
            other => panic!("expected listing, got {other:?}"),
        }
    }

    #[test]
    fn sidebar_style_wraps_paragraph_in_container() {
        // `[sidebar]` on a paragraph yields a single-child sidebar block; the
        // container keeps id/roles, the child is a plain paragraph.
        let b = &parse("[#sb.box]\n[sidebar]\nJust text.")
            .document
            .into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("sb"));
        assert_eq!(b.roles, ["box"]);
        let kids = sidebar_children(b);
        assert_eq!(kids.len(), 1);
        assert!(matches!(kids[0].kind, BlockKind::Paragraph { .. }));
        assert!(kids[0].id.is_none() && kids[0].roles.is_empty());
    }

    #[test]
    fn sidebar_delimiter_distinct_from_unordered_marker_run() {
        // A whole-line `****` is the sidebar fence; `**** item` (content after the
        // marker run) is *not* a delimiter and stays an unordered list.
        let sb = &parse("****\ntext\n****").document.into_flat_blocks()[0];
        assert!(matches!(sb.kind, BlockKind::Sidebar { .. }));
        let list = &parse("**** item").document.into_flat_blocks()[0];
        assert!(
            matches!(list.kind, BlockKind::List { .. }),
            "got {:?}",
            list.kind
        );
    }

    #[test]
    fn sidebar_paragraph_does_not_swallow_closing_fence() {
        // Regression: a paragraph hugging the closing `****` (no blank line) must
        // end at the fence, not fold it into the body.
        let src = "****\ntext\n****\nafter";
        let doc = parse(src).document;
        assert_eq!(doc.flat_blocks().len(), 2);
        let kids = sidebar_children(doc.flat_blocks()[0]);
        match &kids[0].kind {
            BlockKind::Paragraph { inlines } => match &inlines[0] {
                Inline::Text { value, .. } => assert_eq!(value, "text"),
                other => panic!("expected Text, got {other:?}"),
            },
            other => panic!("expected paragraph, got {other:?}"),
        }
        assert!(matches!(
            doc.flat_blocks()[1].kind,
            BlockKind::Paragraph { .. }
        ));
    }

    #[test]
    fn unterminated_sidebar_is_a_diagnostic_not_a_failure() {
        let r = parse("****\nopen but never closed");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Sidebar { .. }
        ));
    }

    #[test]
    fn admonition_style_on_sidebar_stays_a_sidebar() {
        // Unlike `====` (where `[NOTE]` becomes an admonition), asciidoctor leaves
        // `[NOTE]` on a `****` block a plain sidebar — the style is inert.
        let b = &parse("[NOTE]\n****\ntext\n****")
            .document
            .into_flat_blocks()[0];
        assert!(matches!(b.kind, BlockKind::Sidebar { .. }));
        assert_eq!(b.attrs.style.as_deref(), Some("NOTE"));
    }

    #[test]
    fn cross_reference_inside_sidebar_resolves_against_whole_document() {
        // The resolve pass must descend into a sidebar: a `<<_target>>` inside it
        // picks up the section title, just like one at the top level.
        let src = "== Target\n\n****\nSee <<_target>>.\n****";
        let kids = sidebar_children(&parse(src).document.into_flat_blocks()[1]).to_vec();
        let inlines = match &kids[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            other => panic!("expected paragraph, got {other:?}"),
        };
        let xref = inlines
            .iter()
            .find_map(|i| match i {
                Inline::CrossReference { children, .. } => Some(children.clone()),
                _ => None,
            })
            .expect("no cross-reference in sidebar body");
        match &xref[0] {
            Inline::Text { value, .. } => assert_eq!(value, "Target"),
            other => panic!("expected resolved Text, got {other:?}"),
        }
    }

    // --- open blocks (`--`, `[open]`, masquerades) ---

    /// Borrow an open block's children, panicking on any other kind.
    fn open_children(block: &Block) -> &[Block] {
        match &block.kind {
            BlockKind::Open { blocks } => blocks,
            other => panic!("expected open block, got {other:?}"),
        }
    }

    #[test]
    fn open_block_parses_children_not_verbatim() {
        // Like `====`/`****`, a `--` body is parsed as blocks: `*world*` is a real
        // inline, and the span covers the opening through closing delimiter.
        let src = "--\nHello *world*.\n--";
        let r = parse(src);
        assert!(r.diagnostics.is_empty());
        let b = &r.document.flat_blocks()[0];
        let kids = open_children(b);
        assert_eq!(kids.len(), 1);
        assert!(matches!(kids[0].kind, BlockKind::Paragraph { .. }));
        assert_eq!(b.span, Span::new(0, src.len()));
    }

    #[test]
    fn open_block_holds_a_nested_listing() {
        let src = "--\nIntro.\n\n----\ncode\n----\n--";
        let kids = open_children(&parse(src).document.into_flat_blocks()[0]).to_vec();
        assert_eq!(kids.len(), 2);
        assert!(matches!(kids[0].kind, BlockKind::Paragraph { .. }));
        match &kids[1].kind {
            BlockKind::Listing { text } => assert_eq!(text, "code"),
            other => panic!("expected listing, got {other:?}"),
        }
    }

    #[test]
    fn open_style_wraps_paragraph_in_container() {
        // `[open]` on a paragraph yields a single-child open block; the container
        // keeps id/roles, the child is a plain paragraph.
        let b = &parse("[#o.box]\n[open]\nJust text.")
            .document
            .into_flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("o"));
        assert_eq!(b.roles, ["box"]);
        let kids = open_children(b);
        assert_eq!(kids.len(), 1);
        assert!(matches!(kids[0].kind, BlockKind::Paragraph { .. }));
        assert!(kids[0].id.is_none() && kids[0].roles.is_empty());
    }

    #[test]
    fn open_delimiter_is_exactly_two_hyphens() {
        // A whole-line `--` is the open-block fence; `---` (three hyphens) is not a
        // delimiter and stays a paragraph.
        let b = &parse("--\ntext\n--").document.into_flat_blocks()[0];
        assert!(matches!(b.kind, BlockKind::Open { .. }));
        let para = &parse("---\ntext\n---").document.into_flat_blocks()[0];
        assert!(
            matches!(para.kind, BlockKind::Paragraph { .. }),
            "got {:?}",
            para.kind
        );
    }

    #[test]
    fn inline_double_dash_is_not_an_open_block() {
        // Regression: an inline `--` (em dash) inside paragraph text must not be
        // mistaken for an open-block fence — the line is not a *whole-line* `--`.
        let doc = parse("an em -- dash here").document;
        assert_eq!(doc.flat_blocks().len(), 1);
        assert!(matches!(
            doc.flat_blocks()[0].kind,
            BlockKind::Paragraph { .. }
        ));
    }

    #[test]
    fn open_paragraph_does_not_swallow_closing_fence() {
        // Regression: a paragraph hugging the closing `--` (no blank line) must end
        // at the fence, not fold it into the body.
        let src = "--\ntext\n--\nafter";
        let doc = parse(src).document;
        assert_eq!(doc.flat_blocks().len(), 2);
        let kids = open_children(doc.flat_blocks()[0]);
        match &kids[0].kind {
            BlockKind::Paragraph { inlines } => match &inlines[0] {
                Inline::Text { value, .. } => assert_eq!(value, "text"),
                other => panic!("expected Text, got {other:?}"),
            },
            other => panic!("expected paragraph, got {other:?}"),
        }
        assert!(matches!(
            doc.flat_blocks()[1].kind,
            BlockKind::Paragraph { .. }
        ));
    }

    #[test]
    fn unterminated_open_is_a_diagnostic_not_a_failure() {
        let r = parse("--\nopen but never closed");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Open { .. }
        ));
    }

    #[test]
    fn quote_style_on_open_masquerades_to_quote() {
        // `[quote]` on a `--` block moves its parsed children into a quote, and the
        // positional attribution folds in just as for a `____` block.
        let src = "[quote, Anon, A Work]\n--\nMasqueraded.\n--";
        let b = &parse(src).document.into_flat_blocks()[0];
        let (attribution, blocks) = quote_parts(b);
        let attr = attribution.as_ref().expect("expected attribution");
        assert_eq!(part_text(&attr.author).as_deref(), Some("Anon"));
        assert_eq!(part_text(&attr.citetitle).as_deref(), Some("A Work"));
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
    }

    #[test]
    fn example_style_on_open_masquerades_to_example() {
        let b = &parse("[example]\n--\nx\n--").document.into_flat_blocks()[0];
        match &b.kind {
            BlockKind::Example { blocks } => assert_eq!(blocks.len(), 1),
            other => panic!("expected example, got {other:?}"),
        }
    }

    #[test]
    fn sidebar_style_on_open_masquerades_to_sidebar() {
        let b = &parse("[sidebar]\n--\nx\n--").document.into_flat_blocks()[0];
        assert!(matches!(b.kind, BlockKind::Sidebar { .. }));
    }

    #[test]
    fn admonition_style_on_open_masquerades_to_admonition() {
        // Unlike a `****` sidebar (where `[NOTE]` is inert), `[NOTE]` on a `--` block
        // folds its children into an admonition — matching asciidoctor.
        let b = &parse("[NOTE]\n--\nHeads up.\n--")
            .document
            .into_flat_blocks()[0];
        let (kind, blocks) = admonition_blocks(b);
        assert_eq!(kind, AdmonitionKind::Note);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn source_style_on_open_masquerades_to_source_listing() {
        // `[source,ruby]` on a `--` block switches its content model to a verbatim
        // source listing: the body stays raw (no inline subs) and the `source`
        // style + `language` ride on `attrs`, matching asciidoctor.
        let src = "[source,ruby]\n--\nputs *not bold*\n--";
        let r = parse(src);
        assert!(r.diagnostics.is_empty());
        let b = &r.document.flat_blocks()[0];
        match &b.kind {
            BlockKind::Listing { text } => assert_eq!(text, "puts *not bold*"),
            other => panic!("expected listing, got {other:?}"),
        }
        assert_eq!(b.attrs.style.as_deref(), Some("source"));
        assert_eq!(
            b.attrs.named.get("language").map(String::as_str),
            Some("ruby")
        );
        let fence = src.find("--").unwrap();
        assert_eq!(b.span, Span::new(fence, src.len()));
    }

    #[test]
    fn literal_style_on_open_masquerades_to_literal() {
        // `[literal]` on a `--` block: verbatim body, no inline subs (`*not bold*`
        // stays literal).
        let src = "[literal]\n--\nverbatim *not bold*\n--";
        let b = &parse(src).document.into_flat_blocks()[0];
        match &b.kind {
            BlockKind::Literal { text } => assert_eq!(text, "verbatim *not bold*"),
            other => panic!("expected literal, got {other:?}"),
        }
        let fence = src.find("--").unwrap();
        assert_eq!(b.span, Span::new(fence, src.len()));
    }

    #[test]
    fn verse_style_on_open_masquerades_to_verse() {
        // `[verse, author, work]` on a `--` block: line breaks survive, inline subs
        // still apply, and the positional attribution folds in like the `____` form.
        let src = "[verse, Carl Sandburg, Fog]\n--\nThe *fog* comes\non little cat feet.\n--";
        let r = parse(src);
        assert!(r.diagnostics.is_empty());
        let b = &r.document.flat_blocks()[0];
        let (attribution, inlines) = verse_parts(b);
        let attr = attribution.as_ref().expect("expected attribution");
        assert_eq!(part_text(&attr.author).as_deref(), Some("Carl Sandburg"));
        assert_eq!(part_text(&attr.citetitle).as_deref(), Some("Fog"));
        assert!(inlines.iter().any(|i| matches!(i, Inline::Strong { .. })));
        assert!(verse_text(inlines).contains('\n'));
        let fence = src.find("--").unwrap();
        assert_eq!(b.span, Span::new(fence, src.len()));
    }

    #[test]
    fn unterminated_verbatim_open_is_a_diagnostic_not_a_failure() {
        // An unterminated verbatim `--` masquerade still parses to a partial block
        // and emits one "unterminated open block" diagnostic, like the `____` forms.
        let r = parse("[literal]\n--\nopen but never closed");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Literal { .. }
        ));
    }

    #[test]
    fn cross_reference_inside_open_resolves_against_whole_document() {
        // The resolve pass must descend into an open block: a `<<_target>>` inside it
        // picks up the section title, just like one at the top level.
        let src = "== Target\n\n--\nSee <<_target>>.\n--";
        let kids = open_children(&parse(src).document.into_flat_blocks()[1]).to_vec();
        let inlines = match &kids[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            other => panic!("expected paragraph, got {other:?}"),
        };
        let xref = inlines
            .iter()
            .find_map(|i| match i {
                Inline::CrossReference { children, .. } => Some(children.clone()),
                _ => None,
            })
            .expect("no cross-reference in open block body");
        match &xref[0] {
            Inline::Text { value, .. } => assert_eq!(value, "Target"),
            other => panic!("expected resolved Text, got {other:?}"),
        }
    }

    // --- quote & verse blocks (`____`, `[quote]`, `[verse]`) ---

    fn quote_parts(block: &Block) -> (&Option<Attribution>, &[Block]) {
        match &block.kind {
            BlockKind::Quote {
                attribution,
                blocks,
            } => (attribution, blocks),
            other => panic!("expected quote, got {other:?}"),
        }
    }

    fn verse_parts(block: &Block) -> (&Option<Attribution>, &[Inline]) {
        match &block.kind {
            BlockKind::Verse {
                attribution,
                inlines,
            } => (attribution, inlines),
            other => panic!("expected verse, got {other:?}"),
        }
    }

    /// The concatenated `Text` content of an attribution part (single-`Text`
    /// authors/citetitles in these tests), or `None` when the part is absent.
    fn part_text(part: &Option<Vec<Inline>>) -> Option<String> {
        part.as_ref().map(|inlines| {
            inlines
                .iter()
                .filter_map(|i| match i {
                    Inline::Text { value, .. } | Inline::Raw { value, .. } => Some(value.as_str()),
                    _ => None,
                })
                .collect()
        })
    }

    /// The concatenated top-level `Text` content of a verse body (markup dropped).
    fn verse_text(inlines: &[Inline]) -> String {
        inlines
            .iter()
            .filter_map(|i| match i {
                Inline::Text { value, .. } => Some(value.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bare_quote_block_defaults_to_quote() {
        // A bare `____` block is a quote whose body is parsed as blocks, with no
        // attribution. Span covers the whole block.
        let src = "____\nA *bold* quote.\n____";
        let r = parse(src);
        assert!(r.diagnostics.is_empty());
        let b = &r.document.flat_blocks()[0];
        let (attribution, blocks) = quote_parts(b);
        assert!(attribution.is_none());
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
        assert_eq!(b.span, Span::new(0, src.len()));
    }

    #[test]
    fn delimited_quote_carries_attribution() {
        let src = "[quote, Albert Einstein, Theory of Relativity]\n____\nText.\n____";
        let r = parse(src);
        let b = &r.document.flat_blocks()[0];
        let (attribution, blocks) = quote_parts(b);
        let attr = attribution.as_ref().expect("expected attribution");
        assert_eq!(part_text(&attr.author).as_deref(), Some("Albert Einstein"));
        assert_eq!(
            part_text(&attr.citetitle).as_deref(),
            Some("Theory of Relativity")
        );
        assert_eq!(blocks.len(), 1);
        // The positional attribution is consumed, not left on `attrs`.
        assert!(b.attrs.positional.is_empty());
    }

    #[test]
    fn quote_style_wraps_paragraph_in_container() {
        let r = parse("[#q.box]\n[quote]\nShort.");
        let b = &r.document.flat_blocks()[0];
        assert_eq!(b.id.as_deref(), Some("q"));
        assert_eq!(b.roles, ["box"]);
        let (attribution, blocks) = quote_parts(b);
        assert!(attribution.is_none());
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
    }

    #[test]
    fn quote_attribution_author_only() {
        let r = parse("[quote, Carl Sandburg]\n____\nx\n____");
        let (attribution, _) = quote_parts(r.document.flat_blocks()[0]);
        let attr = attribution.as_ref().expect("expected attribution");
        assert_eq!(part_text(&attr.author).as_deref(), Some("Carl Sandburg"));
        assert!(attr.citetitle.is_none());
    }

    #[test]
    fn quote_attribution_citetitle_only() {
        // `[quote, , Fog]` — an empty author slot is preserved (matching
        // asciidoctor): citetitle only, no author.
        let r = parse("[quote, , Fog]\n____\nx\n____");
        let (attribution, _) = quote_parts(r.document.flat_blocks()[0]);
        let attr = attribution.as_ref().expect("expected attribution");
        assert!(attr.author.is_none());
        assert_eq!(part_text(&attr.citetitle).as_deref(), Some("Fog"));
    }

    #[test]
    fn attribution_follows_ruby_field_keys() {
        // Named and `None` fields occupy positional slots: `Author` is the
        // citetitle (Ruby's third positional), not the author.
        for src in [
            "[quote, role=x, Author]\n____\nx\n____",
            "[quote, role=None, Author]\nx",
        ] {
            let r = parse(src);
            let (attribution, _) = quote_parts(r.document.flat_blocks()[0]);
            let attr = attribution.as_ref().expect("expected attribution");
            assert!(attr.author.is_none(), "{src}");
            assert_eq!(part_text(&attr.citetitle).as_deref(), Some("Author"));
        }
        // A later line replaces the same key; an explicit nil clears it.
        let r = parse("[quote, A, W1]\n[quote, B]\n____\nx\n____");
        let (attribution, _) = quote_parts(r.document.flat_blocks()[0]);
        let attr = attribution.as_ref().unwrap();
        assert_eq!(part_text(&attr.author).as_deref(), Some("B"));
        assert_eq!(part_text(&attr.citetitle).as_deref(), Some("W1"));
        let r = parse("[quote,A]\n[,B]\n____\nx\n____");
        let (attribution, _) = quote_parts(r.document.flat_blocks()[0]);
        assert_eq!(
            part_text(&attribution.as_ref().unwrap().author).as_deref(),
            Some("B")
        );
        let r = parse("[quote, A]\n[quote, , W]\n--\nx\n--");
        let (attribution, _) = quote_parts(r.document.flat_blocks()[0]);
        let attr = attribution.as_ref().unwrap();
        assert!(attr.author.is_none());
        assert_eq!(part_text(&attr.citetitle).as_deref(), Some("W"));
    }

    #[test]
    fn quoted_empty_author_is_an_empty_part() {
        let r = parse("[quote,\"\",W]\nx");
        let (attribution, _) = quote_parts(r.document.flat_blocks()[0]);
        let attr = attribution.as_ref().unwrap();
        assert_eq!(attr.author.as_deref(), Some(&[][..]));
        assert_eq!(part_text(&attr.citetitle).as_deref(), Some("W"));
    }

    #[test]
    fn attribution_inlines_carry_field_spans() {
        // Unicode and CRLF before the field keep byte offsets exact. Unquoted
        // values stay raw source; single-quoted values get normal substitutions.
        let src = "Ünï\r\n\r\n[quote, *Ann* & <i>, '*Wörk*']\r\n____\r\nx\r\n____";
        let r = parse(src);
        let (attribution, _) = quote_parts(r.document.flat_blocks()[1]);
        let attr = attribution.as_ref().unwrap();
        let [Inline::Raw { value, span }] = attr.author.as_deref().unwrap() else {
            panic!("expected raw author: {attr:?}");
        };
        assert_eq!(value, "*Ann* & <i>");
        assert_eq!(&src[span.start..span.end], "*Ann* & <i>");
        let [Inline::Strong { span, .. }] = attr.citetitle.as_deref().unwrap() else {
            panic!("expected strong citetitle: {attr:?}");
        };
        assert_eq!(&src[span.start..span.end], "*Wörk*");
    }

    #[test]
    fn attribute_lines_follow_ruby_recognition() {
        // `[]` is consumed metadata; a leading blank or another first
        // character leaves the line as paragraph text.
        let r = parse("[]\npara");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert_eq!(
            r.document.flat_blocks()[0].span,
            Span::new(3, 7),
            "the consumed line is outside the paragraph"
        );
        for src in ["[ quote]\npara", "[*x]\npara", "[[x y]]\npara"] {
            let r = parse(src);
            let BlockKind::Paragraph { inlines } = &r.document.flat_blocks()[0].kind else {
                panic!("expected paragraph for {src}");
            };
            assert!(
                matches!(&inlines[0], Inline::Text { value, .. } if value.starts_with('[')),
                "{src}: {inlines:?}"
            );
        }
    }

    #[test]
    fn shorthand_warnings_point_to_the_attribute_line() {
        let src = "para\n\n[#a#b]\nx";
        let r = parse(src);
        assert_eq!(r.document.flat_blocks()[1].id.as_deref(), Some("b"));
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.message == "multiple ids detected in style attribute")
            .expect("warning");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(&src[d.span.start..d.span.end], "[#a#b]");
    }

    #[test]
    fn delimited_verse_preserves_newlines() {
        let r = parse("[verse]\n____\nThe fog comes\non little cat feet.\n____");
        let (attribution, inlines) = verse_parts(r.document.flat_blocks()[0]);
        assert!(attribution.is_none());
        assert_eq!(verse_text(inlines), "The fog comes\non little cat feet.");
    }

    #[test]
    fn verse_carries_attribution_and_inline_markup() {
        let r = parse("[verse, Carl Sandburg, Fog]\n____\nThe *fog* comes\non feet.\n____");
        let (attribution, inlines) = verse_parts(r.document.flat_blocks()[0]);
        let attr = attribution.as_ref().expect("expected attribution");
        assert_eq!(part_text(&attr.author).as_deref(), Some("Carl Sandburg"));
        assert_eq!(part_text(&attr.citetitle).as_deref(), Some("Fog"));
        // Inline markup applies (a Strong node) while the line break survives.
        assert!(inlines.iter().any(|i| matches!(i, Inline::Strong { .. })));
        assert!(verse_text(inlines).contains('\n'));
    }

    #[test]
    fn verse_style_paragraph_preserves_newlines() {
        // The paragraph form re-scans without folding, so a wrapped multi-line
        // `[verse]` paragraph keeps its breaks.
        let r = parse("[verse]\nRoses are red,\nviolets are blue.");
        let (_, inlines) = verse_parts(r.document.flat_blocks()[0]);
        assert_eq!(verse_text(inlines), "Roses are red,\nviolets are blue.");
    }

    #[test]
    fn verse_text_span_points_at_real_source() {
        // The verse body keeps its newline, and the `Text` span maps to the real
        // bytes (so the editor can locate it).
        let src = "[verse]\n____\nab\ncd\n____";
        let r = parse(src);
        let (_, inlines) = verse_parts(r.document.flat_blocks()[0]);
        let body_start = src.find("ab").unwrap();
        match &inlines[0] {
            Inline::Text { value, span } => {
                assert_eq!(value, "ab\ncd");
                assert_eq!(*span, Span::new(body_start, body_start + 5));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_quote_is_a_diagnostic_not_a_failure() {
        let r = parse("____\nopen but never closed");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Quote { .. }
        ));
    }

    #[test]
    fn unterminated_verse_is_a_diagnostic_not_a_failure() {
        let r = parse("[verse]\n____\nopen but never closed");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Verse { .. }
        ));
    }

    #[test]
    fn cross_reference_inside_quote_resolves_against_whole_document() {
        let src = "== Target\n\n____\nSee <<_target>>.\n____";
        let r = parse(src);
        let (_, blocks) = quote_parts(r.document.flat_blocks()[1]);
        let inlines = match &blocks[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            other => panic!("expected paragraph, got {other:?}"),
        };
        let xref = inlines
            .iter()
            .find_map(|i| match i {
                Inline::CrossReference { children, .. } => Some(children.clone()),
                _ => None,
            })
            .expect("no cross-reference in quote body");
        match &xref[0] {
            Inline::Text { value, .. } => assert_eq!(value, "Target"),
            other => panic!("expected resolved Text, got {other:?}"),
        }
    }

    // --- block titles (`.Title`) ---

    /// The first `Text` inline of a block's title, or `None` if it has no title.
    fn block_title_text(block: &Block) -> Option<String> {
        block.title.as_ref().map(|inlines| match &inlines[0] {
            Inline::Text { value, .. } => value.clone(),
            other => panic!("expected Text in title, got {other:?}"),
        })
    }

    #[test]
    fn parse_block_title_recognises_and_disambiguates() {
        // A leading `.` + a non-space, non-dot char is a title; the marker dot is
        // stripped, an inner dot kept (asciidoctor's `BlockTitleRx`).
        assert_eq!(parse_block_title(".Title"), Some("Title"));
        assert_eq!(parse_block_title(".A caption"), Some("A caption"));
        assert_eq!(parse_block_title("..foo"), Some(".foo"));
        assert_eq!(parse_block_title(".NET release"), Some("NET release"));
        assert_eq!(parse_block_title(".Title  "), Some("Title")); // trailing trimmed
                                                                  // Not titles: ordered-list marker (dot + space), the `....`/`..` delimiters,
                                                                  // a bare dot, and a line that does not start with a dot.
        assert_eq!(parse_block_title(". item"), None);
        assert_eq!(parse_block_title("...."), None);
        assert_eq!(parse_block_title(".."), None);
        assert_eq!(parse_block_title("."), None);
        assert_eq!(parse_block_title("plain"), None);
    }

    #[test]
    fn block_title_attaches_to_paragraph() {
        let b = &parse(".A caption\nBody text.").document.into_flat_blocks()[0];
        assert_eq!(block_title_text(b).as_deref(), Some("A caption"));
        assert!(matches!(b.kind, BlockKind::Paragraph { .. }));
        // The title line is metadata; the paragraph body is just its own text.
        assert_eq!(paragraph_text(b), "Body text.");
    }

    #[test]
    fn block_title_span_points_at_real_source() {
        // The title text begins right after the marker dot (byte offset 1).
        let b = &parse(".Caption\nbody").document.into_flat_blocks()[0];
        let title = b.title.as_ref().expect("title present");
        match &title[0] {
            Inline::Text { value, span } => {
                assert_eq!(value, "Caption");
                assert_eq!(*span, Span::new(1, 1 + "Caption".len()));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn block_title_parses_inline_markup() {
        // Inline substitutions apply in a title (asciidoctor renders the markup).
        let b = &parse(".A *bold* caption\nbody").document.into_flat_blocks()[0];
        let title = b.title.as_ref().expect("title present");
        assert!(
            title.iter().any(|n| matches!(n, Inline::Strong { .. })),
            "expected strong in title, got {title:?}"
        );
    }

    #[test]
    fn block_title_floats_across_a_blank_line() {
        // As with anchors / attribute lines, a blank line may sit before the block.
        let b = &parse(".Caption\n\nbody").document.into_flat_blocks()[0];
        assert_eq!(block_title_text(b).as_deref(), Some("Caption"));
    }

    #[test]
    fn second_block_title_wins() {
        let b = &parse(".first\n.second\nbody").document.into_flat_blocks()[0];
        assert_eq!(block_title_text(b).as_deref(), Some("second"));
    }

    #[test]
    fn dot_space_is_an_ordered_list_not_a_title() {
        // `. item` is an ordered-list marker, never a title.
        let b = &parse(". item").document.into_flat_blocks()[0];
        assert!(b.title.is_none());
        assert!(matches!(b.kind, BlockKind::List { ordered: true, .. }));
    }

    #[test]
    fn block_title_on_listing_and_example() {
        let listing = &parse(".Code\n----\nx = 1\n----")
            .document
            .into_flat_blocks()[0];
        assert_eq!(block_title_text(listing).as_deref(), Some("Code"));
        assert!(matches!(listing.kind, BlockKind::Listing { .. }));

        let example = &parse(".Demo\n====\ninside\n====")
            .document
            .into_flat_blocks()[0];
        assert_eq!(block_title_text(example).as_deref(), Some("Demo"));
        assert!(matches!(example.kind, BlockKind::Example { .. }));
    }

    #[test]
    fn block_title_before_note_style_stays_on_the_admonition() {
        // A `.Title` before `[NOTE]` attaches to the resulting admonition container,
        // not the wrapped child paragraph (confirmed vs asciidoctor: the block title
        // renders alongside the `Note` label).
        let b = &parse(".My note\n[NOTE]\nNote body.")
            .document
            .into_flat_blocks()[0];
        assert_eq!(block_title_text(b).as_deref(), Some("My note"));
        let BlockKind::Admonition { kind, blocks, .. } = &b.kind else {
            panic!("expected admonition, got {:?}", b.kind);
        };
        assert_eq!(*kind, AdmonitionKind::Note);
        assert!(blocks[0].title.is_none(), "child must not carry the title");
    }
}
