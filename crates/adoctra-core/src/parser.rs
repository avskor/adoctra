//! Parser: builds a [`ParseResult`] from source text.
//!
//! Error-tolerant by design: it never returns `Err` for malformed syntax.
//! Whatever can be parsed becomes blocks (with spans); problems become
//! [`Diagnostic`]s. This is what lets an editor parse on every keystroke while
//! the buffer is half-written.

use crate::ast::{
    AdmonitionKind, Attribution, BlankEdges, Block, BlockAttrs, BlockKind, Document, Inline,
    ListItem, Span,
};
use crate::attributes::Attributes;
use crate::block_attributes::parse_block_attr_list;
use crate::comments::{comment_fence, is_line_comment, skip_comment};
use crate::context::{consume_attribute_entry, ParseContext, ParseOptions};
use crate::diagnostic::{Diagnostic, ParseResult, Severity};
use crate::header::{collect_header, setext_level};
use crate::indentation::Indentation;
use crate::inline::scan_inlines_report;
use crate::reader::Reader;
use crate::ruby_strip::RubyStrip;
use crate::tokenizer::TokenKind;
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
    /// A style the block sets over the metadata (fenced code is `source`).
    pub style: Option<String>,
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
            style: None,
        }
    }
}

/// Ruby's `BlockMediaMacroRx` branch of `next_block`: the macro list (with
/// the media's positional names) gets the early a-stage like an attribute
/// line, then the target its attribute references. A target emptied by
/// `drop-line` drops the block and its metadata.
fn read_media_macro(
    line: &str,
    off: usize,
    (media, target, list): (
        crate::image::Media,
        std::ops::Range<usize>,
        std::ops::Range<usize>,
    ),
    context: &mut ParseContext,
    includes: &crate::xref_target::Includes,
) -> Option<Block> {
    let attr_line = crate::block_attributes::AttrLine {
        inner: line[list.clone()].to_owned(),
        base: off + list.start,
    };
    let (meta, _) =
        crate::block_attributes::macro_list(attr_line, media.posattrs(), context, includes);
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
    let kind = match media {
        crate::image::Media::Image => BlockKind::Image {
            target: value,
            src: String::new(),
            alt: Vec::new(),
            svg: None,
        },
        crate::image::Media::Video => BlockKind::Video {
            target: value,
            src: String::new(),
            poster: None,
            embedded: false,
        },
        crate::image::Media::Audio => BlockKind::Audio {
            target: value,
            src: String::new(),
        },
    };
    Some(Block::new(Span::new(off, off + line.len()), kind))
}

/// Complete a media block once its metadata is merged: Ruby drops the style,
/// which for an image becomes the alt text without an explicit one, and makes
/// an image's unitless `scaledwidth` a percentage.
fn complete_media(block: &mut Block, attributes: &Attributes) {
    match &mut block.kind {
        BlockKind::Image {
            target, src, alt, ..
        } => {
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
        BlockKind::Video { .. } | BlockKind::Audio { .. } => {
            block.attrs.style = None;
            crate::media::resolve(block, attributes);
        }
        _ => {}
    }
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
            "role" => {
                meta.roles = Some(
                    text.split(crate::ruby_strip::is_ruby_space)
                        .filter(|role| !role.is_empty())
                        .map(str::to_owned)
                        .collect(),
                )
            }
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
    parse_reader(options, Reader::new(source), None).0
}

pub(crate) fn parse_with_start(
    source: &str,
    options: &ParseOptions,
    start: &mut dyn crate::ConversionStart,
) -> ParseResult {
    parse_reader(options, Reader::new(source), Some(start)).0
}

pub(crate) fn parse_reader<'a>(
    options: &ParseOptions,
    mut lines: Reader<'a>,
    start: Option<&mut dyn crate::ConversionStart>,
) -> (ParseResult, Reader<'a>) {
    lines.set_primary_id(options.source_id);
    lines.enable_unread_includes(options);
    let images = crate::data_uri::Images::new(lines.image_source());
    let mut context = ParseContext::new(options);
    context.images = Some(&images);
    lines.skip_primary_front_matter(&mut context);
    let mut header = collect_header(&mut lines, &mut context);
    let mut doc = Document {
        attributes: (*context.attributes).clone(),
        final_attributes: Attributes::default(),
        header: header.metadata,
        blocks: Vec::new(),
        has_title: header.effective_title.is_some(),
        footnotes: Vec::new(),
        manpage: header.manpage.take(),
        htmlsyntax: context.htmlsyntax(),
        output: None,
        toc: None,
    };
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
    let invalid_header = header.invalid;
    let mut i = header.body_start;
    context.first_body_line = None;
    let body = collect_blocks(
        &mut lines,
        &mut i,
        &mut context,
        None,
        header.body_pending.unwrap_or(header.pending),
    );
    context.close_sections(0, usize::MAX);
    let mut body = nest_sections(
        body,
        TopLevel {
            has_header: doc.has_title,
            invalid_header,
            first_line: context.first_body_line,
            fragment: doc.attributes.get("fragment").is_some(),
        },
        book,
        doc.attributes.get("preface-title").filter(|_| book),
        &mut context,
        &lines,
    );
    check_open_styles(&mut body, book, &mut context.body_subs.excluded);
    // Ruby's reader reaches the end of the input before conversion.
    lines.finish(&mut context);
    doc.blocks.extend(body);

    let mut references = Vec::new();
    let registry = context.register_references(&mut doc, &lines.include_catalog, &mut references);
    name_attributes(&mut doc);
    // Everything reported so far (placed by an earlier stamp, or a duplicate
    // id registration reported without one) is Ruby's load; the rest is
    // conversion.
    let loaded = context.stamp();
    let registered = references.len();
    let inline_tokens = context.execute_body(
        &mut doc,
        registry,
        &lines.include_catalog,
        &mut references,
        start,
    );
    let visible_footnotes = visible_footnote_origins(&mut doc);
    let name_heading = doc
        .manpage
        .as_mut()
        .and_then(|manpage| manpage.name_section.as_mut())
        .map(|section| &mut section.heading);
    doc.footnotes = collect_footnotes_after(
        name_heading,
        &mut doc.blocks,
        std::mem::take(&mut doc.footnotes),
        &visible_footnotes,
    );

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
    context.comments.sort_by_key(|span| (span.start, span.end));
    // A folded list paragraph is scanned again as the item text.
    context.comments.dedup();
    lines.comments = context.comments.clone();
    // One stream in Ruby's reporting order: reports placed earlier than
    // adoctra determined them precede the stream entry they were stamped at.
    let stream = std::mem::take(&mut context.diagnostics);
    let mut placed = std::mem::take(&mut context.placed);
    placed.sort_by_key(|placed| placed.stamp);
    let mut placed = placed.into_iter().peekable();
    let mut diagnostics = Vec::with_capacity(stream.len() + placed.len() + references.len());
    let mut map = |mut diagnostic: Diagnostic, physical: bool| {
        if !physical {
            lines.map_diagnostic(&mut diagnostic, &mut provenance);
        }
        diagnostic
    };
    let converted = |mut diagnostic: Diagnostic, conversion: bool| {
        diagnostic.report.conversion = conversion;
        diagnostic
    };
    for (index, diagnostic) in stream.into_iter().enumerate() {
        while let Some(early) = placed.next_if(|placed| placed.stamp.at <= index) {
            let conversion = early.stamp.seq > loaded.seq;
            diagnostics.push(converted(map(early.diagnostic, early.physical), conversion));
        }
        diagnostics.push(converted(map(diagnostic, false), index >= loaded.at));
    }
    diagnostics.extend(placed.map(|late| {
        let conversion = late.stamp.seq > loaded.seq;
        converted(map(late.diagnostic, late.physical), conversion)
    }));
    for (index, mut diagnostic) in references.into_iter().enumerate() {
        lines.map_diagnostic(&mut diagnostic, &mut provenance);
        diagnostic.report.conversion = index >= registered;
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
    let mut result = ParseResult {
        tokens: Vec::new(),
        document: doc,
        diagnostics,
        comments,
        conditional_directives,
        inactive,
        include_directives,
        provenance,
    };
    result.tokens = crate::tokenizer::project(&mut result, &lines, inline_tokens);
    (result, lines)
}

/// A manual page's NAME section resolves its references with the registry
/// (before the body, like Ruby converting it in the header): its id and,
/// when the header took it from the section, its title become `manname-id`
/// and `manname-title`.
fn name_attributes(doc: &mut Document) {
    let Some((section, title)) = doc.manpage.as_ref().and_then(|manpage| {
        let section = manpage.name_section.as_ref()?;
        section.body.map(|_| (section, manpage.section_title))
    }) else {
        return;
    };
    let heading = &section.heading;
    if let Some(id) = &heading.id {
        doc.attributes.set("manname-id", id.clone());
    }
    if let (true, BlockKind::Heading { inlines, .. }) = (title, &heading.kind) {
        doc.attributes
            .set_parts("manname-title", crate::resolve::label_parts(inlines));
    }
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
    stop: Option<&str>,
    pending: PendingMeta,
) -> Vec<Block> {
    collect_blocks_in(lines, i, context, stop, pending, Scope::Body)
}

/// Where [`collect_blocks_in`] reads blocks.
#[derive(Clone, Copy)]
enum Scope {
    /// The document body (without `stop`) or a delimited container's body.
    Body,
    /// A list item: its list type and whether its first block is text only.
    ListItem(crate::lists::ListType, bool),
    /// A compound block read from its own lines (a Markdown blockquote),
    /// where sections are not recognized.
    Container,
}

/// The mutable state a block reading step works on: the reader of the
/// innermost run (the document's, or a list item's or blockquote's own) and
/// the shared parse context.
struct Env<'e, 's, 'c> {
    lines: &'e mut Reader<'s>,
    i: &'e mut usize,
    context: &'e mut ParseContext<'c>,
}

/// A nested run of blocks that a suspended step waits for: a container's
/// body up to `stop`, or the lines of a list item or blockquote in their own
/// reader.
struct Collect<'s> {
    stop: Option<String>,
    pending: PendingMeta,
    scope: Scope,
    reader: Option<Reader<'s>>,
    /// Reported when reading ahead to `stop` reaches the end, as Ruby's
    /// `read_lines_until` does before the body is parsed.
    unterminated: Option<Diagnostic>,
}

/// The rest of a suspended step, given the nested run's blocks and its own
/// reader (to absorb); it runs with the enclosing run's reader.
type Resume<'s, 'c> =
    Box<dyn for<'e> FnOnce(&mut Env<'e, 's, 'c>, Vec<Block>, Option<Reader<'s>>) -> Step<'s, 'c>>;

/// Work on the finished block of a suspended step, innermost first.
type Then<'s, 'c> = Box<dyn for<'e> FnOnce(&mut Env<'e, 's, 'c>, Option<Block>) -> Option<Block>>;

/// A block reading step: finished (`parse_block`'s result), or suspended on a
/// nested run, with the work to do on its eventual block. Nesting lives on
/// [`read_blocks`]' heap stack, not the call stack, so no document depth can
/// overflow it.
// A step lives briefly and moves a few times per block; boxing every block
// would cost more than the size difference.
#[allow(clippy::large_enum_variant)]
enum Step<'s, 'c> {
    Done(Option<Block>),
    Collect(Box<Collect<'s>>, Resume<'s, 'c>, Vec<Then<'s, 'c>>),
}

impl<'s, 'c> Step<'s, 'c> {
    /// Apply `then` to the finished block, now or when the step finishes.
    fn then(
        self,
        env: &mut Env<'_, 's, 'c>,
        then: impl for<'e> FnOnce(&mut Env<'e, 's, 'c>, Option<Block>) -> Option<Block> + 'static,
    ) -> Step<'s, 'c> {
        match self {
            Step::Done(block) => Step::Done(then(env, block)),
            Step::Collect(collect, resume, mut thens) => {
                thens.push(Box::new(then));
                Step::Collect(collect, resume, thens)
            }
        }
    }

    /// Apply the outer steps' work after this step's own.
    fn then_all(self, env: &mut Env<'_, 's, 'c>, outer: Vec<Then<'s, 'c>>) -> Step<'s, 'c> {
        match self {
            Step::Done(mut block) => {
                for then in outer {
                    block = then(env, block);
                }
                Step::Done(block)
            }
            Step::Collect(collect, resume, mut thens) => {
                thens.extend(outer);
                Step::Collect(collect, resume, thens)
            }
        }
    }
}

/// One run of [`collect_blocks_in`]: its loop state between blocks.
struct Run<'s, 'c> {
    stop: Option<String>,
    previous_limit: Option<usize>,
    scope: Scope,
    blocks: Vec<Block>,
    text_only: bool,
    /// Blank lines before the next block's metadata (Ruby's `skipped`).
    skipped: usize,
    metadata_seen: bool,
    pending: PendingMeta,
    /// The metadata and saved entries of the block being parsed.
    parsing: Option<(PendingMeta, crate::context::Entries)>,
    /// A list item's or blockquote's own reader and position.
    reader: Option<(Reader<'s>, usize)>,
    resume: Option<(Resume<'s, 'c>, Vec<Then<'s, 'c>>)>,
}

impl<'s, 'c> Run<'s, 'c> {
    /// Start a run. Ruby first preprocesses and captures a container up to its
    /// closing fence, then parses the body: a nested comment/verbatim region
    /// cannot hide that fence.
    fn begin(
        collect: Box<Collect<'s>>,
        resume: Option<(Resume<'s, 'c>, Vec<Then<'s, 'c>>)>,
        env: &mut Env<'_, 's, 'c>,
    ) -> Self {
        let Collect {
            stop,
            pending,
            scope,
            reader,
            unterminated,
        } = *collect;
        let mut reader = reader.map(|reader| (reader, 0));
        let bounded = stop.as_deref().map(|fence| match &mut reader {
            Some((reader, j)) => reader.bound_until(*j, env.context, |line| line == fence),
            None => env
                .lines
                .bound_until(*env.i, env.context, |line| line == fence),
        });
        if let (Some((_, false)), Some(diagnostic)) = (bounded, unterminated) {
            env.context.diagnostics.push(diagnostic);
        }
        let previous_limit = bounded.map(|(limit, _)| limit);
        Run {
            stop,
            previous_limit,
            scope,
            blocks: Vec::new(),
            text_only: matches!(scope, Scope::ListItem(_, true)),
            skipped: 0,
            metadata_seen: false,
            pending,
            parsing: None,
            reader,
            resume,
        }
    }

    /// Continue the run with the result of the block being parsed, if any:
    /// returns the context to parse the next block in, or `None` at its end.
    fn advance(
        &mut self,
        env: &mut Env<'_, 's, 'c>,
        parsed: Option<Option<Block>>,
    ) -> Option<BlockCtx> {
        if let Some(parsed) = parsed {
            self.finish_block(env, parsed);
        }
        let Env {
            lines, i, context, ..
        } = env;
        while lines.has(**i, context) {
            let (_, line_buffer) = lines.line(**i);
            let line = line_buffer.as_ref();
            if line.is_ruby_blank() {
                if !self.metadata_seen {
                    self.skipped += 1;
                }
                **i += 1;
                continue;
            }
            // A text-only block takes no title or attribute entry (Ruby's
            // `parse_block_metadata_line` with `:text_only`).
            let text_only_now = self.text_only && self.skipped == 0;
            if !(text_only_now && line.starts_with(['.', ':']))
                && consume_block_metadata(lines, i, context, &mut self.pending)
            {
                self.metadata_seen = true;
                continue;
            }
            let metadata = std::mem::take(&mut self.pending);
            let ctx = match self.scope {
                Scope::ListItem(list_type, _) => BlockCtx::InListItem {
                    list_type,
                    text_only: self.text_only && self.skipped == 0,
                    adjacent: self.skipped == 0,
                },
                Scope::Body if self.stop.is_none() => BlockCtx::Section,
                Scope::Body | Scope::Container => BlockCtx::TopLevel,
            };
            self.text_only = false;
            self.skipped = 0;
            self.metadata_seen = false;
            let entries = context.flush_entries();
            self.parsing = Some((metadata, entries));
            return Some(ctx);
        }
        context.drop_entries();
        if let Some(limit) = self.previous_limit {
            lines.limit = limit;
        }
        None
    }

    /// The block metadata a parsed block takes, its fixups and substitutions.
    fn finish_block(&mut self, env: &mut Env<'_, 's, 'c>, parsed: Option<Block>) {
        let Env { lines, context, .. } = env;
        let (lines, context) = (&mut **lines, &mut **context);
        let (metadata, entries) = self.parsing.take().expect("a block being parsed");
        if parsed.is_none() {
            context.restore_entries(entries);
        }
        let mut carried = None;
        if let Some(mut block) = parsed {
            let pending = metadata;
            // A section-title anchor supplies the id only without one from
            // metadata, but its reftext replaces a pending reftext.
            block.id = pending.id.or(block.id.take());
            let mut reftext = context.section_reftext.take().or(pending.reftext);
            block.roles = pending.roles;
            let mut title = pending.title;
            let mut macro_meta = context.block_macro.take();
            // A block that sets its own style (fenced code, a Markdown or
            // quoted paragraph blockquote) does not rekey positional fields.
            let restyled = macro_meta.as_ref().is_some_and(|meta| meta.style.is_some());
            // A fenced code block forces the `source` style.
            let fenced = macro_meta
                .as_ref()
                .filter(|meta| meta.style.as_deref() == Some("source"))
                .map(|meta| meta.named.iter().any(|(key, _)| key == "language"));
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
            // A list keeps its implicit (ordered) style without an explicit one.
            let own_style = block.attrs.style.take();
            let own_options = std::mem::take(&mut block.attrs.options);
            block.attrs = pending.attrs;
            if block.attrs.style.is_none() {
                block.attrs.style = own_style;
            }
            for option in own_options {
                if !block.attrs.options.contains(&option) {
                    block.attrs.options.push(option);
                }
            }
            if let Some(meta) = macro_meta {
                if meta.style.is_some() {
                    // The block's own style replaces the metadata style,
                    // whose `source` rekeying does not apply.
                    (block.attrs.named, block.attrs.positional) =
                        crate::block_attributes::unkeyed_fields(&pending.list);
                    block.attrs.style = meta.style;
                }
                block.attrs.named.extend(meta.named);
                for option in meta.options {
                    if !block.attrs.options.contains(&option) {
                        block.attrs.options.push(option);
                    }
                }
            }
            complete_media(&mut block, &context.attributes);
            let attributes = inline_attributes(&context.render_attributes(), Some(&block.attrs));
            // Ruby builds a styled paragraph (or a label paragraph) with simple content.
            let simple = matches!(
                block.kind,
                BlockKind::Paragraph { .. } | BlockKind::Admonition { .. }
            );
            apply_style_fixup(&mut block, lines, &attributes, context);
            if !restyled {
                rekey_attribution(&mut block, &pending.list);
            }
            source_defaults(&mut block, &pending.list, fenced, &context.attributes);
            adjust_verbatim(&mut block, context);
            if let BlockKind::Stem { style, .. } = &mut block.kind {
                if block.attrs.style.as_deref() == Some("stem") {
                    *style = crate::StemStyle::from_alias(
                        pending
                            .list
                            .value(2)
                            .map(|v| v.text.as_str())
                            .or(context.attributes.get("stem")),
                    );
                }
                block.attrs.style = Some(style.as_str().into());
            }
            trim_verbatim_edges(&mut block);
            let simple = simple || block.simple;
            commit_subs(&mut block, simple, lines, &attributes, context);
            apply_callouts(&mut block, lines, context);
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
                } else if !matches!(
                    block.kind,
                    BlockKind::Passthrough { .. } | BlockKind::ThematicBreak | BlockKind::PageBreak
                ) {
                    block.title = Some(context.body_subs.defer_title(
                        &title.text,
                        title.base,
                        title.origin,
                        &context.attributes,
                    ));
                }
                // An unrendered passthrough or break title retains its source
                // text; it must neither execute expressions nor issue inline
                // warnings.
            }
            // Ruby registers the id here, after converting an anchored title;
            // in a book, a part's content is checked here too.
            if block.id.is_some() || context.book {
                let stamp = context.stamp();
                context
                    .block_stamps
                    .insert((block.span.start, block.span.end), stamp);
            }
            block.attribute_context = Arc::clone(&context.attributes);
            self.blocks.push(block);
        }
        if let Some(title) = carried.take() {
            self.pending.title = Some(title);
        }
    }
}

/// Read a run of blocks without recursion: nested runs (container bodies,
/// list items, blockquotes) are frames of a heap stack, so a deeply nested
/// document cannot overflow the call stack (user decision 2026-10-31).
fn read_blocks<'s, 'c>(
    lines: &mut Reader<'s>,
    i: &mut usize,
    context: &mut ParseContext<'c>,
    collect: Collect<'s>,
) -> Vec<Block> {
    /// Run `f` with the innermost reader of `stack` (or the document's).
    fn with_env<'s, 'c, R>(
        stack: &mut [Run<'s, 'c>],
        lines: &mut Reader<'s>,
        i: &mut usize,
        context: &mut ParseContext<'c>,
        f: impl FnOnce(&mut Env<'_, 's, 'c>, &mut [Run<'s, 'c>]) -> R,
    ) -> R {
        let owner = stack.iter().rposition(|run| run.reader.is_some());
        let mut own = owner.and_then(|owner| stack[owner].reader.take());
        let result = {
            let (lines, i) = match &mut own {
                Some((reader, j)) => (reader, j),
                None => (lines, i),
            };
            let mut env = Env { lines, i, context };
            f(&mut env, stack)
        };
        if let Some(owner) = owner {
            stack[owner].reader = own;
        }
        result
    }

    let mut stack = Vec::new();
    let first = {
        let mut env = Env {
            lines: &mut *lines,
            i: &mut *i,
            context: &mut *context,
        };
        Run::begin(Box::new(collect), None, &mut env)
    };
    stack.push(first);
    let mut parsed = None;
    loop {
        let step = with_env(&mut stack, lines, i, context, |env, stack| {
            let outermost = stack.len() == 1;
            let top = stack.last_mut().expect("a run");
            let ctx = top.advance(env, parsed.take())?;
            if outermost
                && matches!(ctx, BlockCtx::Section)
                && env.context.first_body_line.is_none()
            {
                env.context.first_body_line = Some(env.lines[*env.i].0);
            }
            let (metadata, _) = top.parsing.as_ref().expect("metadata");
            Some(parse_block_step(
                env,
                ctx,
                Some(&metadata.attrs),
                metadata.id.is_some(),
            ))
        });
        let step = match step {
            Some(step) => step,
            None => {
                let mut run = stack.pop().expect("a run");
                let blocks = std::mem::take(&mut run.blocks);
                let Some((resume, thens)) = run.resume.take() else {
                    return blocks;
                };
                let reader = run.reader.map(|(reader, _)| reader);
                with_env(&mut stack, lines, i, context, |env, _| {
                    resume(env, blocks, reader).then_all(env, thens)
                })
            }
        };
        match step {
            Step::Done(block) => parsed = Some(block),
            Step::Collect(collect, resume, thens) => {
                let run = with_env(&mut stack, lines, i, context, |env, _| {
                    Run::begin(collect, Some((resume, thens)), env)
                });
                stack.push(run);
            }
        }
    }
}

/// The blocks of a container, of a list item, or of a compound block read
/// from its own lines, as `scope` gives.
fn collect_blocks_in(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    stop: Option<&str>,
    pending: PendingMeta,
    scope: Scope,
) -> Vec<Block> {
    read_blocks(
        lines,
        i,
        context,
        Collect {
            stop: stop.map(str::to_owned),
            pending,
            scope,
            reader: None,
            unterminated: None,
        },
    )
}

/// Ruby's `next_section`: nest the document-level blocks under their section
/// titles. A title opens a subsection of the innermost open section with a lower
/// level and closes the others; levels that skip one warn, and a level-0 section
/// outside a book is an error but still nests under the document. Blocks before
/// the first section form a preamble when the document has a title, unless there
/// are no sections at all (except for a book, as in Ruby).
/// What the document header decided for the top-level section checks.
struct TopLevel {
    has_header: bool,
    /// Ruby's `invalid-header`: a block title above the doctitle.
    invalid_header: bool,
    /// Ruby's `fragment`: any top-level section level is expected.
    fragment: bool,
    /// Where the first body block starts ([`ParseContext::first_body_line`]).
    first_line: Option<usize>,
}

fn nest_sections(
    blocks: Vec<Block>,
    top_level: TopLevel,
    book: bool,
    preface_title: Option<&str>,
    context: &mut ParseContext<'_>,
    lines: &Reader<'_>,
) -> Vec<Block> {
    use crate::section_numbers::Ordinals;
    // Reading has ended for these blocks: the parts still open close here.
    let reading = context.stamp();
    let sections = &mut context.sections;
    let diagnostics = &mut context.diagnostics;
    let placed = &mut context.placed;
    let block_stamps = &context.block_stamps;
    let TopLevel {
        has_header,
        invalid_header,
        fragment,
        first_line,
    } = top_level;
    // Hierarchy diagnostics, placed where Ruby reports them: at the title
    // (`reading` for the parts closing at the end).
    let mut inserted: Vec<(crate::context::Stamp, Diagnostic)> = Vec::new();
    // Close the innermost open section into its parent (or the top level).
    // Like Ruby, a book part must end with a nested section; Ruby reports a
    // part without one at `reader.cursor` when its reading stops: the title
    // line of the section that closes it, or the end of the input (`cursor`).
    fn close(
        open: &mut Vec<Block>,
        top: &mut Vec<Block>,
        book: bool,
        diagnostics: &mut Vec<(crate::context::Stamp, Diagnostic)>,
        at: crate::context::Stamp,
        cursor: crate::DiagnosticLocation,
    ) {
        let mut section = open.pop().unwrap();
        if let BlockKind::Section { level, blocks } = &section.kind {
            let last = blocks.last().unwrap();
            if book && *level == 0 && !matches!(last.kind, BlockKind::Section { .. }) {
                diagnostics.push((at, Diagnostic {
                    omitted_span: None,
                    report: crate::DiagnosticReport {
                        location: cursor,
                        ..Default::default()
                    },
                    span: blocks[0].span,
                    severity: Severity::Error,
                    message: "invalid part, must have at least one section (e.g., chapter, appendix, etc.)"
                        .into(),
                }));
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
    // create every top-level section directly, without checking its level,
    // unless a block title cancelled the header (Ruby's `invalid-header`).
    // Ruby peeks the first body line (`is_next_line_section?`), so a dropped
    // first block (a `[comment]` paragraph, a `drop-line` block macro) makes
    // the following section title a checked one.
    let unchecked = !has_header
        && !invalid_header
        && blocks.first().is_some_and(|block| {
            section_level(block).is_some() && first_line == Some(block.span.start)
        });
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
                        // Ruby reports a part's illegal content when reading
                        // completes the block.
                        let stamp = block_stamps
                            .get(&(block.span.start, block.span.end))
                            .copied();
                        part_content(
                            blocks,
                            intros.last_mut().unwrap(),
                            block,
                            &mut |diagnostic| match stamp {
                                Some(stamp) => placed.push(crate::context::Placed {
                                    stamp,
                                    diagnostic,
                                    physical: false,
                                }),
                                None => diagnostics.push(diagnostic),
                            },
                        )
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
        // Ruby reports while reading the title (and closes the previous part
        // just before it).
        let at = sections
            .get(&key)
            .map_or(reading, |section| section.diagnostics_before.min(reading));
        while matches!(
            open.last(),
            Some(Block { kind: BlockKind::Section { level: current, .. }, .. }) if raw <= *current
        ) {
            let cursor = lines.report_cursor(block.span.start).map_or(
                crate::DiagnosticLocation::Source,
                crate::DiagnosticLocation::Cursor,
            );
            close(&mut open, &mut top, book, &mut inserted, at, cursor);
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
        // Ruby reports the level checks at `reader.cursor` after peeking the
        // title's lines, so a directive dropped by that peek moves the line.
        let cursor_report = crate::DiagnosticReport {
            location: lines.section_cursor(block.span.start).map_or(
                crate::DiagnosticLocation::Source,
                crate::DiagnosticLocation::Cursor,
            ),
            ..Default::default()
        };
        let expected = match parent {
            Some((_, Some(parent))) if !parent.allows_nesting() => {
                inserted.push((
                    at,
                    Diagnostic {
                        omitted_span: None,
                        report: cursor_report.clone(),
                        span: block.span,
                        severity: Severity::Error,
                        message: format!(
                            "{} sections do not support nested sections",
                            parent.sectname
                        ),
                    },
                ));
                None
            }
            Some((level, _)) => Some(level.saturating_add(1)),
            _ if unchecked => None,
            _ if raw == 0 => {
                if !book {
                    inserted.push((
                        at,
                        Diagnostic {
                            omitted_span: None,
                            report: cursor_report.clone(),
                            span: block.span,
                            severity: Severity::Error,
                            message: "level 0 sections can only be used when doctype is book"
                                .into(),
                        },
                    ));
                }
                None
            }
            _ if fragment => None,
            _ => Some(1),
        };
        if let Some(expected) = expected.filter(|expected| *expected != raw) {
            let condition = if book && open.is_empty() {
                format!("expected levels 0 or {expected}")
            } else {
                format!("expected level {expected}")
            };
            inserted.push((
                at,
                Diagnostic {
                    omitted_span: None,
                    report: cursor_report,
                    span: block.span,
                    severity: Severity::Warning,
                    message: format!("section title out of sequence: {condition}, got level {raw}"),
                },
            ));
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
        let cursor = crate::DiagnosticLocation::Cursor(lines.eof_cursor());
        close(&mut open, &mut top, book, &mut inserted, reading, cursor);
    }
    placed.extend(
        inserted
            .into_iter()
            .map(|(stamp, diagnostic)| crate::context::Placed {
                stamp,
                diagnostic,
                physical: false,
            }),
    );
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
    report: &mut dyn FnMut(Diagnostic),
) {
    let partintro = || Some("partintro".to_string());
    if blocks.len() == 1 {
        let style = block.attrs.style.as_deref();
        match (&block.kind, style) {
            // A `[partintro]` paragraph is already an open block holding it;
            // Ruby makes it compound (a single paragraph).
            (_, Some("partintro")) => block.simple = false,
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
            Intro::None => report(Diagnostic {
                omitted_span: None,
                report: Default::default(),
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
                    first.simple = false;
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
    /// Any other container, including descriptions, callout list items and
    /// AsciiDoc table cells (a nested document, never a book part).
    Other,
}

/// Find the abstract and partintro blocks that Ruby's HTML converter
/// excludes: an abstract directly in a book document (no preamble) and a
/// partintro outside a book part. Their content is not visited. The
/// converter logs them without a cursor when it converts the block, so the
/// diagnostic waits in `excluded` for the conversion walk.
fn check_open_styles(
    blocks: &mut [Block],
    book: bool,
    excluded: &mut std::collections::BTreeMap<(usize, usize), Diagnostic>,
) {
    struct Check<'a> {
        book: bool,
        parents: Vec<OpenParent>,
        excluded: &'a mut std::collections::BTreeMap<(usize, usize), Diagnostic>,
    }
    impl crate::visit::VisitorMut for Check<'_> {
        fn enter_block(&mut self, block: &mut Block) -> bool {
            let parent = *self.parents.last().unwrap_or(&OpenParent::Document);
            if let BlockKind::Open { .. } = &block.kind {
                let exclusion = match block.attrs.style.as_deref() {
                    Some("abstract") if self.book && parent == OpenParent::Document => Some((
                        Severity::Warning,
                        "abstract block cannot be used in a document without a doctitle when doctype is book. Excluding block content.",
                    )),
                    Some("partintro") if !self.book || parent != OpenParent::Part => Some((
                        Severity::Error,
                        "partintro block can only be used when doctype is book and must be a child of a book part. Excluding block content.",
                    )),
                    _ => None,
                };
                if let Some((severity, message)) = exclusion {
                    let diagnostic = Diagnostic {
                        omitted_span: None,
                        report: crate::DiagnosticReport {
                            location: crate::DiagnosticLocation::Unlocated,
                            ..Default::default()
                        },
                        span: block.span,
                        severity,
                        message: message.into(),
                    };
                    self.excluded
                        .insert((block.span.start, block.span.end), diagnostic);
                    return false;
                }
            }
            self.parents.push(match &block.kind {
                BlockKind::Section { level: 0, .. } => OpenParent::Part,
                _ => OpenParent::Other,
            });
            true
        }

        fn leave_block(&mut self, _block: &mut Block) {
            self.parents.pop();
        }

        fn visit_inlines(
            &mut self,
            _inlines: &mut Vec<Inline>,
            _context: crate::visit::InlineContext,
        ) {
        }
    }
    crate::visit::walk_blocks_mut(
        &mut Check {
            book,
            parents: Vec::new(),
            excluded,
        },
        blocks,
    );
}

/// Consume metadata once, both before header detection and at body boundaries.
pub(crate) fn consume_block_metadata(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    pending: &mut PendingMeta,
) -> bool {
    // At the end of input there is no metadata line (a header ending the
    // document, e.g. a manual page without a body).
    if !lines.has(*i, context) {
        return false;
    }
    let (_, line_buffer) = lines.line(*i);
    let line = line_buffer.as_ref();
    if skip_comment(lines, i, context) {
        return true;
    }
    if consume_attribute_entry(lines, i, context) {
        return true;
    }
    if let Some((id, reftext)) = parse_block_anchor(line) {
        lines.record_fences(
            Span::new(lines[*i].0, lines[*i].0 + line.len()),
            None,
            TokenKind::Anchor,
        );
        // Ruby assigns the id even when it is nil (`[[]]`).
        pending.id = id.map(str::to_string);
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
        lines.record_fences(span, None, TokenKind::BlockAttributeList);
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
        lines.record_fences(
            Span::new(lines[*i].0, lines[*i].0 + line.len()),
            None,
            TokenKind::BlockTitle,
        );
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
    // single child paragraph; on a `====` example or `--` open block its
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
                    icons: None,
                    src: String::new(),
                };
                block.simple = true;
            }
            // A `====` example and a `--` open block fold their parsed
            // children into the admonition: only these delimiters list
            // `admonition` among their masquerades (Ruby's `DELIMITED_BLOCKS`),
            // so `[NOTE]` on `____` or `****` is inert.
            BlockKind::Example { blocks } | BlockKind::Open { blocks } => {
                let blocks = std::mem::take(blocks);
                block.kind = BlockKind::Admonition {
                    kind,
                    label: context.admonition_label(kind, Some(&block.attrs)),
                    blocks,
                    icons: None,
                    src: String::new(),
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
                block.kind = BlockKind::Literal {
                    text,
                    callouts: Vec::new(),
                    inlines: None,
                };
            }
            // `[listing]`/`[source]` on a paragraph: a listing block with the
            // same verbatim body (Ruby's strict verbatim paragraphs).
            "listing" | "source" => {
                let text = lines.content(block.span.start, block.span.end).text;
                block.kind = BlockKind::Listing {
                    text,
                    callouts: Vec::new(),
                    inlines: None,
                    blank_edges: BlankEdges::default(),
                };
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
                block.kind = BlockKind::Passthrough {
                    text,
                    inlines: None,
                };
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
                let indentation = Indentation::of_block(&block.attrs.named, &context.attributes);
                let inlines = lines.scan_verse(
                    block.span.start,
                    block.span.end,
                    attrs,
                    indentation,
                    context,
                );
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
        // The wrapping styles keep Ruby's simple content model.
        block.simple = matches!(
            block.kind,
            BlockKind::Example { .. }
                | BlockKind::Sidebar { .. }
                | BlockKind::Quote { .. }
                | BlockKind::Open { .. }
        );
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
}

/// Rekey the positional attributes (`[quote, author, work]`) into the quote /
/// verse attribution — the `____` fence form (with or without a style, like
/// `[, author, work]`) and the paragraph form. Like Ruby's
/// `AttributeList.rekey`, a positional value wins over a named `attribution`
/// or `citetitle`. Consumed from `attrs.positional` so they are not also
/// surfaced raw.
fn rekey_attribution(block: &mut Block, list: &crate::block_attributes::BlockList) {
    if !matches!(
        block.kind,
        BlockKind::Quote { .. } | BlockKind::Verse { .. }
    ) {
        return;
    }
    block.attrs.positional.clear();
    let attribution = build_attribution(
        list.value(2).or_else(|| list.named_value("attribution")),
        list.value(3).or_else(|| list.named_value("citetitle")),
    );
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
        Some(Attribution {
            author,
            citetitle,
            trailing: Vec::new(),
        })
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
                | "verse"
        )
}

/// Ruby's `build_block` for verbatim content: the pending block's `indent`
/// and (else the document's) `tabsize`.
fn verbatim_indentation(pending_attrs: Option<&BlockAttrs>, context: &ParseContext) -> Indentation {
    let empty = BTreeMap::new();
    let named = pending_attrs.map_or(&empty, |attrs| &attrs.named);
    Indentation::of_block(named, &context.attributes)
}

/// Ruby's `:listing`/`:source` and `:fenced_code` branches of `next_block`.
/// A listing without a style becomes a source block when its second
/// positional or the document's `source-language` gives a language. A source
/// block takes `source-language` without a language of its own (a fence's
/// metadata `language` is replaced), `linenums` from the `linenums` option or
/// `source-linenums-option`, and `indent` from `source-indent`. `fenced` tells
/// whether a fence line gave the language.
fn source_defaults(
    block: &mut Block,
    list: &crate::block_attributes::BlockList,
    fenced: Option<bool>,
    document: &Attributes,
) {
    if !matches!(block.kind, BlockKind::Listing { .. }) {
        return;
    }
    let named = &mut block.attrs.named;
    let default_language = document.get("source-language").map(str::to_owned);
    match fenced {
        Some(has_language) => {
            if let Some(language) = default_language.filter(|_| !has_language) {
                named.insert("language".to_owned(), language);
            }
        }
        None => match block.attrs.style.as_deref() {
            Some("source") => {
                if let Some(language) = default_language.filter(|_| !named.contains_key("language"))
                {
                    named.insert("language".to_owned(), language);
                }
            }
            None => {
                let Some(language) = list.value(2).map(|v| v.text.clone()).or(default_language)
                else {
                    return;
                };
                block.attrs.style = Some("source".to_owned());
                named.insert("language".to_owned(), language);
                crate::block_attributes::rekey_linenums(list, named);
            }
            Some(_) => return,
        },
    }
    if !named.contains_key("linenums")
        && (block.attrs.options.iter().any(|o| o == "linenums")
            || named.contains_key("linenums-option")
            || document.get("source-linenums-option").is_some())
    {
        named.insert("linenums".to_owned(), String::new());
    }
    if !named.contains_key("indent") {
        if let Some(indent) = document.get("source-indent") {
            named.insert("indent".to_owned(), indent.to_owned());
        }
    }
}

/// Ruby's `build_block` adjusts the lines of a listing or literal block
/// (`indent`, `tabsize`); an indented literal paragraph keeps its own
/// adjustment. A `subs` attribute converts the adjusted body.
fn adjust_verbatim(block: &mut Block, context: &mut ParseContext) {
    let literal = matches!(block.kind, BlockKind::Literal { .. });
    let (BlockKind::Listing { text, .. } | BlockKind::Literal { text, .. }) = &mut block.kind
    else {
        return;
    };
    let span = block.span;
    let entry = context.verbatim_bodies.entry(span.start).or_insert((
        span.start,
        span.end,
        Indentation::default(),
    ));
    // An indented (literal) paragraph keeps Ruby's `adjust_indentation!`
    // without a tab size, also under a style it ignores; only `[literal]`
    // builds the block with `tabsize`.
    if literal && block.attrs.style.as_deref() != Some("literal") && entry.0 == span.start {
        return;
    }
    let indentation = Indentation::of_block(&block.attrs.named, &context.attributes);
    entry.2 = indentation;
    *text = indentation.apply_text(text);
}

/// Ruby's `Block#content` for verbatim and raw blocks: blank lines at either
/// edge of a multi-line body are dropped (a single line is kept as is).
fn trim_verbatim_edges(block: &mut Block) {
    let (text, edges) = match &mut block.kind {
        BlockKind::Listing {
            text, blank_edges, ..
        } => (text, Some(blank_edges)),
        BlockKind::Literal { text, .. } | BlockKind::Passthrough { text, .. } => (text, None),
        _ => return,
    };
    if !text.contains('\n') {
        return;
    }
    let blank = |line: &&str| {
        line.trim_end_matches([' ', '\t', '\r', '\n', '\x0b', '\x0c', '\0'])
            .is_empty()
    };
    let lines: Vec<&str> = text.split('\n').collect();
    let start = lines
        .iter()
        .position(|line| !blank(line))
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|line| !blank(line))
        .map_or(start, |i| i + 1);
    if let Some(edges) = edges {
        edges.leading = start;
        edges.trailing = lines.len() - end;
    }
    *text = lines[start..end].join("\n");
}

/// Ruby's layout breaks in `next_block` (with Markdown syntax): a line of three
/// or more `'` (thematic break) or `<` (page break), or Markdown's thematic
/// break (`MarkdownThematicBreakRx`, also indented by up to three spaces).
fn layout_break(line: &str) -> Option<BlockKind> {
    let markdown = |line: &str| {
        let mut chars = line.chars();
        let c = chars.next().filter(|c| matches!(c, '-' | '*' | '_'))?;
        let rest = chars.as_str();
        let spaces = " ".repeat(rest.len() - rest.trim_start_matches(' ').len());
        (rest == format!("{spaces}{c}{spaces}{c}")).then_some(BlockKind::ThematicBreak)
    };
    let uniform = |c: char| line.len() >= 3 && line.chars().all(|x| x == c);
    if uniform('\'') {
        Some(BlockKind::ThematicBreak)
    } else if uniform('<') {
        Some(BlockKind::PageBreak)
    } else if line.starts_with(' ') {
        let indented = line.trim_start_matches(' ');
        (line.len() - indented.len() <= 3)
            .then(|| markdown(indented))
            .flatten()
    } else {
        markdown(line)
    }
}

/// The attributes of a fenced code block's opening line, as Ruby's
/// `:fenced_code` branch: the text after the backticks up to a comma is the
/// language (a leading comma keeps the whole text), and text after the comma
/// sets `linenums`.
fn fenced_code_attributes(line: &str) -> Vec<(String, String)> {
    let mut named = Vec::new();
    let rest = line.get(3..).unwrap_or("");
    let language = match rest.find(',') {
        Some(0) => {
            if rest.len() > 1 {
                named.push(("linenums".to_owned(), String::new()));
            }
            rest
        }
        Some(comma) => {
            if comma + 1 < rest.len() {
                named.push(("linenums".to_owned(), String::new()));
            }
            rest[..comma].ruby_strip()
        }
        None => rest.ruby_lstrip(),
    };
    if !language.is_empty() {
        named.push(("language".to_owned(), language.to_owned()));
    }
    named
}

/// Read a compound delimited block whose opening `fence` is at `*i`: its body
/// is parsed as blocks up to the identical closing line, like Ruby's
/// `build_block`. Returns the blocks and the block's end offset; an
/// unterminated block ends at its last child (or the opening fence when empty),
/// and the missing close is a diagnostic, not a parse failure.
fn read_container_step<'s, 'c>(
    lines: &mut Reader<'s>,
    i: &mut usize,
    fence: String,
    block_name: &'static str,
    kind: TokenKind,
    build: impl FnOnce(Vec<Block>, usize) -> Block + 'static,
) -> Step<'s, 'c> {
    let (start, opening_line) = lines.line(*i);
    let opening = Span::new(start, start + opening_line.len());
    *i += 1;
    // Ruby reads the body up to the closing fence before parsing it, so an
    // unterminated container warns before its children do (and after the
    // preprocessor's reports on its lines).
    let unterminated = Diagnostic {
        omitted_span: None,
        report: crate::DiagnosticReport::unterminated(block_name),
        span: Span::new(start, start + fence.len()),
        severity: Severity::Error,
        message: format!("unterminated {block_name} block (missing closing `{fence}`)"),
    };
    Step::Collect(
        Box::new(Collect {
            stop: Some(fence.clone()),
            pending: PendingMeta::default(),
            scope: Scope::Body,
            reader: None,
            unterminated: Some(unterminated),
        }),
        Box::new(move |env, blocks, _| {
            let Env { lines, i, context } = env;
            let closing = if lines.has(**i, context) && *lines[**i].1 == *fence {
                let (loff, lline) = lines.line(**i);
                **i += 1;
                Some(Span::new(loff, loff + lline.len()))
            } else {
                None
            };
            lines.record_fences(opening, closing, kind);
            let end = closing.map_or_else(
                || blocks.last().map_or(opening.end, |b| b.span.end),
                |c| c.end,
            );
            Step::Done(Some(build(blocks, end)))
        }),
        Vec::new(),
    )
}

/// The block context Ruby names in `unterminated <context> block`: the
/// delimiter's own context, or the masquerading style it accepts
/// (`DELIMITED_BLOCKS`; `source` reads as `listing`, an admonition style as
/// `admonition`, the STEM styles as `stem`).
fn unterminated_context(delimiter: &'static str, style: Option<&str>) -> &'static str {
    let admonition = matches!(
        style,
        Some("NOTE" | "TIP" | "IMPORTANT" | "WARNING" | "CAUTION" | "admonition")
    );
    match (delimiter, style) {
        ("open", Some("quote")) => "quote",
        ("open", Some("verse")) => "verse",
        ("open", Some("literal")) => "literal",
        ("open", Some("listing" | "source")) => "listing",
        ("open", Some("pass")) => "pass",
        ("open", Some("example")) => "example",
        ("open", Some("sidebar")) => "sidebar",
        ("open" | "example", _) if admonition => "admonition",
        ("pass", Some("stem" | "latexmath" | "asciimath")) => "stem",
        (delimiter, _) => delimiter,
    }
}

/// Capture the verbatim body of a delimited block whose opening `fence` has just
/// been consumed (so `*i` points at the first body line). Scans lines until the
/// identical closing `fence`, returning `(body_start, body_end, block_end)` as byte
/// offsets into the source, the closing fence's span and whether the body has
/// no line at all (Ruby's empty `lines`). The body slice is taken verbatim — the caller decides
/// whether to inline-scan it (verse) or keep it raw (literal/source). An
/// unterminated block pushes a `block_name` diagnostic but is never a failure.
/// Shared by the `____` verse branch and the `--` verbatim masquerades.
fn capture_verbatim_body(
    lines: &mut Reader<'_>,
    i: &mut usize,
    opening: Span,
    fence: &str,
    block_name: &str,
    context: &mut ParseContext,
) -> (usize, usize, usize, Option<Span>, bool) {
    let body_start = if lines.has(*i, context) {
        lines[*i].0
    } else {
        opening.end
    };
    let mut body_end = body_start;
    let mut end = opening.end;
    let mut closed = false;
    let mut empty = true;
    while lines.has(*i, context) {
        let (loff, lline_buffer) = lines.line(*i);
        let lline = lline_buffer.as_ref();
        if lline == fence {
            end = loff + lline.len();
            *i += 1;
            closed = true;
            break;
        }
        body_end = loff + lline.len();
        end = body_end;
        empty = false;
        *i += 1;
    }
    if !closed {
        context.diagnostics.push(Diagnostic {
            omitted_span: None,
            report: crate::DiagnosticReport::unterminated(block_name),
            span: Span::new(opening.start, opening.start + fence.len()),
            severity: Severity::Error,
            message: format!("unterminated {block_name} block (missing closing `{fence}`)"),
        });
    }
    let closing = closed.then(|| Span::new(end - fence.len(), end));
    (body_start, body_end, end, closing, empty)
}

/// Whether a block has an option (`%name`, `options=name` or `name-option`).
pub(crate) fn has_option(attrs: &BlockAttrs, name: &str) -> bool {
    attrs.options.iter().any(|o| o == name) || attrs.named.contains_key(&format!("{name}-option"))
}

/// Ruby's `:table` branch of `next_block`: the lines up to the identical
/// closing fence, without line comments, parsed by `parse_table`. Cell text
/// is deferred to conversion like paragraphs; leading cell anchors are
/// cataloged while reading.
fn read_table(
    lines: &mut Reader<'_>,
    i: &mut usize,
    opening: Span,
    hint: char,
    attrs: Option<&BlockAttrs>,
    context: &mut ParseContext,
) -> Block {
    use crate::ast::{CellContent, Table, TableCell, TableColumn};
    let fence = lines.content(opening.start, opening.end).text;
    let limit = lines.bound(*i, context, |line| line == fence);
    let mut body = Vec::new();
    while lines.has(*i, context) {
        let (off, line) = lines.line(*i);
        if is_line_comment(&line) {
            context.comments.push(Span::new(off, off + line.len()));
        } else {
            body.push((off, line));
        }
        *i += 1;
    }
    lines.limit = limit;
    let end = if lines.has(*i, context) && *lines[*i].1 == *fence {
        let (off, line) = lines.line(*i);
        lines.record_fences(
            Span::new(off, off + line.len()),
            None,
            TokenKind::TableDelimiter,
        );
        *i += 1;
        off + line.len()
    } else {
        context.diagnostics.push(Diagnostic {
            omitted_span: None,
            report: Default::default(),
            span: opening,
            severity: Severity::Warning,
            message: "unterminated table block".into(),
        });
        lines
            .loaded
            .get(i.saturating_sub(1))
            .map_or(opening.end, |(off, line)| {
                (off + line.len()).max(opening.end)
            })
    };
    let default = BlockAttrs::default();
    let attrs = attrs.unwrap_or(&default);
    let named = |key: &str| attrs.named.get(key).map(String::as_str);
    let body: Vec<(usize, &str)> = body
        .iter()
        .map(|(off, line)| (*off, line.as_ref()))
        .collect();
    let base = context.diagnostics.len();
    let parsed = crate::table::parse(
        &body,
        hint,
        &crate::table::Options {
            nested: context.nested,
            cols: named("cols"),
            format: named("format"),
            separator: named("separator"),
            header: has_option(attrs, "header"),
            noheader: has_option(attrs, "noheader"),
            footer: has_option(attrs, "footer"),
        },
        opening,
        &mut context.diagnostics,
    );
    lines.record_fences(opening, None, TokenKind::TableDelimiter);
    lines.table_separators.extend(parsed.separators);
    let columns = parsed.columns;
    // Complete the cells in Ruby's order: an AsciiDoc cell reads its nested
    // document when it is created, a leading anchor is cataloged then (or
    // when the header row is partitioned). Cells Ruby drops are created too
    // (the fourth section), so their side effects remain. Each cell's
    // diagnostics follow the table diagnostics reported before it.
    let mut table_diagnostics = context.diagnostics.split_off(base).into_iter();
    let mut flushed = 0;
    let sections = [parsed.head, parsed.body, parsed.foot, vec![parsed.dropped]];
    let mut order: Vec<(usize, usize, usize, usize)> = Vec::new();
    for (s, rows) in sections.iter().enumerate() {
        for (r, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                order.push((cell.order, s, r, c));
            }
        }
    }
    order.sort_unstable();
    let mut done: Vec<Vec<Vec<Option<TableCell>>>> = sections
        .iter()
        .map(|rows| rows.iter().map(|row| vec![None; row.len()]).collect())
        .collect();
    let render = context.render_attributes();
    for (_, s, r, c) in order {
        let cell = &sections[s][r][c];
        let mark = cell.mark.saturating_sub(base);
        if mark > flushed {
            context
                .diagnostics
                .extend(table_diagnostics.by_ref().take(mark - flushed));
            flushed = mark;
        }
        if cell.catalog {
            let source = crate::table::source(&cell.text);
            context.catalog_leading_anchor(&crate::table::text(&cell.text), |range| {
                source.original_span(Span::new(range.start, range.end))
            });
        }
        let asciidoc = !cell.literal && cell.style == Some(crate::ast::CellStyle::Asciidoc);
        if s == 3 && !asciidoc {
            // A dropped cell's text is never converted (Ruby's `Cell#text`
            // is lazy); only its creation-time effects happen.
            continue;
        }
        if s == 3 {
            // A dropped AsciiDoc cell is read (its warnings and ids count)
            // but never output.
            let (blocks, _, _) =
                read_nested_document(lines, &cell.text, (cell.span.start, cell.span.end), context);
            context.dropped_blocks.insert(cell.span.start, blocks);
            context
                .body_subs
                .dropped
                .insert((cell.span.start, cell.span.end));
            continue;
        }
        let content = if cell.literal {
            CellContent::Literal(crate::table::text(&cell.text))
        } else if asciidoc {
            let (blocks, has_title, attributes) =
                read_nested_document(lines, &cell.text, (cell.span.start, cell.span.end), context);
            CellContent::Blocks {
                blocks,
                footnotes: Vec::new(),
                has_title,
                attributes,
                toc: None,
            }
        } else {
            // The cell text substitutes as one field, like Ruby's
            // `Cell#text`; a body cell's converted text splits into
            // paragraphs afterwards (see body_subs.rs, `Cell#content`).
            let runs = if cell.text.is_empty() {
                Vec::new()
            } else {
                vec![cell.text.clone()]
            };
            CellContent::Paragraphs(
                runs.iter()
                    .map(|run| {
                        context.body_subs.defer(
                            crate::table::source(run),
                            None,
                            &render,
                            cell.style != Some(crate::ast::CellStyle::Literal),
                        )
                    })
                    .collect(),
            )
        };
        done[s][r][c] = Some(TableCell {
            content,
            style: cell.style,
            halign: cell.halign,
            valign: cell.valign,
            colspan: cell.colspan,
            rowspan: cell.rowspan,
            background: None,
            span: cell.span,
        });
    }
    context.diagnostics.extend(table_diagnostics);
    let mut done = done.into_iter().map(|rows| {
        rows.into_iter()
            .map(|row| row.into_iter().flatten().collect())
            .collect::<Vec<Vec<TableCell>>>()
    });
    let (head, body, foot) = (
        done.next().unwrap_or_default(),
        done.next().unwrap_or_default(),
        done.next().unwrap_or_default(),
    );
    let table = Table {
        columns: columns
            .iter()
            .map(|column| TableColumn {
                halign: column.halign,
                valign: column.valign,
                style: column.style,
                width: column.width(),
                autowidth: column.autowidth,
            })
            .collect(),
        head,
        body,
        foot,
        width: crate::table::table_width(named("width")),
        frame: "all".into(),
        grid: "all".into(),
        stripes: None,
    };
    Block::new(
        Span::new(opening.start, end),
        BlockKind::Table(Box::new(table)),
    )
}

/// A lazy title may convert while another document (an AsciiDoc cell) is
/// active. Its source node, not that temporary registry, owns its definition.
type FootnoteOrigins = std::collections::BTreeSet<(crate::SourceId, usize, usize)>;

fn visible_footnote_origins(document: &mut crate::Document) -> FootnoteOrigins {
    #[derive(Default)]
    struct Origins(FootnoteOrigins);
    impl crate::visit::VisitorMut for Origins {
        fn visit_inline(&mut self, inline: &mut Inline) {
            crate::substitution_context::inline_footnotes(inline, &mut |footnote, span| {
                if footnote.sequence.is_some() {
                    self.0.insert((span.source_id, span.start, span.end));
                }
            });
            crate::visit::walk_inline_mut(self, inline);
        }
    }
    let mut origins = Origins::default();
    crate::visit::walk_document_mut(&mut origins, document);
    origins.0
}

/// Footnotes of `blocks`, after those of a block read before them (a manual
/// page's NAME title). Visible fields supply final resolved content; the
/// registration catalog retains definitions without a visible use.
fn collect_footnotes_after(
    first: Option<&mut Block>,
    blocks: &mut [Block],
    registered: Vec<crate::ast::FootnoteDefinition>,
    visible: &FootnoteOrigins,
) -> Vec<crate::ast::FootnoteDefinition> {
    use crate::visit::{InlineContext, VisitorMut};
    struct Collect<'a>(
        BTreeMap<usize, crate::ast::FootnoteDefinition>,
        &'a FootnoteOrigins,
    );
    impl Collect<'_> {
        fn add(&mut self, footnote: &crate::Footnote, span: Span) {
            if let (Some(sequence), Some(index)) = (footnote.sequence, &footnote.index) {
                self.0
                    .entry(sequence)
                    .or_insert_with(|| crate::ast::FootnoteDefinition {
                        index: index.clone(),
                        id: footnote.id.clone(),
                        content: footnote.content.clone(),
                        span,
                    });
            }
        }
    }
    impl VisitorMut for Collect<'_> {
        fn enter_block(&mut self, block: &mut Block) -> bool {
            let BlockKind::Table(table) = &mut block.kind else {
                return true;
            };
            if let Some(title) = &mut block.title {
                self.visit_inlines(title, InlineContext::Title);
            }
            for cell in table.cells_mut() {
                match &mut cell.content {
                    crate::ast::CellContent::Paragraphs(paragraphs) => {
                        for inlines in paragraphs {
                            self.visit_inlines(inlines, InlineContext::Body);
                        }
                    }
                    crate::ast::CellContent::Blocks {
                        blocks, footnotes, ..
                    } => {
                        *footnotes = collect_footnotes_after(
                            None,
                            blocks,
                            std::mem::take(footnotes),
                            self.1,
                        );
                    }
                    crate::ast::CellContent::Literal(_) => {}
                }
            }
            // A cell's own footnotes are collected above, not walked.
            false
        }

        fn visit_inline(&mut self, inline: &mut Inline) {
            crate::substitution_context::inline_footnotes(inline, &mut |footnote, span| {
                self.add(footnote, span)
            });
            crate::visit::walk_inline_mut(self, inline);
        }
    }
    let mut collect = Collect(BTreeMap::new(), visible);
    if let Some(first) = first {
        crate::visit::walk_block_mut(&mut collect, first);
    }
    crate::visit::walk_blocks_mut(&mut collect, blocks);
    for (sequence, definition) in registered.into_iter().enumerate() {
        let span = definition.span;
        if !visible.contains(&(span.source_id, span.start, span.end)) {
            collect.0.entry(sequence).or_insert(definition);
        }
    }
    collect.0.into_values().collect()
}

/// Ruby's verbatim `callouts` substitution for a listing or literal block:
/// the marks ending its lines are cataloged (`catalog_callouts`) and replaced
/// (`sub_callouts`, numbering `<.>` per block); an escaped mark loses its
/// backslash. Each mark's span is found on the block's source line with the
/// same ending.
/// Ruby's `commit_subs` and `catalog_callouts`: a `subs` attribute selects a
/// block's substitutions relative to its content model's defaults. Simple
/// content (a paragraph, or a styled block in paragraph form), verse and STEM
/// select the substitutions of their deferred source; listing, literal and
/// passthrough blocks convert theirs into `inlines`. A selection equal to
/// the defaults changes nothing. With callouts, the source marks register in
/// the catalog; without any, the callouts stage is dropped.
fn commit_subs(
    block: &mut Block,
    simple: bool,
    lines: &mut Reader<'_>,
    attrs: &Attributes,
    context: &mut ParseContext,
) {
    use crate::pass_subs::Defaults;
    let (defaults, subject) = match &block.kind {
        BlockKind::Paragraph { .. } => (Defaults::Normal, "paragraph"),
        BlockKind::Admonition { .. } if simple => (Defaults::Normal, "admonition"),
        BlockKind::Example { .. } if simple => (Defaults::Normal, "example"),
        BlockKind::Sidebar { .. } if simple => (Defaults::Normal, "sidebar"),
        BlockKind::Quote { .. } if simple => (Defaults::Normal, "quote"),
        BlockKind::Open { .. } if simple => (Defaults::Normal, "open"),
        BlockKind::Verse { .. } => (Defaults::Normal, "verse"),
        BlockKind::Listing { .. } => (Defaults::Verbatim, "listing"),
        BlockKind::Literal { .. } => (Defaults::Verbatim, "literal"),
        BlockKind::Passthrough { .. } => (Defaults::None, "pass"),
        BlockKind::Stem { .. } => (Defaults::Basic, "stem"),
        _ => return,
    };
    let mut stages = match block.attrs.named.get("subs") {
        Some(custom) => crate::pass_subs::resolve_block(
            custom,
            defaults,
            subject,
            block.span,
            &mut context.diagnostics,
        ),
        None => defaults.stages(),
    };
    // Fenced bodies are verbatim for highlighting already; verbatim text in
    // paragraph form keeps only the inline tokens of its stages.
    let fenced = context
        .verbatim_bodies
        .get(&block.span.start)
        .is_some_and(|&(start, ..)| start != block.span.start);
    if !fenced {
        let kind = match block.kind {
            BlockKind::Listing { .. } => Some(TokenKind::ListingText),
            BlockKind::Literal { .. } => Some(TokenKind::LiteralText),
            BlockKind::Passthrough { .. } => Some(TokenKind::PassthroughText),
            _ => None,
        };
        if let Some(kind) = kind {
            lines.record_text(block.span.start, block.span.end, kind);
        }
    }
    if stages == defaults.stages() {
        return;
    }
    // The content's reader source and its comment-filtered text, if any.
    let (span, pass) = (
        block.span,
        matches!(block.kind, BlockKind::Passthrough { .. }),
    );
    let source = |lines: &Reader<'_>, context: &ParseContext| match context
        .verbatim_bodies
        .get(&span.start)
    {
        Some(&(start, end, indentation)) => (indentation.apply(&lines.content(start, end)), None),
        _ => {
            let content = lines.content(span.start, span.end);
            // A passthrough paragraph skips line comments, like other text.
            let filtered = pass.then(|| crate::comments::filtered_text(&content.text, 0));
            (content, filtered)
        }
    };
    use crate::pass_subs::Stage;
    let line_comment = block
        .attrs
        .named
        .get("line-comment")
        .cloned()
        .or_else(|| context.attributes.get("line-comment").map(str::to_owned));
    let mut callout_ids = Vec::new();
    let mut marks = Vec::new();
    if stages.contains(&Stage::Callouts) {
        let (content, _) = source(lines, context);
        let (found, _, ranges) = crate::callouts::catalog(&content.text, &mut context.callouts);
        if found {
            // The marks `sub_callouts` converts (with any `line-comment`)
            // read the registered ids in order (`read_next_id`).
            let converted: usize = crate::scanner::lines_with_offsets(&content.text)
                .into_iter()
                .map(|(_, line)| {
                    crate::callouts::scan_line(line, line_comment.as_deref())
                        .iter()
                        .filter(|mark| !mark.escaped)
                        .count()
                })
                .sum();
            callout_ids = (0..converted)
                .map_while(|_| context.callouts.read_next())
                .collect();
            // Only marks protected by special characters first are converted.
            let position = |stage| stages.iter().position(|&s| s == stage);
            if matches!(
                (position(Stage::Specials), position(Stage::Callouts)),
                (Some(specials), Some(callouts)) if specials < callouts
            ) {
                marks = ranges
                    .into_iter()
                    .map(|r| content.original_span(Span::new(r.start, r.end)))
                    .collect();
            }
        } else {
            stages.retain(|&stage| stage != Stage::Callouts);
        }
    }
    let subs = crate::body_subs::BlockSubs {
        stages,
        line_comment,
        callout_ids,
        trim: !matches!(defaults, Defaults::Normal)
            || matches!(block.kind, BlockKind::Verse { .. }),
        // Ruby's `commit_subs`: a converter's syntax highlighter replaces
        // `specialcharacters` of a `source` listing (it decides whether it
        // highlights).
        source_listing: (matches!(block.kind, BlockKind::Listing { .. })
            && block.attrs.style.as_deref() == Some("source"))
        .then(|| (block.attrs.named.clone(), block.attrs.options.clone())),
    };
    lines.callout_marks.extend(marks);
    match &mut block.kind {
        BlockKind::Paragraph { inlines } | BlockKind::Verse { inlines, .. } => {
            context.body_subs.set_block_subs(inlines, subs);
        }
        BlockKind::Stem { inlines, .. } => context.body_subs.set_block_subs(inlines, subs),
        BlockKind::Admonition { blocks, .. }
        | BlockKind::Example { blocks }
        | BlockKind::Sidebar { blocks }
        | BlockKind::Quote { blocks, .. }
        | BlockKind::Open { blocks } => {
            if let [Block {
                kind: BlockKind::Paragraph { inlines },
                ..
            }] = blocks.as_slice()
            {
                context.body_subs.set_block_subs(inlines, subs);
            }
        }
        BlockKind::Listing { inlines, .. }
        | BlockKind::Literal { inlines, .. }
        | BlockKind::Passthrough { inlines, .. } => {
            let (content, filtered) = source(lines, context);
            *inlines = Some(
                context
                    .body_subs
                    .defer_block(content, filtered, attrs, subs),
            );
        }
        _ => {}
    }
}

fn apply_callouts(block: &mut Block, lines: &Reader<'_>, context: &mut ParseContext) {
    let line_comment = block
        .attrs
        .named
        .get("line-comment")
        .cloned()
        .or_else(|| context.attributes.get("line-comment").map(str::to_owned));
    let span = block.span;
    let (BlockKind::Listing {
        text,
        callouts,
        inlines: None,
        ..
    }
    | BlockKind::Literal {
        text,
        callouts,
        inlines: None,
    }) = &mut block.kind
    else {
        return;
    };
    if !text.contains('<') {
        return;
    }
    // Ruby's `catalog_callouts` registers every mark (`CalloutScanRx`, with
    // no `line-comment`) while parsing; the converted marks then read those
    // ids in order.
    crate::callouts::catalog(text, &mut context.callouts);
    // The block's source lines, to locate each mark.
    let first = lines
        .loaded
        .partition_point(|(off, line)| off + line.len() < span.start);
    let source: Vec<(usize, Arc<str>)> = lines.loaded[first..]
        .iter()
        .take_while(|(off, _)| *off < span.end)
        .cloned()
        .collect();
    let mut source_at = 0;
    let mut autonum = 0u64;
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let marks = crate::callouts::scan_line(line, line_comment.as_deref());
        // The source line ending with this line's marks (the adjusted
        // indentation and expanded tabs precede them): the end of the line.
        let tail = &line[marks.first().map_or(line.len(), |mark| mark.start)..];
        let origin = (!marks.is_empty())
            .then(|| {
                source[source_at..]
                    .iter()
                    .position(|(_, s)| s.ends_with(tail))
                    .map(|k| {
                        source_at += k + 1;
                        let (off, s) = &source[source_at - 1];
                        off + s.len()
                    })
            })
            .flatten();
        let mut copied = 0;
        for mark in marks {
            if mark.escaped {
                let slash = mark.start + mark.guard.as_ref().map_or(0, String::len);
                out.push_str(&line[copied..slash]);
                copied = slash + 1;
                continue;
            }
            out.push_str(&line[copied..mark.start]);
            let start = out.len();
            out.push_str(&line[mark.start..mark.end]);
            copied = mark.end;
            let number = if mark.number == "." {
                autonum += 1;
                autonum.to_string()
            } else {
                mark.number.clone()
            };
            let id = context.callouts.read_next().unwrap_or_default();
            let span = origin.map_or(Span::new(span.start, span.start), |end| {
                Span::new(end + mark.start - line.len(), end + mark.end - line.len())
            });
            callouts.push(crate::ast::Callout {
                start,
                end: out.len(),
                number,
                guard: mark.guard,
                xml: mark.xml,
                id,
                span,
                icons: None,
                src: String::new(),
            });
        }
        out.push_str(&line[copied..]);
    }
    *text = out;
}

/// Ruby's nested `Document` of an AsciiDoc table cell, read when the cell is
/// created: its own header and body with the parent's attributes locked,
/// sharing counters and catalogs. Its title is not shown (Ruby's `notitle`).
fn read_nested_document(
    lines: &mut Reader<'_>,
    text: &[(usize, char)],
    key: (usize, usize),
    context: &mut ParseContext,
) -> (Vec<Block>, bool, Arc<Attributes>) {
    // Ruby preprocesses the first line again (`a|include::…[]`).
    let cell = lines
        .preprocess_cell(text, context)
        .unwrap_or_else(|| Reader::cell_lines(text));
    let mut nested = Reader::nested(cell, lines.include_catalog.clone());
    let outer = context.enter_nested();
    let mut header = collect_header(&mut nested, context);
    // The root tokenizer scans only the document's own header lines.
    for index in [header.title_line, header.title_underline]
        .into_iter()
        .flatten()
    {
        let (off, line) = nested.line(index);
        nested.heading_lines.push(Span::new(off, off + line.len()));
    }
    // Like the document's, the nested document's title converts first
    // (Ruby's `convert_embedded`); it never takes an automatic id.
    let title = header.effective_title.take().map(|effective| {
        let inlines = context.body_subs.defer_title(
            &effective.text,
            effective.span.start,
            effective.replacement.then_some(effective.span),
            &context.attributes,
        );
        let mut title = Block::new(
            header
                .metadata
                .as_ref()
                .map_or(effective.span, |metadata| metadata.title_span),
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
        context
            .nested_titles
            .insert((title.span.start, title.span.end));
        title
    });
    context.start_body();
    let snapshot = (*context.render_attributes()).clone();
    let mut i = header.body_start;
    let outer_first_line = context.first_body_line.take();
    let blocks = collect_blocks(&mut nested, &mut i, context, None, header.pending);
    context.close_sections(0, usize::MAX);
    let has_title = title.is_some();
    let top_level = TopLevel {
        has_header: has_title,
        invalid_header: header.invalid,
        fragment: context.attributes.get("fragment").is_some(),
        first_line: std::mem::replace(&mut context.first_body_line, outer_first_line),
    };
    let blocks = nest_sections(blocks, top_level, context.book, None, context, &nested);
    let header = Arc::new(snapshot.clone());
    context.exit_nested(outer, key, snapshot);
    lines.absorb(nested, true);
    (title.into_iter().chain(blocks).collect(), has_title, header)
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
    /// A block of a list item (Ruby's `next_block` with `:list_type`):
    /// `text_only` for its first block, `adjacent` when no blank line
    /// precedes it (a paragraph then also ends at a list item).
    InListItem {
        list_type: crate::lists::ListType,
        text_only: bool,
        adjacent: bool,
    },
}

/// Parse exactly one block starting at `lines[*i]` (which the caller has ensured
/// is non-blank), advancing `*i` past every line it consumes. Returns `None`
/// only defensively, if a blank line slipped through. Shared by the top-level
/// loop and by list-item `+` continuations, so attached blocks reuse the very
/// same block grammar.
fn parse_block_step<'s, 'c>(
    env: &mut Env<'_, 's, 'c>,
    ctx: BlockCtx,
    pending_attrs: Option<&BlockAttrs>,
    has_id: bool,
) -> Step<'s, 'c> {
    let mut deferred = None;
    let block = parse_block_content(
        env.lines,
        env.i,
        env.context,
        ctx,
        pending_attrs,
        has_id,
        &mut deferred,
    );
    let step = deferred.unwrap_or(Step::Done(block));
    step.then(env, |env, block| {
        let mut block = block?;
        block.attribute_context = Arc::clone(&env.context.attributes);
        Some(block)
    })
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

#[allow(clippy::too_many_arguments)]
fn parse_block_content<'s, 'c>(
    lines: &mut Reader<'s>,
    i: &mut usize,
    context: &mut ParseContext<'c>,
    ctx: BlockCtx,
    // The *pending* block style (from a preceding `[verse]` / `[quote]` attribute
    // line). A `____` fence needs it to choose its content model: `verse` is
    // verbatim (like `....`), every other style — including the bare default — is
    // a parsed `quote` container (like `====`).
    pending_attrs: Option<&BlockAttrs>,
    // Whether metadata already supplied an id (a section-title anchor then stays text).
    has_id: bool,
    // A step suspended on a nested run (a container body, a list item): the
    // caller's driver resumes it; the function then returns `None`.
    deferred: &mut Option<Step<'s, 'c>>,
) -> Option<Block> {
    let pending_style = pending_attrs.and_then(|attrs| attrs.style.as_deref());
    let attrs = inline_attributes(&context.render_attributes(), pending_attrs);
    let (off, line_buffer) = lines.line(*i);
    let line = line_buffer.as_ref();
    let trimmed = line.ruby_rstrip();

    if trimmed.is_ruby_blank() {
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
    // Ruby marks the block after `is_next_line_section?` peeked two
    // preprocessed lines (a comment style peeks them raw, a discrete one
    // not at all), so a line dropped between them moves the block's cursor.
    if ctx == BlockCtx::Section && !discrete && pending_style != Some("comment") {
        lines.peeked_blocks.insert(off);
    }
    // Ruby peeks two lines for a title (`is_next_line_section?`, or
    // `is_section_title?` with `peek_line` for a discrete heading), so the
    // line after an ATX title is preprocessed before the title is read.
    if sections && pending_style != Some("comment") && parse_heading(trimmed).is_some() {
        lines.has(*i + 1, context);
    }
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
            // Ruby peeks a comment-styled block's next line without
            // preprocessing it (`peek_lines` direct): unless it underlines a
            // title, a later read still preprocesses it.
            if pending_style == Some("comment") {
                let underline = lines.peek_raw(*i + 1)?;
                setext_level(line, &underline)?;
                lines.raw_has(*i + 1);
            } else if !lines.has(*i + 1, context) {
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
            context.diagnostics.push(Diagnostic {
                omitted_span: None,
                report: Default::default(),
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
            context.read_section((off, end), pending_style, level, title)
        };
        let heading_attrs = inline_attributes(&context.attributes, pending_attrs);
        // Ruby converts the title while reading only to generate an id
        // (`sectids`) or, with an explicit id, when it holds an attribute
        // reference; otherwise its first use during conversion does.
        let explicit_id = has_id || anchor.is_some();
        let inlines = if explicit_id && title.contains('{')
            || !explicit_id && context.attributes.get("sectids").is_some()
        {
            context.reading_title(title, base, &heading_attrs, &lines.include_catalog)
        } else {
            context
                .body_subs
                .defer_title(title, base, None, &heading_attrs)
        };
        if !has_id && anchor.is_none() {
            context.generating_id();
        }
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
    // A list item's text-only first block is a paragraph, not a macro.
    let text_only = matches!(
        ctx,
        BlockCtx::InListItem {
            text_only: true,
            ..
        }
    );
    let verbatim = verbatim || text_only;
    if let Some(kind) = layout_break(line).filter(|_| !verbatim) {
        let span = Span::new(off, off + line.len());
        let token = match kind {
            BlockKind::PageBreak => TokenKind::PageBreak,
            _ => TokenKind::ThematicBreak,
        };
        lines.record_fences(span, None, token);
        *i += 1;
        return Some(Block::new(span, kind));
    }
    if let Some(found) = crate::image::match_macro(trimmed).filter(|_| !verbatim) {
        lines.record_fences(
            Span::new(off, off + trimmed.len()),
            None,
            TokenKind::BlockMacro,
        );
        *i += 1;
        return read_media_macro(trimmed, off, found, context, &lines.include_catalog);
    }
    if let Some(meta) = parse_toc_macro(trimmed, off).filter(|_| !verbatim) {
        lines.record_fences(
            Span::new(off, off + trimmed.len()),
            None,
            TokenKind::BlockMacro,
        );
        context.block_macro = Some(meta);
        *i += 1;
        return Some(Block::new(
            Span::new(off, off + trimmed.len()),
            BlockKind::Toc,
        ));
    }

    // Ruby's `is_delimited_block?` on the unindented line: a fence longer than
    // the minimum closes only with the identical line, so blocks of one kind
    // nest with fences of different lengths.
    let delimiter = crate::lists::delimiter(line);
    let fence = |tip: &str| {
        delimiter
            .as_ref()
            .filter(|d| d.tip == tip)
            .map(|d| d.terminator.clone())
    };

    // A table (`|===`, `,===`, `:===`, `!===`) whatever the style, like Ruby's
    // delimited blocks without masquerades.
    if let Some(hint) = crate::table::delimiter(trimmed) {
        *i += 1;
        return Some(read_table(
            lines,
            i,
            Span::new(off, off + trimmed.len()),
            hint,
            pending_attrs,
            context,
        ));
    }

    // Markdown fenced code (Ruby's `:fenced_code`): a source listing whatever
    // the style, closed by a line of exactly three backticks. The opening
    // line's rest gives the language and `linenums`.
    if let Some(fence) = fence("```") {
        let opening = Span::new(off, off + line.len());
        *i += 1;
        let (body_start, body_end, end, closing, empty) =
            capture_verbatim_body(lines, i, opening, &fence, "listing", context);
        context
            .verbatim_bodies
            .insert(off, (body_start, body_end, Indentation::default()));
        lines.record_verbatim(
            opening,
            closing,
            (body_start, body_end),
            TokenKind::ListingDelimiter,
            TokenKind::ListingText,
        );
        context.block_macro = Some(MacroMeta {
            style: Some("source".to_owned()),
            named: fenced_code_attributes(line),
            ..MacroMeta::default()
        });
        let text = lines.content(body_start, body_end).text;
        return Some(Block::new(
            Span::new(off, end),
            BlockKind::Listing {
                text,
                callouts: Vec::new(),
                inlines: None,
                blank_edges: BlankEdges {
                    empty,
                    ..BlankEdges::default()
                },
            },
        ));
    }

    // Listing (`----`) and literal (`....`) blocks: verbatim bodies up to the
    // identical closing fence. Like Ruby's `DELIMITED_BLOCKS` masquerades, a
    // `[literal]` style turns a listing fence into a literal block and a
    // `[listing]`/`[source]` style turns a literal fence into a listing.
    let verbatim_fence = fence("----")
        .map(|fence| (fence, pending_style == Some("literal")))
        .or_else(|| {
            fence("....").map(|fence| (fence, !matches!(pending_style, Some("listing" | "source"))))
        });
    if let Some((fence, literal)) = verbatim_fence {
        let start = off;
        let mut end = off + line.len();
        *i += 1;
        let mut text = String::new();
        let mut closed = false;
        let mut body: Option<(usize, usize)> = None;
        while lines.has(*i, context) {
            let (loff, lline_buffer) = lines.line(*i);
            let lline = lline_buffer.as_ref();
            if lline == fence {
                end = loff + lline.len();
                *i += 1;
                closed = true;
                break;
            }
            // Every line after the first starts a new one, blank lines too:
            // leading blank lines are trimmed only after substitution.
            if body.is_some() {
                text.push('\n');
            }
            text.push_str(lline);
            end = loff + lline.len();
            body = Some((body.map_or(loff, |(s, _)| s), end));
            *i += 1;
        }
        let (body_start, body_end) = body.unwrap_or((off + line.len(), off + line.len()));
        context
            .verbatim_bodies
            .insert(start, (body_start, body_end, Indentation::default()));
        let (delimiter, body_kind, name) = if literal {
            (
                TokenKind::LiteralDelimiter,
                TokenKind::LiteralText,
                "literal",
            )
        } else {
            (
                TokenKind::ListingDelimiter,
                TokenKind::ListingText,
                "listing",
            )
        };
        lines.record_verbatim(
            Span::new(start, off + line.len()),
            closed.then(|| Span::new(end - fence.len(), end)),
            (body_start, body_end),
            delimiter,
            body_kind,
        );
        if !closed {
            context.diagnostics.push(Diagnostic {
                omitted_span: None,
                report: crate::DiagnosticReport::unterminated(name),
                span: Span::new(start, start + fence.len()),
                severity: Severity::Error,
                message: format!("unterminated {name} block (missing closing `{fence}`)"),
            });
        }
        let callouts = Vec::new();
        let kind = if literal {
            BlockKind::Literal {
                text,
                callouts,
                inlines: None,
            }
        } else {
            BlockKind::Listing {
                text,
                callouts,
                inlines: None,
                blank_edges: BlankEdges {
                    empty: body.is_none(),
                    ..BlankEdges::default()
                },
            }
        };
        return Some(Block::new(Span::new(start, end), kind));
    }

    // Passthrough block (verbatim, fenced by `++++`). Like `....`/`----` the body
    // is captured raw with no inline substitutions; what sets it apart is the
    // backend, which emits the text *completely raw* (no escaping, no wrapper).
    // The whole trimmed line must be exactly `++++`; inline passthrough
    // delimiters are handled only within parsed inline content.
    if let Some(fence) = fence("++++") {
        let start = off;
        *i += 1;
        let opening = Span::new(start, off + line.len());
        let (body_start, body_end, end, closing, _) = capture_verbatim_body(
            lines,
            i,
            opening,
            &fence,
            unterminated_context("pass", pending_style),
            context,
        );
        context
            .verbatim_bodies
            .insert(start, (body_start, body_end, Indentation::default()));
        let kind = if matches!(pending_style, Some("stem" | "asciimath" | "latexmath")) {
            // Generic STEM is selected from the original positional list on
            // block completion, where absent and quoted-empty fields differ.
            let style = crate::StemStyle::from_alias(pending_style);
            lines.record_verbatim(
                opening,
                closing,
                (body_start, body_end),
                TokenKind::StemDelimiter,
                TokenKind::StemText,
            );
            let source = lines.content(body_start, body_end);
            // A `subs` attribute is committed on completion.
            let inlines = context.body_subs.defer_block(
                source,
                None,
                &attrs,
                crate::body_subs::BlockSubs::new(crate::pass_subs::Defaults::Basic),
            );
            BlockKind::Stem { style, inlines }
        } else {
            lines.record_verbatim(
                opening,
                closing,
                (body_start, body_end),
                TokenKind::PassthroughDelimiter,
                TokenKind::PassthroughText,
            );
            BlockKind::Passthrough {
                text: lines.content(body_start, body_end).text,
                inlines: None,
            }
        };
        return Some(Block::new(Span::new(start, end), kind));
    }

    // Example block (a parsed container, fenced by `====`). Unlike the verbatim
    // `----`/`....` blocks above, the body is parsed *as blocks* via the shared
    // `collect_blocks` (so nested blocks, inline subs and anchors all apply).
    // Only the identical fence closes it, so a longer or shorter `====` nests.
    if let Some(fence) = fence("====") {
        *deferred = Some(read_container_step(
            lines,
            i,
            fence,
            unterminated_context("example", pending_style),
            TokenKind::ExampleDelimiter,
            move |blocks, end| Block::new(Span::new(off, end), BlockKind::Example { blocks }),
        ));
        return None;
    }

    // Quote / verse block, fenced by `____`. asciidoctor's default style for a
    // `____` block is `quote`, whose body is parsed *as blocks* (like `====`); a
    // `[verse]` style instead makes it verbatim (like `....`) while still applying
    // inline substitutions. The style arrives via `pending_style` (the `[verse]`
    // attribute line precedes the fence). The positional attribution
    // (`[quote, author, work]`) is folded in afterwards by `apply_style_fixup`.
    if let Some(fence) = fence("____") {
        let start = off;
        // Verse: verbatim body. Capture the exact source slice (so internal line
        // breaks survive) and inline-scan it — verse keeps newlines but still
        // formats `*bold*`, unlike the no-subs `Literal`/`Listing`.
        if pending_style == Some("verse") {
            *i += 1;
            let opening = Span::new(start, off + line.len());
            let (body_start, body_end, end, closing, _) =
                capture_verbatim_body(lines, i, opening, &fence, "verse", context);
            lines.record_fences(opening, closing, TokenKind::QuoteDelimiter);
            // Verse keeps its line breaks — scan without folding newlines.
            let indentation = verbatim_indentation(pending_attrs, context);
            let inlines = lines.scan_verse(body_start, body_end, &attrs, indentation, context);
            return Some(Block::new(
                Span::new(start, end),
                BlockKind::Verse {
                    attribution: None,
                    inlines,
                },
            ));
        }
        // Quote (default): a parsed container, mirror of the `====` example block.
        *deferred = Some(read_container_step(
            lines,
            i,
            fence,
            "quote",
            TokenKind::QuoteDelimiter,
            move |blocks, end| {
                Block::new(
                    Span::new(start, end),
                    BlockKind::Quote {
                        attribution: None,
                        blocks,
                    },
                )
            },
        ));
        return None;
    }

    // Sidebar block (a parsed container, fenced by `****`). A mirror of the `====`
    // example above: the body is parsed *as blocks* via `collect_blocks`. The check
    // requires the *whole* trimmed line to be `****`, so it precedes (and is
    // distinct from) the `**` unordered-list marker runs — a `**** item` line keeps
    // its content and stays a 4-level list item. Only the identical fence closes
    // the block.
    if let Some(fence) = fence("****") {
        *deferred = Some(read_container_step(
            lines,
            i,
            fence,
            "sidebar",
            TokenKind::SidebarDelimiter,
            move |blocks, end| Block::new(Span::new(off, end), BlockKind::Sidebar { blocks }),
        ));
        return None;
    }

    // Open block (a parsed container, fenced by `--`). The *generic* container:
    // the body is parsed *as blocks* via `collect_blocks`, exactly like the
    // `====`/`****` blocks, and a *container* style applied to it masquerades it
    // into that kind later, in `apply_style_fixup`. The check requires the whole
    // trimmed line to be exactly `--` (two hyphens): `----` is already consumed
    // above as a listing fence, `---` falls through to a paragraph, and an inline
    // `--` (em dash) never reaches here — it lives inside paragraph text, scanned
    // after block detection. A same-length `--` inside closes the outer block.
    if let Some(fence) = fence("--") {
        if pending_style == Some("comment") {
            crate::comments::consume_delimited_comment(lines, i, context, &fence);
            return None;
        }
        let start = off;
        // Verbatim masquerades: a `[source]` / `[listing]` / `[literal]` / `[verse]` style
        // switches the open block's content model from "parsed blocks" to
        // *verbatim*, mirroring how `____` above picks Verse over Quote. The body
        // must not be parsed as blocks, so the choice is made here at the fence —
        // not later in `apply_style_fixup`, which only ever sees an already-parsed
        // body. (The `language` for `[source,lang]` and the verse attribution ride
        // on `attrs`, applied to the block after this returns.)
        if matches!(
            pending_style,
            Some("source" | "listing" | "literal" | "verse" | "pass")
        ) {
            *i += 1;
            let opening = Span::new(start, off + line.len());
            let (body_start, body_end, end, closing, empty) = capture_verbatim_body(
                lines,
                i,
                opening,
                &fence,
                unterminated_context("open", pending_style),
                context,
            );
            let text = match pending_style {
                Some("literal") => Some(TokenKind::LiteralText),
                Some("source" | "listing") => Some(TokenKind::ListingText),
                Some("pass") => Some(TokenKind::PassthroughText),
                _ => None,
            };
            match text {
                Some(text) => lines.record_verbatim(
                    opening,
                    closing,
                    (body_start, body_end),
                    TokenKind::OpenDelimiter,
                    text,
                ),
                None => lines.record_fences(opening, closing, TokenKind::OpenDelimiter),
            }
            let body = lines.content(body_start, body_end);
            context
                .verbatim_bodies
                .insert(start, (body_start, body_end, Indentation::default()));
            let kind = match pending_style {
                // Verse keeps its line breaks but still applies inline subs; the
                // positional attribution (`[verse, author, work]`) is folded in
                // afterwards by `apply_style_fixup`, exactly like the `____` form.
                Some("verse") => BlockKind::Verse {
                    attribution: None,
                    inlines: lines.scan_verse(
                        body_start,
                        body_end,
                        &attrs,
                        verbatim_indentation(pending_attrs, context),
                        context,
                    ),
                },
                // Literal / source keep the body raw (no inline subs). For
                // `[source,lang]` the `source` style and `language` attribute ride
                // on `attrs` and drive the HTML backend's highlight rendering.
                Some("literal") => BlockKind::Literal {
                    text: body.text,
                    callouts: Vec::new(),
                    inlines: None,
                },
                Some("source" | "listing") => BlockKind::Listing {
                    text: body.text,
                    callouts: Vec::new(),
                    inlines: None,
                    blank_edges: BlankEdges {
                        empty,
                        ..BlankEdges::default()
                    },
                },
                // `[pass]` on `--`: raw passthrough, emitted unescaped and
                // unwrapped by the backend (no inline subs), like the `++++` fence.
                Some("pass") => BlockKind::Passthrough {
                    text: body.text,
                    inlines: None,
                },
                _ => unreachable!(),
            };
            return Some(Block::new(Span::new(start, end), kind));
        }
        *deferred = Some(read_container_step(
            lines,
            i,
            fence,
            unterminated_context("open", pending_style),
            TokenKind::OpenDelimiter,
            move |blocks, end| Block::new(Span::new(start, end), BlockKind::Open { blocks }),
        ));
        return None;
    }

    // List: a run of items that nests a level deeper whenever a *unique* marker
    // appears. A list only *starts* at column 0 (an indented marker elsewhere is
    // ordinary text); deeper levels may be indented, handled inside `parse_list`,
    // which consumes the whole (possibly nested) list.
    // Lists (Ruby's order: unordered, ordered, description), unless a
    // verbatim style keeps the lines as a paragraph.
    let verbatim_style = matches!(
        pending_style,
        Some("literal" | "listing" | "source" | "verse")
    );
    if !verbatim_style && crate::callouts::list_item(trimmed).is_some() {
        *deferred = Some(CalloutList::step(&mut Env { lines, i, context }));
        return None;
    }
    if !verbatim_style {
        for list_type in [
            crate::lists::ListType::Unordered,
            crate::lists::ListType::Ordered,
            crate::lists::ListType::Description,
        ] {
            if crate::lists::item(trimmed, list_type).is_some() {
                let style = pending_style.or_else(|| {
                    (list_type == crate::lists::ListType::Unordered
                        && ctx == BlockCtx::Section
                        && context.bibliography_section())
                    .then_some("bibliography")
                });
                *deferred = Some(PlainList::step(
                    &mut Env { lines, i, context },
                    list_type,
                    style.map(str::to_owned),
                ));
                return None;
            }
        }
    }

    // Verbatim paragraph styles retain comment-looking lines.
    let verbatim = matches!(
        pending_style,
        Some("literal" | "listing" | "source" | "verse")
    );

    // Paragraph: accumulate contiguous lines until this context's terminator.
    // Only an unstyled paragraph uses Ruby's `read_paragraph_lines`; Ruby
    // resets a style it does not know for a paragraph before reading it.
    let plain = !pending_style.is_some_and(changes_paragraph_type);
    let start = off;
    let mut end = off + line.len();
    let first = *i;
    *i += 1;
    while if pending_style == Some("comment") {
        lines.raw_has(*i)
    } else {
        lines.has(*i, context)
    } {
        let (loff, lline_buffer) = lines.line(*i);
        let lline = lline_buffer.as_ref();
        let t = lline.ruby_rstrip();
        // Ruby's `read_paragraph_lines`: a blank line, a list continuation, a
        // delimited block or a block attribute line ends the paragraph, and in
        // a list item directly after its marker, any list item too. A verbatim
        // paragraph (`[literal]`, `[listing]`, `[source]`, `[verse]`) ends only at a blank
        // line or a list continuation, so it keeps fence-looking lines.
        let ends = (!verbatim && comment_fence(t).is_some())
            || t.is_ruby_blank()
            || t == "+"
            || (!verbatim
                && (crate::lists::delimited(t).is_some() || crate::lists::block_attribute_line(t)))
            || (plain
                && matches!(ctx, BlockCtx::InListItem { adjacent: true, .. })
                && crate::lists::any(t));
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
    // Ruby's literal paragraph: unstyled lines starting with whitespace, with
    // their common indentation removed (`adjust_indentation!`). A list item's
    // text-only first block, or one adjacent to a description, stays a paragraph.
    let dlist_adjacent = matches!(
        ctx,
        BlockCtx::InListItem {
            list_type: crate::lists::ListType::Description,
            adjacent: true,
            ..
        }
    );
    // Ruby resets a style it does not know for a paragraph (outside
    // `PARAGRAPH_STYLES` and the admonitions) before this check.
    let unstyled =
        !pending_style.is_some_and(|style| changes_paragraph_type(style) || style == "normal");
    if line.starts_with([' ', '\t']) && unstyled && !text_only && !dlist_adjacent {
        let text = Indentation::strip()
            .apply_text(&lines.content(start, end).text)
            .lines()
            .collect::<Vec<_>>()
            .join("\n");
        context
            .verbatim_bodies
            .insert(start, (start, end, Indentation::strip()));
        return Some(Block::new(
            Span::new(start, end),
            BlockKind::Literal {
                text,
                callouts: Vec::new(),
                inlines: None,
            },
        ));
    }
    // An admonition label prefix (`NOTE: `, …) on the first line turns the whole
    // paragraph into an admonition: the prefix is stripped and the rest (incl. any
    // folded lines) is the body. The block span stays the whole paragraph. The
    // `[NOTE]` *style* form is handled later, in the pending-attrs fixup. Like
    // Ruby, only a paragraph that stays a paragraph checks the prefix: a style
    // that changes the block type (`[sidebar]`, `[TIP]`, …) keeps it as text.
    if let Some((kind, prefix_len)) = AdmonitionKind::from_label_prefix(line)
        .filter(|_| !text_only && !pending_style.is_some_and(changes_paragraph_type))
    {
        lines.record_fences(
            Span::new(start, start + prefix_len - 1),
            None,
            TokenKind::AdmonitionLabel,
        );
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
        let mut block = Block::new(
            Span::new(start, end),
            BlockKind::Admonition {
                kind,
                label: context.admonition_label(kind, pending_attrs),
                blocks: vec![child],
                icons: None,
                src: String::new(),
            },
        );
        block.simple = true;
        return Some(block);
    }
    // Ruby's ordinary paragraph branch, after the admonition label: a
    // Markdown blockquote or a quoted paragraph with a credit line. A style
    // Ruby does not know as a paragraph style is ignored there.
    if !text_only && !pending_style.is_some_and(changes_paragraph_type) {
        let quoted = quote_paragraph(
            lines,
            first..*i,
            Span::new(start, end),
            &attrs,
            context,
            deferred,
        );
        if quoted.is_some() || deferred.is_some() {
            return quoted;
        }
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

/// Ruby's Markdown blockquote and quoted paragraph (`next_block`): an
/// unstyled paragraph whose first line starts with `> `, or with `"` when its
/// last line is a `-- ` credit and the line before ends with `"`. The credit
/// gets normal substitutions while reading and later splits at the first
/// `, ` into the attribution and citetitle. A blockquote parses its unmarked lines as
/// blocks (compound); a quoted paragraph is simple content. Both catalog the
/// inline anchors of their lines, like an ordinary paragraph.
/// A blockquote reads its body as a nested run: it suspends in `deferred`.
fn quote_paragraph<'s, 'c>(
    lines: &mut Reader<'s>,
    range: std::ops::Range<usize>,
    span: Span,
    attrs: &Arc<Attributes>,
    context: &mut ParseContext<'c>,
    deferred: &mut Option<Step<'s, 'c>>,
) -> Option<Block> {
    // Ruby's `read_paragraph_lines` skips line comments.
    let mut comments = Vec::new();
    let mut text = Vec::new();
    for k in range {
        let (off, line) = lines.line(k);
        if crate::comments::is_line_comment(&line) {
            comments.push(Span::new(off, off + line.len()));
        } else {
            text.push((off, line, lines.eol(k)));
        }
    }
    let head = Arc::clone(&text.first()?.1);
    let block = if head.starts_with("> ") {
        let mut body: Vec<(usize, Arc<str>, String)> = Vec::new();
        for (off, line, eol) in text {
            let cut = if &*line == ">" {
                1
            } else if line.starts_with("> ") {
                2
            } else {
                0
            };
            if cut > 0 {
                lines.quote_markers.push(Span::new(off, off + 1));
            }
            body.push((off + cut, Arc::from(&line[cut..]), eol));
        }
        let credit = body
            .last()
            .filter(|(_, line, _)| line.starts_with("-- "))
            .map(|(off, line, _)| (off + 3, line[3..].to_owned()));
        if credit.is_some() {
            body.pop();
            while body.last().is_some_and(|(_, line, _)| line.is_empty()) {
                body.pop();
            }
        }
        context.comments.extend(comments);
        let range = body
            .first()
            .zip(body.last())
            .map(|(first, last)| (first.0, last.0 + last.1.len()));
        let nested = Reader::nested(body, lines.include_catalog.clone());
        *deferred = Some(Step::Collect(
            Box::new(Collect {
                stop: None,
                pending: PendingMeta::default(),
                scope: Scope::Container,
                reader: Some(nested),
                unterminated: None,
            }),
            Box::new(move |env, blocks, nested| {
                let nested = nested.expect("the blockquote's reader");
                let attribution = credit
                    .and_then(|(off, line)| credit_attribution(&line, off, env.lines, env.context));
                if let Some((start, end)) = range {
                    nested.catalog_anchors(start, end, false, env.context);
                }
                env.lines.absorb(nested, false);
                let block = Block::new(
                    span,
                    BlockKind::Quote {
                        attribution,
                        blocks,
                    },
                );
                Step::Done(Some(quoted_block(block, env.context)))
            }),
            Vec::new(),
        ));
        return None;
    } else if head.starts_with('"') {
        let [.., (body_off, body, _), (credit_off, credit, _)] = text.as_slice() else {
            return None;
        };
        if !credit.starts_with("-- ") || !body.ends_with('"') {
            return None;
        }
        let start = text[0].0 + 1;
        let end = (body_off + body.len() - 1).max(start);
        context
            .comments
            .extend(comments.into_iter().filter(|comment| comment.start >= end));
        let inlines = lines.scan_content(start, end, attrs, context);
        let mut child = Block::new(Span::new(start, end), BlockKind::Paragraph { inlines });
        child.attribute_context = Arc::clone(attrs);
        let attribution = credit_attribution(&credit[3..], credit_off + 3, lines, context);
        lines.catalog_anchors(start, end, false, context);
        let mut block = Block::new(
            span,
            BlockKind::Quote {
                attribution,
                blocks: vec![child],
            },
        );
        block.simple = true;
        block
    } else {
        return None;
    };
    Some(quoted_block(block, context))
}

/// A quoted block sets its own `quote` style; a credit is split later.
fn quoted_block(block: Block, context: &mut ParseContext) -> Block {
    if matches!(
        block.kind,
        BlockKind::Quote {
            attribution: Some(_),
            ..
        }
    ) {
        context.credits.insert((block.span.start, block.span.end));
    }
    context.block_macro = Some(MacroMeta {
        style: Some("quote".to_owned()),
        ..MacroMeta::default()
    });
    block
}

/// A blockquote credit line: normal substitutions while reading (an empty
/// result has no attribution). The registry pass splits it into the
/// attribution and citetitle once its reference labels are known
/// (blockquote_credit.rs).
fn credit_attribution(
    credit: &str,
    base: usize,
    lines: &Reader<'_>,
    context: &mut ParseContext,
) -> Option<Attribution> {
    let attributes = Arc::clone(&context.attributes);
    let inlines = context.reading_title(credit, base, &attributes, &lines.include_catalog);
    if inlines.iter().all(empty_inline) {
        return None;
    }
    Some(Attribution {
        author: Some(inlines),
        citetitle: None,
        trailing: Vec::new(),
    })
}

/// Whether an inline converts to nothing.
fn empty_inline(inline: &Inline) -> bool {
    match inline {
        Inline::Text { value, .. } => value.is_empty(),
        Inline::AttributeRef { value, parts, .. } => {
            parts.is_empty() && value.as_deref() == Some("")
        }
        _ => false,
    }
}

/// A line of a list item, as Ruby's `read_lines_for_list_item` buffers it: a
/// continuation marker is Ruby's `ListContinuationPlaceholder` (an empty line)
/// or `ListContinuationString` (`+`).
#[derive(Clone)]
struct ItemLine {
    off: usize,
    text: Arc<str>,
    eol: String,
    marker: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Continuation {
    Inactive,
    Active,
    Frozen,
}

fn read_item_line(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
) -> Option<ItemLine> {
    if !lines.has(*i, context) {
        return None;
    }
    let (off, text) = lines.line(*i);
    let eol = lines.eol(*i);
    *i += 1;
    Some(ItemLine {
        marker: text.is_empty() && lines.placeholders.contains(&off),
        off,
        text,
        eol,
    })
}

/// Ruby's `NESTABLE_LIST_CONTEXTS` (only description lists once nested):
/// the list type a line starts and whether it is a term without text.
fn nested_list(line: &str, within_nested: bool) -> Option<(crate::lists::ListType, bool)> {
    use crate::lists::{description, item, ListType};
    if !within_nested {
        for list_type in [ListType::Unordered, ListType::Ordered] {
            if item(line, list_type).is_some() {
                return Some((list_type, false));
            }
        }
    }
    description(line).map(|m| (ListType::Description, m.text.is_none()))
}

/// Ruby's `read_lines_until` for a literal paragraph inside a list item: up
/// to a blank line, a later `+` or (for a description list) a sibling.
fn read_literal_lines(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    sibling: Option<&crate::lists::Sibling>,
) -> Vec<ItemLine> {
    let mut out = Vec::new();
    while lines.has(*i, context) {
        let text = lines[*i].1.clone();
        if text.is_empty()
            || (!out.is_empty() && &*text == "+")
            || sibling.is_some_and(|s| crate::lists::is_sibling(&text, s))
        {
            break;
        }
        out.extend(read_item_line(lines, i, context));
    }
    out
}

/// Ruby's `read_lines_for_list_item`: the lines after an item's marker line
/// up to a sibling item or the end of the list, with attached blocks joined by
/// list continuations. Leaves `*i` at the first line not taken.
fn read_list_item_lines(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    list_type: crate::lists::ListType,
    sibling: &crate::lists::Sibling,
    mut has_text: bool,
) -> Vec<ItemLine> {
    use crate::lists::{block_attribute_line, delimited, is_sibling, ListType};
    let dlist = list_type == ListType::Description;
    let mut buffer: Vec<ItemLine> = Vec::new();
    let mut continuation = Continuation::Inactive;
    let mut within_nested = false;
    let mut detached = None;
    let mut unread = false;
    while let Some(mut line) = read_item_line(lines, i, context) {
        if is_sibling(&line.text, sibling) {
            unread = true;
            break;
        }
        if &*line.text == "+" {
            line.marker = true;
        }
        // Ruby keeps the previous line object even when it becomes a placeholder.
        let prev = buffer.last().map(|l| (l.text.clone(), l.marker));
        if prev.as_ref().is_some_and(|(_, marker)| *marker) {
            if continuation == Continuation::Inactive {
                continuation = Continuation::Active;
                has_text = true;
                if !within_nested {
                    buffer.last_mut().unwrap().text = Arc::from("");
                }
            }
            if line.marker {
                if continuation != Continuation::Frozen {
                    continuation = Continuation::Frozen;
                    buffer.push(line);
                }
                continue;
            }
        }
        if let Some(terminator) = delimited(&line.text) {
            if continuation != Continuation::Active {
                unread = true;
                break;
            }
            buffer.push(line);
            while let Some(next) = read_item_line(lines, i, context) {
                let done = *next.text == *terminator;
                buffer.push(next);
                if done {
                    break;
                }
            }
            continuation = Continuation::Inactive;
        } else if dlist
            && continuation != Continuation::Active
            && line.text.starts_with('[')
            && block_attribute_line(&line.text)
        {
            let mut attribute_lines = vec![line];
            let mut interrupt = false;
            while lines.has(*i, context) {
                let next = lines[*i].1.clone();
                if delimited(&next).is_some() {
                    interrupt = true;
                } else if next.is_empty() || (next.starts_with('[') && block_attribute_line(&next))
                {
                    attribute_lines.extend(read_item_line(lines, i, context));
                    continue;
                } else if crate::lists::any(&next) && !is_sibling(&next, sibling) {
                    buffer.append(&mut attribute_lines);
                } else {
                    interrupt = true;
                }
                break;
            }
            if interrupt {
                *i -= attribute_lines.len();
                break;
            }
        } else if continuation == Continuation::Active && !line.text.is_empty() {
            if crate::lists::literal_paragraph(&line.text) {
                *i -= 1;
                buffer.extend(read_literal_lines(
                    lines,
                    i,
                    context,
                    dlist.then_some(sibling),
                ));
                continuation = Continuation::Inactive;
            } else if (line.text.starts_with('.') && parse_block_title(&line.text).is_some())
                || (line.text.starts_with('[') && block_attribute_line(&line.text))
                || (line.text.starts_with(':') && crate::lists::attribute_entry(&line.text))
            {
                buffer.push(line);
            } else {
                if let Some((nested, termless)) = nested_list(&line.text, within_nested) {
                    within_nested = true;
                    if nested == ListType::Description && termless {
                        has_text = false;
                    }
                }
                buffer.push(line);
                continuation = Continuation::Inactive;
            }
        } else if prev.as_ref().is_some_and(|(text, _)| text.is_empty()) {
            if line.text.is_empty() {
                while lines.has(*i, context) && lines[*i].1.is_empty() {
                    *i += 1;
                }
                let Some(next) = read_item_line(lines, i, context) else {
                    break;
                };
                line = next;
                if is_sibling(&line.text, sibling) {
                    unread = true;
                    break;
                }
            }
            if &*line.text == "+" {
                detached = Some(buffer.len());
                line.marker = true;
                buffer.push(line);
            } else if has_text {
                if is_sibling(&line.text, sibling) {
                    unread = true;
                    break;
                } else if let Some((nested, termless)) = nested_list(&line.text, false) {
                    buffer.push(line);
                    within_nested = true;
                    if nested == ListType::Description && termless {
                        has_text = false;
                    }
                } else if crate::lists::literal_paragraph(&line.text) {
                    *i -= 1;
                    buffer.extend(read_literal_lines(
                        lines,
                        i,
                        context,
                        dlist.then_some(sibling),
                    ));
                } else {
                    unread = true;
                    break;
                }
            } else {
                // A description list still needs its text: take it.
                if !within_nested {
                    buffer.pop();
                }
                buffer.push(line);
                has_text = true;
            }
        } else if line.marker {
            has_text = true;
            buffer.push(line);
        } else {
            if !line.text.is_empty() {
                has_text = true;
            }
            if let Some((nested, termless)) = nested_list(&line.text, within_nested) {
                within_nested = true;
                if nested == ListType::Description && termless {
                    has_text = false;
                }
            }
            buffer.push(line);
        }
    }
    if unread {
        *i -= 1;
    }
    if let Some(at) = detached {
        buffer[at].text = Arc::from("");
    }
    while let Some(last) = buffer.last() {
        if last.marker {
            buffer.pop();
            break;
        } else if last.text.is_empty() {
            buffer.pop();
        } else {
            break;
        }
    }
    buffer
}

/// Ruby's `parse_list_item` after the marker line: the item's blocks from its
/// lines, and the range of a paragraph adjacent to the marker line that
/// folds into the item text (Ruby's `fold_first`). An item with lines reads
/// them in its own reader as a nested run ([`ItemBlocks::Read`]).
enum ItemBlocks<'s> {
    Ready(Vec<Block>, Option<Span>),
    Read(Box<Collect<'s>>, bool),
}

fn item_blocks_begin<'s>(
    lines: &Reader<'s>,
    buffer: Vec<ItemLine>,
    list_type: crate::lists::ListType,
    mut has_text: bool,
) -> ItemBlocks<'s> {
    if buffer.is_empty() {
        return ItemBlocks::Ready(Vec::new(), None);
    }
    // Peek past comment lines.
    let adjacent = buffer
        .iter()
        .find(|l| !is_line_comment(&l.text))
        .is_some_and(|l| !l.text.is_empty());
    if adjacent && list_type != crate::lists::ListType::Description {
        has_text = false;
    }
    let placeholders = buffer
        .iter()
        .filter(|l| l.marker && l.text.is_empty())
        .map(|l| l.off)
        .collect();
    let mut item_reader = Reader::nested(
        buffer.into_iter().map(|l| (l.off, l.text, l.eol)).collect(),
        lines.include_catalog.clone(),
    );
    item_reader.placeholders = placeholders;
    ItemBlocks::Read(
        Box::new(Collect {
            stop: None,
            pending: PendingMeta::default(),
            scope: Scope::ListItem(list_type, !has_text),
            reader: Some(item_reader),
            unterminated: None,
        }),
        adjacent,
    )
}

/// The item's blocks once its run is read: absorb its reader and fold an
/// adjacent paragraph into the item text.
fn item_blocks_end(
    lines: &mut Reader<'_>,
    context: &mut ParseContext,
    mut blocks: Vec<Block>,
    item_reader: Option<Reader<'_>>,
    adjacent: bool,
) -> (Vec<Block>, Option<Span>) {
    // Where the item's own lines start: metadata of a folded paragraph lies
    // between there and the paragraph.
    let first_line = item_reader
        .as_ref()
        .and_then(|reader| reader.loaded.first().map(|(off, _)| *off));
    if let Some(item_reader) = item_reader {
        lines.absorb(item_reader, false);
    }
    let folded = if adjacent
        && matches!(
            blocks.first().map(|b| &b.kind),
            Some(BlockKind::Paragraph { .. })
        ) {
        // Ruby removes the paragraph's indentation before folding it.
        let paragraph = blocks.remove(0);
        let span = paragraph.span;
        // Ruby never plays back the attribute entries of the discarded
        // paragraph: they changed reading only.
        if let Some(first_line) = first_line {
            context
                .body_subs
                .entries
                .retain(|(at, _, _)| !(first_line..span.start).contains(at));
        }
        // Ruby registered the paragraph's id before folding it away.
        if let Some(id) = paragraph.id {
            let key = (span.start, span.end);
            let stamp = context.block_stamps.remove(&key).unwrap_or_default();
            let reftext = context.reftexts.remove(&key);
            context.anchors.push((
                id,
                crate::anchor_catalog::Entry {
                    span,
                    reftext,
                    bibliography: false,
                    block: true,
                    stamp,
                },
            ));
        }
        let text = lines.content(span.start, span.end).text;
        let indent = text.len() - text.trim_start_matches([' ', '\t']).len();
        Some(Span::new(span.start + indent, span.end))
    } else {
        None
    };
    (blocks, folded)
}

/// A list being read item by item: [`item_step`] hands it each item's blocks
/// and resumes it after a nested run.
trait ItemList: Sized + 'static {
    /// Take the blocks of the item being read.
    fn add(&mut self, env: &mut Env<'_, '_, '_>, blocks: Vec<Block>, folded: Option<Span>);
    /// Read on: the next items, or the finished list.
    fn next<'s, 'c>(self, env: &mut Env<'_, 's, 'c>) -> Step<'s, 'c>;
}

/// Continue a list once an item's blocks are read. Ready items continue in
/// place (`Ok`), so a long list does not deepen the call stack either; an
/// item with lines suspends the list on its run (`Err`).
#[allow(clippy::result_large_err)] // The `Err` is a brief `Step`, as above.
fn item_step<'s, 'c, L: ItemList>(
    env: &mut Env<'_, 's, 'c>,
    blocks: ItemBlocks<'s>,
    mut list: L,
) -> Result<L, Step<'s, 'c>> {
    match blocks {
        ItemBlocks::Ready(blocks, folded) => {
            list.add(env, blocks, folded);
            Ok(list)
        }
        ItemBlocks::Read(collect, adjacent) => Err(Step::Collect(
            collect,
            Box::new(move |env, blocks, reader| {
                let (blocks, folded) =
                    item_blocks_end(env.lines, env.context, blocks, reader, adjacent);
                list.add(env, blocks, folded);
                list.next(env)
            }),
            Vec::new(),
        )),
    }
}

/// The text of an item: the marker line's text through a folded paragraph,
/// or that paragraph alone.
fn item_text(
    lines: &Reader<'_>,
    text: Option<Span>,
    folded: Option<Span>,
    context: &mut ParseContext,
) -> Vec<Inline> {
    let mut content = match (text, folded) {
        (Some(text), Some(folded)) => lines.folded_content(text, folded),
        (Some(text), None) => lines.content(text.start, text.end),
        (None, Some(folded)) => lines.content(folded.start, folded.end),
        (None, None) => return Vec::new(),
    };
    let attrs = context.render_attributes();
    if let Some(folded) = folded {
        content = fold_indentation(content, folded, lines);
    }
    // Ruby skips line comments while reading the lines, before
    // `adjust_indentation!`: an indented ` // x` stays text.
    let filtered =
        crate::scanner::FilteredSource::without_lines_at(&content.text, 0, |at, line| {
            let original = content.original_span(Span::new(at, at)).start;
            is_line_comment(line) && lines.line_start(original) == original
        });
    lines.scan_prepared(content, filtered, &attrs, context)
}

/// Ruby's `adjust_indentation!` on a folded paragraph's lines: their common
/// indentation goes (the first line's is already outside the folded span),
/// keeping the map to physical bytes.
fn fold_indentation(
    content: crate::scanner::FilteredSource,
    folded: Span,
    lines: &Reader<'_>,
) -> crate::scanner::FilteredSource {
    use crate::scanner::Piece;
    let blank = |c: char| c == ' ' || c == '\t';
    let paragraph = lines.line_start(folded.start);
    let first = folded.start - paragraph;
    let text = content.text.as_str();
    // Lines of the folded paragraph after the item text: (start, leading
    // blanks). Its first line keeps its indentation here when the content
    // starts at the item text; Ruby strips the common indentation from it too.
    let mut continued = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let original = content.original_span(Span::new(at, at)).start;
        let body = line.trim_end_matches(['\n', '\r']);
        if at > 0 && original >= paragraph && !body.trim_matches(blank).is_empty() {
            continued.push((at, body.len() - body.trim_start_matches(blank).len()));
        }
        at += line.len();
    }
    let common = continued
        .iter()
        .map(|(_, n)| *n)
        .chain([first])
        .min()
        .unwrap_or(0);
    if common == 0 || continued.is_empty() {
        return content;
    }
    let mut pieces = Vec::new();
    let mut copied = 0;
    for (start, _) in continued {
        pieces.push(Piece::Copy(copied..start));
        copied = start + common;
    }
    pieces.push(Piece::Copy(copied..text.len()));
    content.remap(&pieces)
}

/// An item read up to its marker line, waiting for its blocks.
struct PendingItem {
    off: usize,
    line_end: usize,
    text: Option<Span>,
    checkbox: Option<bool>,
    marker: String,
}

/// An unordered or ordered list (Ruby's `parse_list`): a run of items that
/// nests a level deeper whenever a *unique* marker appears.
struct PlainList {
    list_type: crate::lists::ListType,
    style: Option<String>,
    start: usize,
    end: usize,
    items: Vec<ListItem>,
    implicit_style: Option<String>,
    checklist: bool,
    current: Option<PendingItem>,
}

impl PlainList {
    fn step<'s, 'c>(
        env: &mut Env<'_, 's, 'c>,
        list_type: crate::lists::ListType,
        style: Option<String>,
    ) -> Step<'s, 'c> {
        let start = env.lines[*env.i].0;
        if list_type == crate::lists::ListType::Description {
            let first = crate::lists::item(&env.lines[*env.i].1, list_type)
                .expect("a description list item");
            let delimiter = first.marker.clone();
            return DescriptionList {
                bibliography: style.as_deref() == Some("bibliography"),
                start,
                sibling: crate::lists::Sibling::Delimiter(delimiter.clone()),
                delimiter,
                first: Some(first),
                items: Vec::new(),
                current: None,
                term: None,
            }
            .next(env);
        }
        PlainList {
            list_type,
            style,
            start,
            end: start,
            items: Vec::new(),
            implicit_style: None,
            checklist: false,
            current: None,
        }
        .next(env)
    }
}

impl ItemList for PlainList {
    fn add(&mut self, env: &mut Env<'_, '_, '_>, blocks: Vec<Block>, folded: Option<Span>) {
        let item = self.current.take().expect("an item being read");
        let inlines = item_text(env.lines, item.text, folded, env.context);
        if self.style.as_deref() == Some("bibliography") {
            env.context.body_subs.bibliography(&inlines);
        }
        let item_end = blocks
            .last()
            .map(|b| b.span.end)
            .or(folded.map(|f| f.end))
            .unwrap_or(item.line_end)
            .max(item.line_end);
        self.items.push(ListItem {
            inlines,
            blocks,
            span: Span::new(item.off, item_end),
            checkbox: item.checkbox,
            marker: Some(item.marker),
        });
        self.end = item_end;
        // Ruby's `reader.skip_blank_lines || break`.
        while env.lines.has(*env.i, env.context) && env.lines[*env.i].1.is_ruby_blank() {
            *env.i += 1;
        }
    }

    fn next<'s, 'c>(mut self, env: &mut Env<'_, 's, 'c>) -> Step<'s, 'c> {
        use crate::lists::{item, resolve_ordered, ListType, Sibling};
        let list_type = self.list_type;
        let ordered = list_type == ListType::Ordered;
        let bibliography = self.style.as_deref() == Some("bibliography");
        while env.lines.has(*env.i, env.context) {
            let Some(m) = item(&env.lines[*env.i].1, list_type) else {
                break;
            };
            let (off, line) = env.lines.line(*env.i);
            let marker_span = Span::new(off + m.marker_range.start, off + m.marker_range.end);
            env.lines.list_markers.push(marker_span);
            let mut text = m
                .text
                .clone()
                .map(|r| Span::new(off + r.start, off + r.end));
            let mut checkbox = None;
            let sibling = if ordered {
                let (marker, style_of) = resolve_ordered(&m.marker);
                if let Some((expected, actual)) =
                    crate::lists::expected_ordinal(&m.marker, self.items.len())
                {
                    if expected != actual {
                        env.context.diagnostics.push(Diagnostic {
                            omitted_span: None,
                            report: Default::default(),
                            span: Span::new(off, off + line.len()),
                            severity: Severity::Warning,
                            message: format!("list item index: expected {expected}, got {actual}"),
                        });
                    }
                }
                if self.items.is_empty() && self.style.is_none() {
                    self.implicit_style = Some(style_of.map(str::to_owned).unwrap_or_else(|| {
                        [
                            "arabic",
                            "loweralpha",
                            "lowerroman",
                            "upperalpha",
                            "upperroman",
                        ]
                        .get(marker.len() - 1)
                        .unwrap_or(&"arabic")
                        .to_string()
                    }));
                }
                Sibling::Marker(list_type, marker)
            } else {
                // Ruby's checklist items.
                if let Some(range) = m.text.as_ref().filter(|_| !bibliography) {
                    let item_text = &line[range.clone()];
                    for (prefix, checked) in [("[ ] ", false), ("[x] ", true), ("[*] ", true)] {
                        if item_text.starts_with(prefix) {
                            self.checklist = true;
                            checkbox = Some(checked);
                            text = Some(Span::new(off + range.start + 4, off + range.end));
                        }
                    }
                }
                Sibling::Marker(list_type, m.marker.clone())
            };
            if let Some(text) = text {
                if bibliography && !ordered {
                    let content = env.lines.content(text.start, text.end);
                    env.context.catalog_bibliography(&content.text, |range| {
                        content.original_span(Span::new(range.start, range.end))
                    });
                } else {
                    env.lines
                        .catalog_anchors(text.start, text.end, true, env.context);
                }
            }
            *env.i += 1;
            let buffer =
                read_list_item_lines(env.lines, env.i, env.context, list_type, &sibling, true);
            self.current = Some(PendingItem {
                off,
                line_end: off + line.len(),
                text,
                checkbox,
                marker: match sibling {
                    Sibling::Marker(_, marker) | Sibling::Delimiter(marker) => marker,
                },
            });
            let blocks = item_blocks_begin(env.lines, buffer, list_type, true);
            match item_step(env, blocks, self) {
                Ok(list) => self = list,
                Err(step) => return step,
            }
        }
        let mut block = Block::new(
            Span::new(self.start, self.end),
            BlockKind::List {
                ordered,
                items: self.items,
                icons: None,
            },
        );
        block.attrs.style = self.style.or(self.implicit_style);
        if self.checklist {
            block.attrs.options.push("checklist".into());
        }
        block.attribute_context = Arc::clone(&env.context.attributes);
        Step::Done(Some(block))
    }
}

/// A term waiting for its description's blocks (Ruby's `parse_list_item`
/// for a description list).
struct PendingTerm {
    off: usize,
    line_end: usize,
    term: Span,
    term_inlines: Vec<Inline>,
    text: Option<Span>,
}

/// A description list: consecutive terms without a description share the
/// next one.
struct DescriptionList {
    bibliography: bool,
    start: usize,
    delimiter: String,
    sibling: crate::lists::Sibling,
    first: Option<crate::lists::ItemMatch>,
    items: Vec<crate::ast::DescriptionListItem>,
    current: Option<crate::ast::DescriptionListItem>,
    term: Option<PendingTerm>,
}

impl ItemList for DescriptionList {
    fn add(&mut self, env: &mut Env<'_, '_, '_>, blocks: Vec<Block>, folded: Option<Span>) {
        let PendingTerm {
            off,
            line_end,
            term,
            term_inlines,
            text,
        } = self.term.take().expect("a term being read");
        let inlines = item_text(env.lines, text, folded, env.context);
        let item_end = blocks
            .last()
            .map(|b| b.span.end)
            .or(folded.map(|f| f.end))
            .unwrap_or(line_end)
            .max(line_end);
        let description = (!inlines.is_empty() || !blocks.is_empty()).then(|| ListItem {
            inlines,
            blocks,
            span: Span::new(text.or(folded).map_or(line_end, |s| s.start), item_end),
            checkbox: None,
            marker: None,
        });
        let next = crate::ast::DescriptionListItem {
            terms: vec![crate::ast::ListTerm {
                inlines: term_inlines,
                span: term,
            }],
            description,
            span: Span::new(off, item_end),
        };
        match &mut self.current {
            None => self.current = Some(next),
            Some(current) if current.description.is_some() => {
                self.items.push(std::mem::replace(current, next));
            }
            Some(current) => {
                current.terms.extend(next.terms);
                current.description = next.description;
                current.span.end = next.span.end;
            }
        }
    }

    fn next<'s, 'c>(mut self, env: &mut Env<'_, 's, 'c>) -> Step<'s, 'c> {
        use crate::lists::ListType;
        loop {
            let m = match self.first.take() {
                Some(first) => first,
                None => {
                    if !env.lines.has(*env.i, env.context) {
                        break;
                    }
                    let Some(next) =
                        crate::lists::description_sibling(&env.lines[*env.i].1, &self.delimiter)
                    else {
                        break;
                    };
                    next
                }
            };
            let (off, line) = env.lines.line(*env.i);
            let term_range = m.term.clone().expect("a description term");
            let term = Span::new(off + term_range.start, off + term_range.end);
            env.lines.list_markers.push(Span::new(
                off + m.marker_range.start,
                off + m.marker_range.end,
            ));
            env.lines
                .catalog_term_anchor(term.start, term.end, env.context);
            let attrs = env.context.render_attributes();
            let term_inlines = env
                .lines
                .scan_content(term.start, term.end, &attrs, env.context);
            let text = m
                .text
                .clone()
                .map(|r| Span::new(off + r.start, off + r.end));
            *env.i += 1;
            let has_text = text.is_some();
            let buffer = read_list_item_lines(
                env.lines,
                env.i,
                env.context,
                ListType::Description,
                &self.sibling,
                has_text,
            );
            self.term = Some(PendingTerm {
                off,
                line_end: off + line.len(),
                term,
                term_inlines,
                text,
            });
            let blocks = item_blocks_begin(env.lines, buffer, ListType::Description, has_text);
            match item_step(env, blocks, self) {
                Ok(list) => self = list,
                Err(step) => return step,
            }
        }
        let mut items = self.items;
        items.extend(self.current);
        if self.bibliography {
            for item in &items {
                for term in &item.terms {
                    env.context.body_subs.bibliography(&term.inlines);
                }
                if let Some(description) = &item.description {
                    env.context.body_subs.bibliography(&description.inlines);
                }
            }
        }
        let end = items.last().map_or(self.start, |item| item.span.end);
        let mut block = Block::new(
            Span::new(self.start, end),
            BlockKind::DescriptionList { items },
        );
        block.attribute_context = Arc::clone(&env.context.attributes);
        Step::Done(Some(block))
    }
}

/// A callout list (`<1> text`), like Ruby's `parse_callout_list`.
struct CalloutList {
    start: usize,
    end: usize,
    items: Vec<crate::ast::CalloutListItem>,
    autonum: u64,
    current: Option<(usize, Span, Option<Span>)>,
}

impl CalloutList {
    fn step<'s, 'c>(env: &mut Env<'_, 's, 'c>) -> Step<'s, 'c> {
        let start = env.lines[*env.i].0;
        CalloutList {
            start,
            end: start,
            items: Vec::new(),
            autonum: 0,
            current: None,
        }
        .next(env)
    }
}

impl ItemList for CalloutList {
    fn add(&mut self, env: &mut Env<'_, '_, '_>, blocks: Vec<Block>, folded: Option<Span>) {
        let (off, line_span, text) = self.current.take().expect("an item being read");
        let inlines = item_text(env.lines, text, folded, env.context);
        let item_end = blocks
            .last()
            .map(|b| b.span.end)
            .or(folded.map(|f| f.end))
            .unwrap_or(line_span.end)
            .max(line_span.end);
        let ordinal = self.items.len() as u64 + 1;
        let coids = env.context.callouts.ids(ordinal);
        if coids.is_empty() {
            env.context.diagnostics.push(Diagnostic {
                omitted_span: None,
                report: Default::default(),
                span: line_span,
                severity: Severity::Warning,
                message: format!("no callout found for <{ordinal}>"),
            });
        }
        self.items.push(crate::ast::CalloutListItem {
            item: ListItem {
                inlines,
                blocks,
                span: Span::new(off, item_end),
                checkbox: None,
                marker: Some("<1>".into()),
            },
            coids,
            icons: None,
            src: String::new(),
        });
        self.end = item_end;
    }

    fn next<'s, 'c>(mut self, env: &mut Env<'_, 's, 'c>) -> Step<'s, 'c> {
        use crate::lists::{ListType, Sibling};
        let sibling = Sibling::Marker(ListType::Callout, "<1>".into());
        while env.lines.has(*env.i, env.context) {
            let Some(m) = crate::callouts::list_item(&env.lines[*env.i].1) else {
                break;
            };
            let (off, line) = env.lines.line(*env.i);
            let line_span = Span::new(off, off + line.len());
            env.lines.list_markers.push(Span::new(
                off + m.marker_range.start,
                off + m.marker_range.end,
            ));
            let number = if m.marker == "." {
                self.autonum += 1;
                self.autonum.to_string()
            } else {
                m.marker.clone()
            };
            let expected = (self.items.len() + 1).to_string();
            if number != expected {
                env.context.diagnostics.push(Diagnostic {
                    omitted_span: None,
                    report: Default::default(),
                    span: line_span,
                    severity: Severity::Warning,
                    message: format!("callout list item index: expected {expected}, got {number}"),
                });
            }
            let text = m
                .text
                .clone()
                .map(|r| Span::new(off + r.start, off + r.end));
            if let Some(text) = text {
                env.lines
                    .catalog_anchors(text.start, text.end, true, env.context);
            }
            *env.i += 1;
            let buffer = read_list_item_lines(
                env.lines,
                env.i,
                env.context,
                ListType::Callout,
                &sibling,
                true,
            );
            self.current = Some((off, line_span, text));
            let blocks = item_blocks_begin(env.lines, buffer, ListType::Callout, true);
            match item_step(env, blocks, self) {
                Ok(list) => self = list,
                Err(step) => return step,
            }
        }
        env.context.callouts.next_list();
        let mut block = Block::new(
            Span::new(self.start, self.end),
            BlockKind::CalloutList { items: self.items },
        );
        block.attrs.style = Some("arabic".into());
        block.attribute_context = Arc::clone(&env.context.attributes);
        Step::Done(Some(block))
    }
}

/// Byte offset of subslice `sub` within `source`. `sub` must be a slice that
/// points into `source` (e.g. produced by `trim`/`strip_prefix` on a line).
fn abs_offset(source: &str, sub: &str) -> usize {
    sub.as_ptr() as usize - source.as_ptr() as usize
}

/// Ruby's `ExtAtxSectionTitleRx` (Markdown syntax is on by default): one to
/// six `=` or `#`, spaces or tabs, the title, and optionally the same marker
/// again after spaces or tabs (`== Title ==`), which is not part of the title.
pub(crate) fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let marker_char = line.chars().next().filter(|c| matches!(c, '=' | '#'))?;
    let count = line.chars().take_while(|&c| c == marker_char).count();
    if count > 6 {
        return None;
    }
    let (marker, rest) = line.split_at(count);
    let blank = |c: char| c == ' ' || c == '\t';
    if !rest.starts_with(blank) {
        return None;
    }
    let mut title = rest.trim_matches(blank);
    if title.is_empty() {
        return None;
    }
    if let Some(before) = title.strip_suffix(marker) {
        let trimmed = before.trim_end_matches(blank);
        if trimmed.len() < before.len() && !trimmed.is_empty() {
            title = trimmed;
        }
    }
    Some(((count - 1) as u8, title))
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
/// whole (trimmed) line — and return `(id?, reftext?)` as slices of `line`. The
/// reftext is the trimmed, non-empty remainder after the first comma. Like
/// Ruby's `BlockAnchorRx`, an empty `[[]]` is an anchor without an id (it
/// clears a pending id); an id that is not well-formed (see
/// [`is_valid_anchor_id`]) returns `None`, so the line is ordinary text.
///
/// An *inline* `[[id]]` embedded mid-line is a different construct (an empty
/// `<a id>`); this matches only a line that is wholly an anchor.
pub(crate) fn parse_block_anchor(line: &str) -> Option<(Option<&str>, Option<&str>)> {
    // Ruby's `BlockAnchorRx` is anchored at the line start: an indented
    // anchor is a literal paragraph line.
    let inner = line
        .trim_end_matches([' ', '\t'])
        .strip_prefix("[[")?
        .strip_suffix("]]")?;
    if inner.is_empty() {
        return Some((None, None));
    }
    let (id, reftext) = match inner.find(',') {
        Some(c) => {
            let rt = inner[c + 1..].ruby_strip();
            (&inner[..c], (!rt.is_empty()).then_some(rt))
        }
        None => (inner, None),
    };
    is_valid_anchor_id(id).then_some((Some(id), reftext))
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
    (significant != b'.' && !significant.is_ascii_whitespace()).then(|| rest.ruby_rstrip())
}

/// Whether `id` is a well-formed anchor id (asciidoctor's `[\p{Word}:][\p{Word}:.-]*`):
/// a leading alphanumeric / `_` / `:`, then those plus `.` and `-`, no whitespace.
/// Shared with the inline scanner, which validates *inline* `[[id]]` anchors.
/// Ruby's anchor id: `[\p{Alpha}_:][\p{Word}\-:.]*` (no leading digit).
pub(crate) fn is_valid_anchor_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' || c == ':' => {}
        _ => return false,
    }
    chars.all(|c| crate::unicode_word::is_word(c) || matches!(c, ':' | '.' | '-'))
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
            BlockKind::List { ordered, items, .. } => (*ordered, items.clone()),
            other => panic!("expected List, got {other:?}"),
        }
    }

    /// The single nested list held by `item`, or panic.
    fn sublist(item: &ListItem) -> (bool, Vec<ListItem>) {
        assert_eq!(item.blocks.len(), 1, "expected exactly one nested block");
        match &item.blocks[0].kind {
            BlockKind::List { ordered, items, .. } => (*ordered, items.clone()),
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
    fn a_list_item_does_not_interrupt_a_paragraph() {
        // Like Ruby's `read_paragraph_lines` outside a list item.
        let r = parse("text\n* item");
        assert_eq!(r.document.flat_blocks().len(), 1);
        assert!(matches!(
            r.document.flat_blocks()[0].kind,
            BlockKind::Paragraph { .. }
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
            BlockKind::Listing { text, .. } if text == "code"
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
        assert_eq!(parse_block_anchor("[[id]]"), Some((Some("id"), None)));
        assert_eq!(
            parse_block_anchor("[[id,ref text]]"),
            Some((Some("id"), Some("ref text")))
        );
        // Trailing whitespace and a trimmed-empty reftext are tolerated; an
        // indented line is not an anchor (Ruby's `BlockAnchorRx` is anchored).
        assert_eq!(parse_block_anchor("[[id]]  "), Some((Some("id"), None)));
        assert_eq!(parse_block_anchor("  [[id]]"), None);
        assert_eq!(parse_block_anchor("[[id, ]]"), Some((Some("id"), None)));
        // Ruby's BlockAnchorRx: an empty `[[]]` is an anchor without an id;
        // ids are `[\p{Alpha}_:][\p{Word}\-:.]*` (Unicode, no leading digit).
        assert_eq!(parse_block_anchor("[[]]"), Some((None, None)));
        assert_eq!(
            parse_block_anchor("[[_x:y.z-1]]"),
            Some((Some("_x:y.z-1"), None))
        );
        assert_eq!(parse_block_anchor("[[ид]]"), Some((Some("ид"), None)));
        // Not anchors: a leading digit, a space in the id, an embedded
        // (inline) anchor and a single-bracket `[id]`.
        assert_eq!(parse_block_anchor("[[3-blind-mice]]"), None);
        assert_eq!(parse_block_anchor("[[a b]]"), None);
        assert_eq!(parse_block_anchor("text [[id]]"), None);
        assert_eq!(parse_block_anchor("[id]"), None);
    }

    #[test]
    fn empty_block_anchor_clears_the_pending_id_and_is_not_output() {
        let r = parse("[[a]]\n[[]]\n--\nBlock content\n--");
        assert_eq!(r.document.blocks.len(), 1);
        assert_eq!(r.document.blocks[0].id, None);
        let r = parse("[[3-blind-mice]]\n--\nsee\n--");
        assert!(matches!(
            r.document.blocks[0].kind,
            BlockKind::Paragraph { .. }
        ));
        assert_eq!(r.document.blocks[1].id, None);
    }

    #[test]
    fn atx_titles_follow_rubys_extended_syntax() {
        // ExtAtxSectionTitleRx: `=` or Markdown `#` markers, spaces or tabs,
        // and an optional closing marker of the same length.
        for (line, expected) in [
            ("== Title", Some((1, "Title"))),
            ("== Title ==", Some((1, "Title"))),
            ("==\tTitle\t==", Some((1, "Title"))),
            ("# Title", Some((0, "Title"))),
            ("## Title ##", Some((1, "Title"))),
            ("###### Six", Some((5, "Six"))),
            // A different closing length, or a marker without space, stays.
            ("== Title ===", Some((1, "Title ==="))),
            ("== Title==", Some((1, "Title=="))),
            ("== ==", Some((1, "=="))),
            (
                "== Section One [[one]] ==",
                Some((1, "Section One [[one]]")),
            ),
            ("####### Seven", None),
            ("#hash", None),
            ("==", None),
            ("== ", None),
        ] {
            assert_eq!(parse_heading(line), expected, "{line:?}");
        }
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
        // `[source,ruby]` is a recognised attribute list: the line is consumed —
        // not shown as text — and its style/language ride on the following
        // block, a verbatim paragraph turned into a listing; id/roles stay empty.
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
        assert!(matches!(&b.kind, BlockKind::Listing { text, .. } if text == "x = 1"));
        // The attribute line is excluded from the block span, like an anchor line.
        assert_eq!(
            b.span,
            Span::new("[source,ruby]\n".len(), "[source,ruby]\nx = 1".len())
        );
    }

    #[test]
    fn listing_records_the_blank_edge_lines_it_trims() {
        // Like Ruby's `Block#content`, `text` drops blank edge lines; the
        // counts keep what a highlighter numbers (`lines` before the trim),
        // and `empty` marks a body without any line.
        let edges = |src: &str| {
            let doc = parse(src).document;
            let b = &doc.flat_blocks()[0];
            let BlockKind::Listing {
                text, blank_edges, ..
            } = &b.kind
            else {
                panic!("{src:?} is no listing")
            };
            (text.clone(), *blank_edges, b.span)
        };
        let blank = |leading, trailing, empty| BlankEdges {
            leading,
            trailing,
            empty,
        };
        let src = "----\n\n\nint a; // <1>\n\n----";
        assert_eq!(
            edges(src),
            (
                "int a; // <1>".into(),
                blank(2, 1, false),
                Span::new(0, src.len())
            )
        );
        let src = "```\n\nint b;\n```";
        assert_eq!(
            edges(src),
            ("int b;".into(), blank(1, 0, false), Span::new(0, src.len()))
        );
        let src = "[source]\n--\n\nint c;\n\n--";
        assert_eq!(
            edges(src),
            (
                "int c;".into(),
                blank(1, 1, false),
                Span::new("[source]\n".len(), src.len())
            )
        );
        // No line, one blank line and two (all trimmed into `leading`).
        assert_eq!(edges("----\n----").1, blank(0, 0, true));
        assert_eq!(edges("[source]\n--\n--").1, blank(0, 0, true));
        assert_eq!(edges("```\n```").1, blank(0, 0, true));
        assert_eq!(edges("----\n\n----").1, blank(0, 0, false));
        assert_eq!(edges("----\n\n\n----").1, blank(2, 0, false));
        let (text, two, _) = edges("----\n\n\n----");
        assert_eq!(two.source(&text).as_deref(), Some("\n"));
        // The untrimmed body Ruby highlights.
        let (text, edges, _) = edges("----\n\n\nint a;\n\n----");
        assert_eq!(edges.source(&text).as_deref(), Some("\n\nint a;\n"));
        assert_eq!(blank(0, 0, true).source(""), None);
        assert_eq!(blank(0, 0, false).source("").as_deref(), Some(""));
        // Leading blank lines of a `----` body are lines of their own: the
        // callout keeps its offset into the trimmed text.
        let doc = parse("----\n\nx // <1>\n----\n<1> y").document;
        let BlockKind::Listing { text, callouts, .. } = &doc.flat_blocks()[0].kind else {
            panic!()
        };
        assert_eq!(text, "x // <1>");
        assert_eq!(&text[callouts[0].start..callouts[0].end], "// <1>");
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
            BlockKind::Literal { text, .. } => assert_eq!(text, "verbatim *not bold*"),
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
            BlockKind::Literal { text, .. } => assert_eq!(text, "verbatim *not bold* text"),
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
            BlockKind::Passthrough {
                text,
                inlines: None,
            } => assert_eq!(text, "<div>{author} *b*</div>"),
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
            BlockKind::Passthrough {
                text,
                inlines: None,
            } => assert_eq!(text, "<b>{x}</b> *b*"),
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
            BlockKind::Passthrough {
                text,
                inlines: None,
            } => assert_eq!(text, "<i>raw</i> {x}"),
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
    fn type_changing_style_keeps_an_admonition_label_as_text() {
        // Ruby checks the `NOTE: ` prefix only on a paragraph that stays one:
        // `[sidebar]`, `[TIP]` or `[literal]` keep it as text.
        let src = "[sidebar]\nIMPORTANT: .\n\n[TIP]\nNOTE: x\n\n[literal]\nNOTE: y";
        let blocks = parse(src).document.into_flat_blocks();
        match &blocks[0].kind {
            BlockKind::Sidebar { blocks } => match &blocks[0].kind {
                BlockKind::Paragraph { inlines } => match &inlines[0] {
                    Inline::Text { value, span } => {
                        assert_eq!(value, "IMPORTANT: .");
                        let start = src.find("IMPORTANT").unwrap();
                        assert_eq!(*span, Span::new(start, start + "IMPORTANT: .".len()));
                    }
                    other => panic!("expected Text, got {other:?}"),
                },
                other => panic!("expected Paragraph, got {other:?}"),
            },
            other => panic!("expected Sidebar, got {other:?}"),
        }
        let (kind, text) = admonition(&blocks[1]);
        assert_eq!((kind, text.as_str()), (AdmonitionKind::Tip, "NOTE: x"));
        let start = src.find("NOTE: x").unwrap();
        assert_eq!(blocks[1].span, Span::new(start, start + 7));
        match &blocks[2].kind {
            BlockKind::Literal { text, .. } => assert_eq!(text, "NOTE: y"),
            other => panic!("expected Literal, got {other:?}"),
        }
        // An unknown style or `[normal]` keeps the paragraph: the prefix applies.
        let (kind, text) = admonition(&parse("[normal]\nNOTE: n").document.into_flat_blocks()[0]);
        assert_eq!((kind, text.as_str()), (AdmonitionKind::Note, "n"));
    }

    #[test]
    fn admonition_style_is_inert_on_a_quote_block() {
        // `____` masquerades only as `verse` (Ruby's `DELIMITED_BLOCKS`).
        let src = "[NOTE]\n____\nq\n____";
        let block = &parse(src).document.into_flat_blocks()[0];
        assert!(
            matches!(block.kind, BlockKind::Quote { .. }),
            "{:?}",
            block.kind
        );
        assert_eq!(block.attrs.style.as_deref(), Some("NOTE"));
        assert_eq!(block.span, Span::new("[NOTE]\n".len(), src.len()));
    }

    #[test]
    fn indented_comment_lookalike_stays_list_item_text() {
        // Ruby skips line comments before `adjust_indentation!`: ` // x` is
        // text; a `//` line at the left margin is still a comment.
        let src = "* m\n // x\n\n* d\n// z\n e";
        let blocks = parse(src).document.into_flat_blocks();
        let BlockKind::List { items, .. } = &blocks[0].kind else {
            panic!("expected List, got {:?}", blocks[0].kind);
        };
        let text = |inlines: &[Inline]| -> (String, Vec<Span>) {
            inlines
                .iter()
                .map(|inline| match inline {
                    Inline::Text { value, span } => (value.clone(), *span),
                    other => panic!("expected Text, got {other:?}"),
                })
                .fold(
                    (String::new(), Vec::new()),
                    |(mut text, mut spans), (value, span)| {
                        text.push_str(&value);
                        spans.push(span);
                        (text, spans)
                    },
                )
        };
        let (first, spans) = text(&items[0].inlines);
        assert!(first.contains("// x"), "{first:?}");
        assert_eq!(spans.last().unwrap().end, src.find(" // x").unwrap() + 5);
        let (second, _) = text(&items[1].inlines);
        assert!(!second.contains("// z"), "{second:?}");
        assert!(second.ends_with('e'), "{second:?}");
    }

    #[test]
    fn doctitle_attribute_is_no_title_under_an_invalid_header() {
        // A block title above the implicit doctitle ends Ruby's header
        // parsing before `doctitle` becomes the document title.
        let r = parse(".e\n:doctitle: d\n= e");
        assert!(!r.document.has_title);
        assert_eq!(r.document.attributes.get("doctitle"), Some("d"));
        // Without a block title it is the document title.
        assert!(parse(":doctitle: d\n\np").document.has_title);
    }

    #[test]
    fn a_dropped_first_block_makes_the_section_check_apply() {
        // Ruby peeks the first body line (`is_next_line_section?`): a body
        // whose first block is dropped does not start with a section.
        for (src, line) in [
            (":attribute-missing: drop-line\nimage::{e}[]\n= e", 2),
            ("[comment]\nx\n\n= e", 3),
            ("|===\na|[comment]\nx\n\n= e\n|===", 4),
        ] {
            let r = parse(src);
            let level0: Vec<_> = r
                .diagnostics
                .iter()
                .filter(|d| d.message == "level 0 sections can only be used when doctype is book")
                .collect();
            assert_eq!(level0.len(), 1, "{src:?}: {:?}", r.diagnostics);
            let start = src
                .split_inclusive('\n')
                .take(line)
                .map(str::len)
                .sum::<usize>();
            assert_eq!(level0[0].span, Span::new(start, start + 3), "{src:?}");
        }
        // A comment line or block is metadata: the body starts with the title.
        for src in ["// c\n= e", "////\nx\n////\n= e", "[[x]]\n= e"] {
            assert!(parse(src).diagnostics.is_empty(), "{src:?}");
        }
    }

    #[test]
    fn section_title_preprocesses_the_next_line_first() {
        // Ruby peeks two lines for a title, so a directive after it reports
        // before the title's level check.
        let src = "1\n\n= c\nendif::[]";
        let r = parse(src);
        let messages: Vec<_> = r.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "unmatched endif directive",
                "level 0 sections can only be used when doctype is book",
            ]
        );
        let endif = src.find("endif").unwrap();
        assert_eq!(r.diagnostics[0].span, Span::new(endif, src.len()));
        assert_eq!(r.diagnostics[1].span, Span::new(3, 6));
        // The level error is logged at Ruby's cursor after that peek: the
        // dropped directive advanced it past the title, while the editor span
        // stays on the title.
        let lines: Vec<_> = r.diagnostics.iter().map(report_line).collect();
        assert_eq!(lines, [Some(4), Some(4)]);
    }

    fn report_line(diagnostic: &Diagnostic) -> Option<usize> {
        match diagnostic.report.location {
            crate::DiagnosticLocation::Cursor(cursor) => Some(cursor.line),
            _ => None,
        }
    }

    #[test]
    fn a_block_mark_follows_the_section_peek() {
        // Ruby marks a block after `is_next_line_section?` peeked two
        // preprocessed lines: a line dropped between them moves the mark
        // (and what is reported at it) down until both lines are read.
        let report = |src: &str, prefix: &str| {
            let r = parse(src);
            let found = r
                .diagnostics
                .iter()
                .find(|d| d.message.starts_with(prefix))
                .unwrap_or_else(|| panic!("{src:?}: {:?}", r.diagnostics));
            (report_line(found), found.span.start)
        };
        for (src, prefix, line) in [
            ("para\n\n====\nendif::[]\n\nx", "unterminated example", 4),
            ("----\nendif::[]\nendif::[]\nx", "unterminated listing", 3),
            // Every line a skipped conditional drops counts.
            (
                "====\nifdef::x[]\na\nb\nendif::[]",
                "unterminated example",
                5,
            ),
            (
                "[[a]]\npara\n\n[[a]]\nfoo\nendif::[]\nbar",
                "id assigned to block already in use",
                6,
            ),
            // A comment style peeks the lines raw: the unprocessed directive
            // is pushed back and later swallowed by the paragraph, so the
            // fence after it is peeked with the dropped `endif`.
            (
                "para\n\n[comment]\nTitle\nifdef::x[]\n=====\nendif::[]\n\nx",
                "unterminated example",
                7,
            ),
            // No section peek: a discrete style, a list item's block.
            (
                "x\n\n[discrete]\n====\nendif::[]\ny",
                "unterminated example",
                4,
            ),
            ("* a\n+\n====\nendif::[]\nx", "unterminated example", 3),
        ] {
            let (found, start) = report(src, prefix);
            assert_eq!(found, Some(line), "{src:?}");
            // The editor span stays on the block's first line.
            let first = src[start..].lines().next().unwrap();
            assert!(!first.starts_with("endif"), "{src:?}");
        }
    }

    #[test]
    fn every_section_level_check_uses_the_peek_cursor() {
        // `section title out of sequence`, `level 0 sections …` and
        // `… do not support nested sections` share Ruby's `reader.cursor`.
        for (src, message, line) in [
            (
                "= D\n\n[glossary]\n== G\n\n=== B\nendif::[]\n\ntext",
                "glossary sections do not support nested sections",
                7,
            ),
            (
                "= D\n\n=== B\nendif::[]\nendif::[]\n\ntext",
                "section title out of sequence: expected level 1, got level 2",
                5,
            ),
            (
                "x\n\nTitle\nendif::[]\n=====",
                "level 0 sections can only be used when doctype is book",
                4,
            ),
            // Without a dropped line the cursor is the title's own line.
            (
                "x\n\n= T\ny",
                "level 0 sections can only be used when doctype is book",
                3,
            ),
        ] {
            let r = parse(src);
            let found = r
                .diagnostics
                .iter()
                .find(|d| d.message == message)
                .unwrap_or_else(|| panic!("{src:?}: {:?}", r.diagnostics));
            assert_eq!(report_line(found), Some(line), "{src:?}");
        }
    }

    #[test]
    fn unshown_titles_register_no_footnotes() {
        // Ruby never converts a nested document's title (`notitle`) nor an
        // undisplayed document title: their footnotes do not exist.
        let src = "|===\na|= T footnote:[f]\n\nx footnote:[g]\n|===";
        let r = parse(src);
        let BlockKind::Table(table) = &r.document.flat_blocks()[0].kind else {
            panic!("expected a table");
        };
        let cell = table.cells().next().unwrap();
        let crate::ast::CellContent::Blocks { footnotes, .. } = &cell.content else {
            panic!("expected an AsciiDoc cell");
        };
        let indexes: Vec<_> = footnotes.iter().map(|f| f.index.as_str()).collect();
        assert_eq!(indexes, ["1"]);
        assert_eq!(footnotes[0].span.start, src.find("footnote:[g]").unwrap());
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
            BlockKind::Listing { text, .. } => assert_eq!(text, "code"),
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
            BlockKind::Listing { text, .. } => assert_eq!(text, "code"),
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
            BlockKind::Listing { text, .. } => assert_eq!(text, "code"),
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
            BlockKind::Listing { text, .. } => assert_eq!(text, "code"),
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
        // delimiter but Markdown's thematic break.
        let b = &parse("--\ntext\n--").document.into_flat_blocks()[0];
        assert!(matches!(b.kind, BlockKind::Open { .. }));
        let hr = &parse("---\n\ntext").document.into_flat_blocks()[0];
        assert!(
            matches!(hr.kind, BlockKind::ThematicBreak),
            "got {:?}",
            hr.kind
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
            BlockKind::Listing { text, .. } => assert_eq!(text, "puts *not bold*"),
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
            BlockKind::Literal { text, .. } => assert_eq!(text, "verbatim *not bold*"),
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
