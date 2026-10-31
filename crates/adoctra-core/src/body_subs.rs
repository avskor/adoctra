//! Conversion-time inline evaluation. Reading records source and evaluated
//! declarations; the displayed document title and surviving body fields execute
//! after the reader finishes. Section titles with a generated id and eligible
//! anchored titles have already been cached; other section titles convert at
//! their first use (a TOC, `<title>`, a reference or the heading). Captions
//! replay their reading-time counters at entry.
//! Cross-references resolve here too, after each field's substitutions, so a
//! label sees the conversion state at that reference (see resolve.rs).
use crate::scanner::FilteredSource;
use crate::substitution_context::Evaluation;
use crate::{AttributePart, Attributes, Block, BlockKind, Diagnostic, Inline, Span};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone)]
struct Pending {
    bibliography: bool,
    source: FilteredSource,
    filtered: Option<FilteredSource>,
    fold: bool,
    hardbreaks: bool,
    replacement_origin: Option<Span>,
    block: Option<BlockSubs>,
}

/// Substitutions selected for a block's content instead of the normal
/// inline scan (Ruby's committed `subs`).
#[derive(Clone)]
pub(crate) struct BlockSubs {
    pub stages: Vec<crate::pass_subs::Stage>,
    /// The block's `line-comment` for its callouts stage.
    pub line_comment: Option<String>,
    /// Ids the callouts catalog registered while reading, in order (Ruby's
    /// `read_next_id`).
    pub callout_ids: Vec<String>,
    /// Verbatim and raw content trims blank edge lines after substitutions.
    pub trim: bool,
    /// A `source` listing's named attributes and options: a converter may
    /// highlight it in place of `specialcharacters` (Ruby's `highlight`).
    pub source_listing: Option<(BTreeMap<String, String>, Vec<String>)>,
}

impl BlockSubs {
    /// A block's default substitutions.
    pub fn new(defaults: crate::pass_subs::Defaults) -> Self {
        Self {
            stages: defaults.stages(),
            line_comment: None,
            callout_ids: Vec::new(),
            trim: defaults != crate::pass_subs::Defaults::Normal,
            source_listing: None,
        }
    }
}

pub(crate) type Declaration = (usize, String, Option<Vec<AttributePart>>);

/// A nested document's conversion state: its header snapshot, its own
/// attribute entries and its locked attributes (Ruby's inner document).
#[derive(Clone)]
pub(crate) struct Nested {
    pub attributes: Attributes,
    pub entries: Vec<Declaration>,
    pub locked: BTreeSet<String>,
    pub footnotes: crate::substitution_context::Footnotes,
}

/// Shared conversion state and outputs, borrowed from the parse context.
pub(crate) struct Conversion<'a> {
    pub locked: &'a BTreeSet<String>,
    pub counters: &'a mut BTreeMap<String, String>,
    pub diagnostics: &'a mut Vec<Diagnostic>,
    pub registry: &'a crate::resolve::Registry,
    pub early_titles: crate::resolve::EarlyTitles,
    pub fixed: crate::resolve::FixedXrefs,
    pub sections: &'a BTreeMap<(usize, usize), crate::section_numbers::Section>,
    /// Unresolved-reference warnings.
    pub references: &'a mut Vec<Diagnostic>,
    pub xrefs: crate::xref_target::Scope<'a>,
    /// Footnotes registered while reading.
    pub footnotes: crate::substitution_context::Footnotes,
    pub images: Option<&'a crate::data_uri::Images<'a>>,
}

#[derive(Default)]
pub(crate) struct BodySubs {
    pub tokens: Vec<crate::Token>,
    header: Option<Pending>,
    pending: BTreeMap<(usize, usize), Pending>,
    pub entries: Vec<Declaration>,
    pub caption_counters: BTreeMap<(usize, usize), (&'static str, Vec<AttributePart>)>,
    /// AsciiDoc table cells, keyed by the cell's reader range.
    pub nested: BTreeMap<(usize, usize), Nested>,
    /// Blocks Ruby's converter excludes (a misplaced abstract or partintro),
    /// keyed by their range: never converted, so nothing inside registers
    /// (footnotes) or reads the conversion attributes.
    pub excluded: BTreeSet<(usize, usize)>,
}

impl BodySubs {
    pub fn bibliography(&mut self, nodes: &[Inline]) {
        if let Some(key) = crate::resolve::placeholder(nodes) {
            if let Some(pending) = self.pending.get_mut(&key) {
                pending.bibliography = true;
            }
        }
    }

    pub fn defer(
        &mut self,
        source: FilteredSource,
        filtered: Option<FilteredSource>,
        attrs: &Attributes,
        fold: bool,
    ) -> Vec<Inline> {
        if filtered.as_ref().unwrap_or(&source).text.is_empty() {
            return Vec::new();
        }
        let span = source.original_span(Span::new(0, source.text.len()));
        self.pending.insert(
            (span.start, span.end),
            Pending {
                bibliography: false,
                source,
                filtered,
                fold,
                hardbreaks: attrs.local_hardbreaks,
                replacement_origin: None,
                block: None,
            },
        );
        // Private placeholder; replaced before source mapping or public output.
        vec![Inline::Text {
            value: String::new(),
            span,
        }]
    }

    /// Content converted with selected substitutions rather than the
    /// normal inline scan. An empty source has nothing to convert.
    pub fn defer_block(
        &mut self,
        source: FilteredSource,
        filtered: Option<FilteredSource>,
        attrs: &Attributes,
        subs: BlockSubs,
    ) -> Vec<Inline> {
        let nodes = self.defer(source, filtered, attrs, false);
        self.set_block_subs(&nodes, subs);
        nodes
    }

    /// Select the substitutions of already deferred content (a paragraph
    /// whose `subs` are committed on completion).
    pub fn set_block_subs(&mut self, nodes: &[Inline], subs: BlockSubs) {
        if let Some(pending) =
            crate::resolve::placeholder(nodes).and_then(|key| self.pending.get_mut(&key))
        {
            pending.block = Some(subs);
        }
    }

    /// A block title runs during conversion. A `title=` value transformed by
    /// the attribute-list a-stage takes its field's physical range.
    pub fn defer_title(
        &mut self,
        text: &str,
        base: usize,
        origin: Option<Span>,
        attrs: &Attributes,
    ) -> Vec<Inline> {
        let nodes = self.defer(
            FilteredSource::from_fragments([(base, text)]),
            None,
            attrs,
            true,
        );
        if let (Some(origin), [Inline::Text { span, .. }]) = (origin, nodes.as_slice()) {
            if let Some(pending) = self.pending.get_mut(&(span.start, span.end)) {
                pending.replacement_origin = Some(origin);
            }
        }
        nodes
    }

    /// The displayed document title runs after reading, before body conversion.
    /// A doctitle override has an enclosing declaration/host origin rather than
    /// offsets into the transformed replacement string.
    pub fn defer_header(
        &mut self,
        title: &crate::header::HeaderTitle,
        attrs: &Attributes,
    ) -> Vec<Inline> {
        let source = FilteredSource::from_fragments([(title.span.start, title.text.as_str())]);
        // Keep this separate from physical body ranges: host replacements can
        // have the same synthetic offsets as the first paragraph.
        self.header = Some(Pending {
            bibliography: false,
            source,
            filtered: None,
            fold: true,
            hardbreaks: attrs.local_hardbreaks,
            replacement_origin: title.replacement.then_some(title.span),
            block: None,
        });
        vec![Inline::Text {
            value: String::new(),
            span: title.span,
        }]
    }

    /// A deferred block title converted while reading, for a reference in a
    /// title Ruby converts at parse time. It uses the reading attributes; its
    /// expressions do not change later state. The registry memoizes the
    /// result for the block's own conversion.
    pub fn convert_early(
        &mut self,
        key: (usize, usize),
        attributes: &Attributes,
        locked: &BTreeSet<String>,
        counters: &BTreeMap<String, String>,
        xrefs: crate::xref_target::Scope<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Vec<Inline>> {
        let pending = self.pending.remove(&key)?;
        let mut attributes = attributes.clone();
        attributes.local_hardbreaks = pending.hardbreaks;
        let mut counters = counters.clone();
        let compat = attributes.get("compat-mode").is_some();
        let mut evaluation =
            Evaluation::definition(&attributes, locked, &mut counters, compat).with_xrefs(xrefs);
        Some(run(
            pending,
            &mut evaluation,
            diagnostics,
            &mut self.tokens,
            None,
        ))
    }

    pub fn execute(
        mut self,
        blocks: &mut [Block],
        attributes: Attributes,
        conversion: Conversion,
        start: Option<&mut dyn crate::ConversionStart>,
    ) -> (
        Attributes,
        Vec<crate::Token>,
        Vec<crate::ast::FootnoteDefinition>,
    ) {
        let mut execution = Execution {
            excluded: std::mem::take(&mut self.excluded),
            heading_converted: false,
            start,
            tokens: &mut self.tokens,
            pending: &mut self.pending,
            caption_counters: &mut self.caption_counters,
            entries: self.entries.into_iter().peekable(),
            attributes,
            snapshot: None,
            locked: std::borrow::Cow::Borrowed(conversion.locked),
            nested: std::mem::take(&mut self.nested),
            root: None,
            footnotes: conversion.footnotes,
            images: conversion.images,
            counters: conversion.counters,
            diagnostics: conversion.diagnostics,
            registry: conversion.registry,
            references: conversion.references,
            xrefs: conversion.xrefs,
            reftexts: None,
            titles: conversion.early_titles,
            fixed: conversion.fixed,
            sections: conversion.sections,
            headings: BTreeMap::new(),
            referable: referable_titles(conversion.registry),
            converting: BTreeMap::new(),
            header_title: None,
            header_converting: None,
            header_lazy: None,
            first_section: None,
            resolving: false,
            outline: Vec::new(),
            preamble_toc: None,
        };
        let mut body = 0;
        // Ruby converts the document title first where the output shows it
        // (`<title>`, or the embedded heading without `notitle`); otherwise
        // only a use (a reference to the document) converts it.
        let displayed = execution.attributes.get("embedded").is_none()
            || execution.attributes.get("notitle").is_none();
        if let (Some(header), Some(block)) = (self.header, blocks.first_mut()) {
            if let BlockKind::Heading { inlines, .. } = &mut block.kind {
                if displayed {
                    *inlines = execution.convert_header(header);
                } else {
                    execution.header_lazy = Some(header);
                }
                body = 1;
            }
        }
        execution.outline = Outline::of(&blocks[body..]);
        // Section titles Ruby converts at their first use: a standalone
        // document's `<title>` falls back to the first section's, and an
        // automatic TOC converts the outline before the body (both with the
        // header attributes).
        let sectioned = !execution.outline.is_empty();
        let header = &execution.attributes;
        let standalone = header.get("embedded").is_none();
        if standalone && body == 0 && header.get("title").is_none() {
            let first = execution.outline.first();
            if let Some(first) = first.filter(|f| crate::resolve::placeholder(&f.title).is_some()) {
                let (key, title) = (first.heading, first.title.clone());
                execution.lazy_heading(key, &title);
            }
        }
        // Ruby's head follows the `<title>` and precedes the header outline
        // and the body: a converter reads its stylesheet now, with the header
        // attributes, before the body's images.
        if let Some(start) = execution.start.as_deref_mut() {
            for message in start.start(&execution.attributes) {
                execution.diagnostics.push(crate::Diagnostic {
                    omitted_span: None,
                    report: crate::DiagnosticReport {
                        location: crate::DiagnosticLocation::Unlocated,
                        ..Default::default()
                    },
                    span: crate::Span::new(0, 0),
                    severity: crate::Severity::Warning,
                    message,
                });
            }
        }
        let header = &execution.attributes;
        let toc = sectioned && header.get("toc").is_some();
        let placement = header.get("toc-placement");
        if toc && placement == Some("preamble") {
            execution.preamble_toc = Some(toclevels(header.get("toclevels")));
        }
        let auto = match standalone {
            true => placement == Some("auto"),
            false => !matches!(placement, Some("macro" | "preamble")),
        };
        if toc && auto {
            let levels = toclevels(header.get("toclevels"));
            execution.toc(levels);
        }
        // Ruby's `Document#content` deletes `title` before the body converts.
        execution.attributes.remove("title");
        execution.first_section = blocks[body..].iter().find_map(|block| match &block.kind {
            BlockKind::Section { blocks, .. } => match blocks.first().map(|b| (b.span, &b.kind)) {
                Some((span, BlockKind::Heading { inlines, .. })) => {
                    Some(((span.start, span.end), inlines.clone()))
                }
                _ => None,
            },
            _ => None,
        });
        execution.blocks(&mut blocks[body..]);
        // An undisplayed title keeps its nodes in the tree: one a reference
        // converted, else converted now (after the body, which it cannot
        // affect any more).
        if body == 1 {
            if let BlockKind::Heading { inlines, .. } = &mut blocks[0].kind {
                if let Some(header) = execution.header_lazy.take() {
                    *inlines = execution.convert_header(header);
                } else if let Some(title) = &execution.header_title {
                    inlines.clone_from(title);
                }
            }
        }
        crate::resolve::resolve_attribute_definitions(&mut execution.attributes, &execution.fixed);
        let footnotes = execution.footnotes.into_definitions(&execution.fixed);
        (execution.attributes, self.tokens, footnotes)
    }
}

struct Execution<'a, 's> {
    /// See [`BodySubs::excluded`].
    excluded: BTreeSet<(usize, usize)>,
    /// A heading's text was converted on entry; the walk skips it once.
    heading_converted: bool,
    /// The converter's hook ([`crate::ConversionStart`]).
    start: Option<&'s mut dyn crate::ConversionStart>,
    tokens: &'a mut Vec<crate::Token>,
    pending: &'a mut BTreeMap<(usize, usize), Pending>,
    caption_counters: &'a mut BTreeMap<(usize, usize), (&'static str, Vec<AttributePart>)>,
    entries: std::iter::Peekable<std::vec::IntoIter<Declaration>>,
    attributes: Attributes,
    /// The last conversion snapshot, shared by blocks while it stays current.
    snapshot: Option<Arc<Attributes>>,
    locked: std::borrow::Cow<'a, BTreeSet<String>>,
    nested: BTreeMap<(usize, usize), Nested>,
    /// The root document's attributes and locks while a nested document
    /// converts: its counters delegate there.
    root: Option<(Attributes, std::borrow::Cow<'a, BTreeSet<String>>)>,
    /// The converting document's footnote registry.
    footnotes: crate::substitution_context::Footnotes,
    images: Option<&'a crate::data_uri::Images<'a>>,
    counters: &'a mut BTreeMap<String, String>,
    diagnostics: &'a mut Vec<Diagnostic>,
    registry: &'a crate::resolve::Registry,
    references: &'a mut Vec<Diagnostic>,
    xrefs: crate::xref_target::Scope<'a>,
    /// Ruby's `@reftexts`: lexical label to id, built by the first natural
    /// reference to an unknown id and kept for the rest of the conversion.
    reftexts: Option<BTreeMap<String, String>>,
    /// Ruby's `@converted_title` for referable block titles, keyed by their
    /// placeholder range: the first reference or the block converts it once.
    titles: BTreeMap<(usize, usize), Vec<Inline>>,
    /// References whose attribute definition fixed their target and label.
    fixed: crate::resolve::FixedXrefs,
    /// Section titles and their numbers.
    sections: &'a BTreeMap<(usize, usize), crate::section_numbers::Section>,
    /// Section titles with resolved references, keyed by the heading range.
    headings: BTreeMap<(usize, usize), Vec<Inline>>,
    /// Placeholder ranges of block titles that a reference may display.
    referable: BTreeSet<(usize, usize)>,
    /// Titles being converted; a reference inside converts them again.
    converting: BTreeMap<(usize, usize), Pending>,
    header_title: Option<Vec<Inline>>,
    /// The document title while it converts: a reference to the document
    /// inside it converts it again (Ruby memoizes the title only after the
    /// outer conversion).
    header_converting: Option<Pending>,
    /// The document title not converted yet because the output does not
    /// show it: the first reference to the document converts it.
    header_lazy: Option<Pending>,
    /// Without a document title, Ruby's `doctitle` is the first top-level
    /// section's title (keyed by its heading range).
    first_section: Option<((usize, usize), Vec<Inline>)>,
    /// Ruby's `@resolving_xref`: references inside a label being produced
    /// display `[id]`.
    resolving: bool,
    /// The root document's sections, for the titles a table of contents
    /// converts first.
    outline: Vec<Outline>,
    /// `toclevels` of a TOC at the preamble's end (header attributes).
    preamble_toc: Option<i128>,
}

/// A section of the root document: its heading range and title.
struct Outline {
    level: u8,
    heading: (usize, usize),
    title: Vec<Inline>,
    sections: Vec<Outline>,
}

impl Outline {
    fn of(blocks: &[Block]) -> Vec<Self> {
        blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Section { level, blocks } => match blocks.first() {
                    Some(Block {
                        span,
                        kind: BlockKind::Heading { inlines, .. },
                        ..
                    }) => Some(Self {
                        level: *level,
                        heading: (span.start, span.end),
                        title: inlines.clone(),
                        sections: Self::of(&blocks[1..]),
                    }),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }
}

/// Ruby's `toclevels` (or a TOC macro's `levels`), as `to_i`.
fn toclevels(levels: Option<&str>) -> i128 {
    levels.map_or(2, crate::include_selection::integer_prefix)
}

impl crate::substitution_context::Labels for Execution<'_, '_> {
    /// The natural lookup and label of a reference in the macros stage, with
    /// the converting field's state.
    fn label(
        &mut self,
        mut state: crate::substitution_context::Lent<'_>,
        xref: &mut crate::Xref,
        textless: bool,
        span: Span,
    ) -> Option<Vec<Inline>> {
        let mut swap = |execution: &mut Self| {
            std::mem::swap(&mut execution.attributes, state.attributes);
            std::mem::swap(execution.counters, state.counters);
            if let (Some((current, _)), Some(lent)) = (execution.root.as_mut(), &mut state.root) {
                std::mem::swap(current, *lent);
            }
            std::mem::swap(&mut execution.footnotes, state.footnotes);
        };
        swap(self);
        let hardbreaks = self.attributes.local_hardbreaks;
        if let Some(text) = crate::resolve::natural_text(xref, self.registry, true) {
            if let Some(id) = self.resolve_id(&text) {
                crate::xref_target::retarget(xref, &id, true);
            }
        }
        let label = textless.then(|| self.xref_label(xref, true, span));
        self.attributes.local_hardbreaks = hardbreaks;
        swap(self);
        label
    }
}

impl Execution<'_, '_> {
    /// The current conversion attributes as a block snapshot (without the
    /// transient local hardbreaks flag).
    fn snapshot(&mut self) -> Arc<Attributes> {
        match &self.snapshot {
            Some(snapshot) if snapshot.same_values(&self.attributes) => Arc::clone(snapshot),
            _ => {
                let mut current = self.attributes.clone();
                current.local_hardbreaks = false;
                Arc::clone(self.snapshot.insert(Arc::new(current)))
            }
        }
    }

    fn replay(&mut self, at: usize) {
        while self.entries.peek().is_some_and(|e| e.0 <= at) {
            let (_, key, parts) = self.entries.next().unwrap();
            if let Some(parts) = parts {
                self.attributes.set_parts(key, parts);
            } else {
                self.attributes.remove(&key);
            }
        }
    }

    fn inlines(&mut self, nodes: &mut Vec<Inline>) {
        let Some(key) = crate::resolve::placeholder(nodes) else {
            return self.resolve(nodes);
        };
        if let Some(converted) = self.titles.get(&key) {
            // Already converted by an earlier (forward) reference.
            *nodes = converted.clone();
        } else if let Some(pending) = self.pending.remove(&key) {
            self.replay(key.0);
            *nodes = self.convert(key, pending);
        }
    }

    /// Convert a deferred field and resolve its references. A referable title
    /// is cached, as Ruby memoizes `@converted_title`.
    fn convert(&mut self, key: (usize, usize), pending: Pending) -> Vec<Inline> {
        let referable = self.referable.contains(&key);
        if referable {
            self.converting.insert(key, pending.clone());
        }
        let mut nodes = vec![Inline::Text {
            value: String::new(),
            span: Span::new(key.0, key.1),
        }];
        self.evaluate(&mut nodes, pending);
        self.resolve(&mut nodes);
        if referable {
            self.converting.remove(&key);
            self.titles.insert(key, nodes.clone());
        }
        nodes
    }

    /// Resolve every cross-reference in a converted field, in order: look up a
    /// natural reference, then give a textless one its label now, with the
    /// current conversion state.
    fn resolve(&mut self, nodes: &mut [Inline]) {
        for node in nodes {
            match node {
                Inline::CrossReference {
                    xref,
                    children,
                    span,
                } => {
                    let resolved = std::mem::take(&mut xref.resolved);
                    let label = xref.label.take();
                    self.target(xref, false, *span);
                    if let Some((at, mut label)) = label {
                        // Generated label text (`[id]`, a path, a style's
                        // words) displays at the reference.
                        struct At(Span, Span);
                        impl crate::visit::VisitorMut for At {
                            fn visit_span(&mut self, span: &mut Span) {
                                if *span == self.0 {
                                    *span = self.1;
                                }
                            }
                        }
                        crate::visit::walk_inlines_mut(&mut At(at, *span), &mut label);
                        *children = label;
                    } else if children.is_empty() && !resolved {
                        // Borrowed labels are not expanded again.
                        *children = self.xref_label(xref, false, *span);
                    } else {
                        self.resolve(children);
                    }
                }
                Inline::Strong { children, .. }
                | Inline::Emphasis { children, .. }
                | Inline::Monospace { children, .. }
                | Inline::Mark { children, .. }
                | Inline::Superscript { children, .. }
                | Inline::Subscript { children, .. }
                | Inline::Link { children, .. } => self.resolve(children),
                // References generated inside attribute values.
                Inline::AttributeRef { span, .. }
                | Inline::Substituted { span, .. }
                | Inline::Footnote { span, .. }
                | Inline::Ui { span, .. }
                | Inline::IndexTerm { span, .. }
                | Inline::Stem { span, .. } => {
                    let span = *span;
                    crate::resolve::fill_labels(node, |xref, textless| {
                        if std::mem::take(&mut xref.resolved) {
                            self.target(xref, true, span);
                            return None;
                        }
                        if let Some(label) = crate::resolve::apply_fixed(&self.fixed, xref) {
                            self.target(xref, true, span);
                            return Some(label);
                        }
                        self.target(xref, true, span);
                        textless.then(|| self.xref_label(xref, true, span))
                    });
                }
                _ => {}
            }
        }
    }

    /// A natural reference to an unknown id that resembles reftext takes the
    /// id whose label spells it. An internal id still unknown is reported.
    fn target(&mut self, xref: &mut crate::Xref, lexical: bool, span: Span) {
        if let Some(text) = crate::resolve::natural_text(xref, self.registry, lexical) {
            if let Some(id) = self.resolve_id(&text) {
                crate::xref_target::retarget(xref, &id, lexical);
            }
        }
        // An empty id (`<<#>>`) is the current document.
        if let Some(id) = crate::resolve::decoded_refid(xref, lexical) {
            if !id.is_empty() && !self.registry.contains_key(&id) {
                self.references.push(Diagnostic {
                    omitted_span: None,
                    // Ruby's sub_macros logs an unlocated INFO, only at
                    // that level.
                    report: crate::DiagnosticReport {
                        verbose_only: true,
                        severity: Some(crate::Severity::Info),
                        message: Some(format!("possible invalid reference: {id}")),
                        location: crate::DiagnosticLocation::Unlocated,
                        ..Default::default()
                    },
                    span,
                    severity: crate::Severity::Warning,
                    message: format!("unresolved cross reference: '{id}'"),
                });
            }
        }
    }

    /// Ruby's `Document#resolve_id` after parsing: every registered node's
    /// label, in registration order, the first spelling winning.
    fn resolve_id(&mut self, text: &str) -> Option<String> {
        if self.reftexts.is_none() {
            // Assigned eagerly: a lookup while building sees the partial table.
            self.reftexts = Some(BTreeMap::new());
            let registry = self.registry;
            for id in crate::resolve::ordered(registry) {
                let label = self.xreftext(id, None, Span::new(0, 0));
                if let Some(spelling) = label.as_deref().and_then(crate::xref_target::lexical_label)
                {
                    let table = self.reftexts.as_mut().expect("table");
                    table.entry(spelling).or_insert_with(|| id.to_owned());
                }
            }
        }
        self.reftexts.as_ref()?.get(text).cloned()
    }

    /// The label of a textless reference: another document's path, the
    /// current document's title (`[^top]` without one), or the target's label.
    fn xref_label(&mut self, xref: &crate::Xref, lexical: bool, span: Span) -> Vec<Inline> {
        if let Some(path) = &xref.path {
            return crate::entity_references::reference_inlines(
                &crate::resolve::decoded(path, lexical),
                span,
            );
        }
        match crate::resolve::decoded_refid(xref, lexical) {
            Some(id) if !id.is_empty() || self.registry.contains_key(&id) => {
                // Ruby's `node.attr 'xrefstyle', nil, true`.
                let style = match &xref.xrefstyle {
                    Some(style) => Some(crate::resolve::decoded(style, lexical)),
                    None => self.attributes.get("xrefstyle").map(str::to_owned),
                };
                let fallback =
                    crate::entity_references::unknown_reference_label(xref, &id, lexical, span);
                self.label(&id, style.as_deref(), span, fallback)
            }
            _ => {
                let mut label = None;
                if !self.resolving {
                    self.resolving = true;
                    label = self.document_xreftext(span);
                    self.resolving = false;
                }
                let mut label = label.unwrap_or_else(|| {
                    vec![Inline::Text {
                        value: "[^top]".into(),
                        span,
                    }]
                });
                crate::resolve::strip_label(&mut label);
                label
            }
        }
    }

    /// Ruby's `Document#xreftext`: the document's nonempty reftext, else its
    /// `doctitle` (the document title, else the first section's title).
    fn document_xreftext(&mut self, span: Span) -> Option<Vec<Inline>> {
        let registry = self.registry;
        let header = registry
            .iter()
            .find(|(_, target)| matches!(target.title, Some(crate::resolve::Title::Header)));
        if let Some((id, _)) = header.filter(|(_, target)| target.reftext.is_some()) {
            return self.xreftext(id, None, span);
        }
        if let Some(value) = self.attributes.get("reftext").filter(|v| !v.is_empty()) {
            let mut nodes = self.reftext(crate::reftext::Read::source(value, span.start));
            struct Origin(Span);
            impl crate::visit::VisitorMut for Origin {
                fn visit_span(&mut self, span: &mut Span) {
                    *span = self.0;
                }
            }
            crate::visit::walk_inlines_mut(&mut Origin(span), &mut nodes);
            return Some(nodes);
        }
        if let Some(title) = self.header_label() {
            return Some(title);
        }
        let (key, nodes) = self.first_section.clone()?;
        Some(self.heading(key, &nodes))
    }

    /// Ruby's `xreftext`: a nonempty reftext, else the (styled) title, else
    /// `[id]`. References inside a label being produced display `[id]`.
    fn label(
        &mut self,
        id: &str,
        style: Option<&str>,
        span: Span,
        fallback: Vec<Inline>,
    ) -> Vec<Inline> {
        if !self.registry.contains_key(id) || self.resolving {
            return fallback;
        }
        self.resolving = true;
        let label = self.xreftext(id, style, span);
        self.resolving = false;
        let Some(mut label) = label else {
            return fallback;
        };
        crate::resolve::strip_label(&mut label);
        label
    }

    /// A registered node's nonempty reftext (formatted now), else its title
    /// with the xrefstyle applied (see resolve::styled).
    fn xreftext(&mut self, id: &str, style: Option<&str>, span: Span) -> Option<Vec<Inline>> {
        let registry = self.registry;
        let target = registry.get(id)?;
        let label = target
            .reftext
            .clone()
            .map(|read| self.reftext(read))
            .filter(|nodes| !crate::resolve::inline_text(nodes).is_empty());
        if label.is_some() {
            return label;
        }
        let title = match &target.title {
            Some(crate::resolve::Title::Nodes(nodes)) => self.title(nodes),
            Some(crate::resolve::Title::Heading(key, nodes)) => self.heading(*key, nodes),
            Some(crate::resolve::Title::Header) => return self.header_label(),
            None => return None,
        };
        Some(crate::resolve::styled(
            title,
            &target.numbering,
            style,
            &self.attributes,
            span,
        ))
    }

    /// Convert the document title once (Ruby's memo is assigned after the
    /// conversion, so a reference inside it converts it again).
    fn convert_header(&mut self, header: Pending) -> Vec<Inline> {
        self.header_converting = Some(header.clone());
        let mut inlines = Vec::new();
        self.evaluate(&mut inlines, header);
        self.header_converting = None;
        self.resolve(&mut inlines);
        self.header_title = Some(inlines.clone());
        inlines
    }

    /// The converted document title; inside its own conversion, converted
    /// again (the reference being labeled makes inner ones display `[id]`).
    fn header_label(&mut self) -> Option<Vec<Inline>> {
        if self.header_title.is_some() {
            return self.header_title.clone();
        }
        if let Some(header) = self.header_lazy.take() {
            return Some(self.convert_header(header));
        }
        let pending = self.header_converting.clone()?;
        let mut nodes = Vec::new();
        self.evaluate(&mut nodes, pending);
        self.resolve(&mut nodes);
        Some(nodes)
    }

    /// Ruby's `apply_reftext_subs`, executed again at each reference.
    fn reftext(&mut self, read: crate::reftext::Read) -> Vec<Inline> {
        let compat = self.attributes.get("compat-mode").is_some();
        let mut evaluation =
            Evaluation::definition(&self.attributes, &self.locked, self.counters, compat)
                .with_root(self.root.as_mut().map(|(a, l)| (a, &**l)))
                .with_footnotes(&mut self.footnotes)
                .with_images(self.images);
        let nodes = read.finish(&mut evaluation, self.diagnostics);
        if let Some(attributes) = evaluation.changed_attributes() {
            self.attributes = attributes;
        }
        nodes
    }

    /// A block title converted once: the first reference converts a title
    /// still pending, with the state at that reference and without replay. A
    /// reference inside a title being converted converts it again, as Ruby's
    /// memo is assigned only once the outer conversion finishes.
    fn title(&mut self, nodes: &[Inline]) -> Vec<Inline> {
        let Some(key) = crate::resolve::placeholder(nodes) else {
            return nodes.to_vec();
        };
        if let Some(converted) = self.titles.get(&key) {
            return converted.clone();
        }
        match self.pending.remove(&key) {
            Some(pending) => self.convert(key, pending),
            None => match self.converting.get(&key).cloned() {
                Some(pending) => self.convert(key, pending),
                None => nodes.to_vec(),
            },
        }
    }

    /// A section title referenced before its heading resolves its own
    /// references now (inner labels display `[id]`), as Ruby memoizes it.
    fn heading(&mut self, key: (usize, usize), nodes: &[Inline]) -> Vec<Inline> {
        if let Some(resolved) = self.headings.get(&key) {
            return resolved.clone();
        }
        if crate::resolve::placeholder(nodes).is_some() {
            return self.lazy_heading(key, nodes);
        }
        let mut resolved = nodes.to_vec();
        self.resolve(&mut resolved);
        self.headings.insert(key, resolved.clone());
        resolved
    }

    /// A section title Ruby converts at its first use (it had no id to
    /// generate while reading), with the state at that use; later uses take
    /// the result.
    fn lazy_heading(&mut self, key: (usize, usize), nodes: &[Inline]) -> Vec<Inline> {
        if let Some(converted) = self.headings.get(&key) {
            return converted.clone();
        }
        let converted = self.title(nodes);
        self.headings.insert(key, converted.clone());
        converted
    }

    /// Ruby's `convert_outline`: every listed section's title, nested while
    /// a section's level is below `toclevels`.
    fn toc(&mut self, toclevels: i128) {
        fn walk(execution: &mut Execution<'_, '_>, sections: &[Outline], toclevels: i128) {
            for section in sections {
                if crate::resolve::placeholder(&section.title).is_some() {
                    execution.lazy_heading(section.heading, &section.title);
                }
                if i128::from(section.level) < toclevels {
                    walk(execution, &section.sections, toclevels);
                }
            }
        }
        let outline = std::mem::take(&mut self.outline);
        walk(self, &outline, toclevels);
        self.outline = outline;
    }

    /// Ruby's `image_uri` with the attributes at this point: `dir` names the
    /// asset directory attribute.
    fn image_uri(&mut self, target: &str, dir: &str, span: crate::Span) -> String {
        let dir = self.attributes.get(dir);
        match self.images {
            Some(images) => images.uri(target, dir, &self.attributes, span, self.diagnostics),
            None => crate::image::web_path(target, dir),
        }
    }

    /// Convert a deferred field. Its evaluation takes the conversion state,
    /// lending it back to the labels its cross-references build (see
    /// [`crate::substitution_context::Labels`]); diagnostics of titles
    /// converted for those labels follow the field's.
    fn evaluate(&mut self, nodes: &mut Vec<Inline>, pending: Pending) {
        self.attributes.local_hardbreaks = pending.hardbreaks;
        let compat = self.attributes.get("compat-mode").is_some();
        let attributes = std::mem::take(&mut self.attributes);
        let locked = self.locked.clone();
        let mut counters = std::mem::take(self.counters);
        let mut root = self
            .root
            .as_mut()
            .map(|(attributes, locked)| (std::mem::take(attributes), locked.clone()));
        let mut footnotes = std::mem::take(&mut self.footnotes);
        let mut diagnostics = std::mem::take(self.diagnostics);
        let (images, xrefs) = (self.images, self.xrefs);
        let mut start = self.start.take();
        let mut tokens = Vec::new();
        let changed = {
            let mut evaluation =
                Evaluation::definition(&attributes, &locked, &mut counters, compat)
                    .with_root(root.as_mut().map(|(a, l)| (a, &**l)))
                    .with_footnotes(&mut footnotes)
                    .with_images(images)
                    .with_xrefs(xrefs)
                    .with_labels(self);
            *nodes = run(
                pending,
                &mut evaluation,
                &mut diagnostics,
                &mut tokens,
                start.as_deref_mut(),
            );
            evaluation.changed_attributes()
        };
        self.start = start;
        self.attributes = changed.unwrap_or(attributes);
        self.tokens.extend(tokens);
        *self.counters = counters;
        if let (Some((current, _)), Some((attributes, _))) = (self.root.as_mut(), root) {
            *current = attributes;
        }
        self.footnotes = footnotes;
        diagnostics.append(self.diagnostics);
        *self.diagnostics = diagnostics;
    }

    /// Convert blocks in Ruby's order (a pre-order walk without recursion,
    /// see [`crate::visit`]).
    fn blocks(&mut self, blocks: &mut [Block]) {
        crate::visit::walk_blocks_mut(self, blocks);
    }
}

impl crate::visit::VisitorMut for Execution<'_, '_> {
    /// A block's conversion before its title and content.
    fn enter_block(&mut self, block: &mut Block) -> bool {
        if self.excluded.contains(&(block.span.start, block.span.end)) {
            return false;
        }
        self.replay(block.span.start);
        if let Some((context, parts)) = self
            .caption_counters
            .remove(&(block.span.start, block.span.end))
        {
            self.attributes
                .set_parts(format!("{context}-number"), parts);
        }
        block.conversion_context = Some(self.snapshot());
        // Ruby's `image_uri` and `media_uri` read `imagesdir` (and
        // `asset-uri-scheme`) from the document at conversion./`asset-uri-scheme`
        let span = block.span;
        if let BlockKind::Image {
            target, src, svg, ..
        } = &mut block.kind
        {
            // Ruby reads an inline SVG instead of resolving its source.
            *svg = crate::data_uri::Images::svg(
                self.images,
                target,
                &block.attrs.named,
                &block.attrs.options,
                &self.attributes,
                span,
                self.diagnostics,
            );
            *src = match svg {
                Some(crate::Svg::Inline(_)) => {
                    crate::image::web_path(target, self.attributes.get("imagesdir"))
                }
                _ => self.image_uri(target, "imagesdir", span),
            };
        }
        crate::media::resolve(block, &self.attributes);
        // Callout icons follow the `icons` mode where the block converts.
        let checklist = block.attrs.options.iter().any(|o| o == "checklist")
            || block.attrs.named.contains_key("checklist-option");
        match &mut block.kind {
            // Ruby's `convert_ulist` reads the document `icons` for a
            // checklist's marks.
            BlockKind::List {
                ordered: false,
                icons,
                ..
            } if checklist => {
                *icons = self.attributes.get("icons").map(str::to_owned);
            }
            BlockKind::Listing {
                callouts,
                inlines: None,
                ..
            }
            | BlockKind::Literal {
                callouts,
                inlines: None,
                ..
            } => {
                for callout in callouts {
                    let (icons, target) = crate::callouts::icon(&self.attributes, &callout.number);
                    callout.icons = icons;
                    callout.src = target.map_or_else(String::new, |target| {
                        self.image_uri(&target, "iconsdir", span)
                    });
                }
            }
            BlockKind::CalloutList { items } => {
                for (index, entry) in items.iter_mut().enumerate() {
                    let (icons, target) =
                        crate::callouts::icon(&self.attributes, &(index + 1).to_string());
                    entry.icons = icons;
                    entry.src = target.map_or_else(String::new, |target| {
                        self.image_uri(&target, "iconsdir", entry.item.span)
                    });
                }
            }
            // Ruby's `convert_admonition`: a font glyph unless the block
            // names an `icon`, else `icon_uri` under the document's
            // `iconsdir` (a block `iconsdir` is ignored).
            BlockKind::Admonition {
                kind, icons, src, ..
            } => {
                *icons = self.attributes.get("icons").map(str::to_owned);
                let icon = block.attrs.named.get("icon").map(String::as_str);
                *src = match icons.as_deref() {
                    None => String::new(),
                    Some("font") if icon.is_none() => String::new(),
                    Some(_) => {
                        let target = crate::image::icon_target(
                            icon,
                            kind.as_str(),
                            self.attributes.get("icontype"),
                        );
                        self.image_uri(&target, "iconsdir", span)
                    }
                };
            }
            _ => {}
        }
        if let Some(title) = &mut block.title {
            self.inlines(title);
        }
        // A table converts its cells itself.
        if let BlockKind::Table(table) = &mut block.kind {
            self.table(table, &block.attrs, block.span);
            return false;
        }
        // A section number uses the attributes at its title, like Ruby's
        // `convert_section` (a heading has no block title).
        if let (BlockKind::Heading { .. }, Some(section)) = (
            &block.kind,
            self.sections.get(&(block.span.start, block.span.end)),
        ) {
            block.set_caption(section.caption(&self.attributes));
        }
        match &mut block.kind {
            // Section titles were converted while reading; a forward
            // reference may already have resolved their references. A
            // title converted at its first use converts here unless a TOC
            // or reference did (a repeated table cell converts it again).
            BlockKind::Heading { inlines, .. } => {
                let key = (block.span.start, block.span.end);
                match self.headings.get(&key) {
                    Some(resolved) => *inlines = resolved.clone(),
                    None if crate::resolve::placeholder(inlines).is_some() => {
                        *inlines = self.title(inlines);
                        if self.root.is_none() {
                            self.headings.insert(key, inlines.clone());
                        }
                    }
                    None => {
                        self.resolve(inlines);
                        self.headings.insert(key, inlines.clone());
                    }
                }
                self.heading_converted = true;
            }
            // Ruby's `convert_preamble` converts its TOC before the content.
            BlockKind::Preamble { .. } => {
                if let Some(levels) = self.preamble_toc {
                    self.toc(levels);
                }
            }
            // Ruby's `convert_toc` with the attributes at the macro.
            BlockKind::Toc
                if !self.outline.is_empty()
                    && self.attributes.get("toc").is_some()
                    && self.attributes.get("toc-placement") == Some("macro") =>
            {
                let levels = match block.attrs.named.get("levels") {
                    Some(levels) => toclevels(Some(levels)),
                    None => toclevels(self.attributes.get("toclevels")),
                };
                self.toc(levels);
            }
            _ => {}
        }
        true
    }

    /// A block's own text: paragraphs, verbatim content
    /// with substitutions, list item text, terms and descriptions (Ruby
    /// converts each term, then the description's text and blocks). Reftext
    /// and quote attributions were substituted while reading.
    fn visit_inlines(&mut self, inlines: &mut Vec<Inline>, context: crate::visit::InlineContext) {
        use crate::visit::InlineContext;
        match context {
            InlineContext::Body if std::mem::take(&mut self.heading_converted) => {}
            InlineContext::Body => self.inlines(inlines),
            // The title converted on entry; reftext and attributions were
            // substituted while reading.
            InlineContext::Title | InlineContext::Reftext | InlineContext::Attribution => {}
        }
    }
}

impl Execution<'_, '_> {
    /// Ruby's `convert_table`: the frame, grid and stripes with their
    /// document fallbacks, then every cell in head, body, foot order, each
    /// reading `cellbgcolor` after its content. A repeated cell (`3*|`)
    /// shares its source and converts again for every copy.
    fn table(&mut self, table: &mut crate::ast::Table, attrs: &crate::ast::BlockAttrs, span: Span) {
        let value = |name: &str, fallback: &str| {
            attrs
                .named
                .get(name)
                .map(String::as_str)
                .or(self.attributes.get(fallback))
                .map(str::to_owned)
        };
        let frame = value("frame", "table-frame").unwrap_or_else(|| "all".into());
        table.frame = if frame == "topbot" {
            "ends".into()
        } else {
            frame
        };
        table.grid = value("grid", "table-grid").unwrap_or_else(|| "all".into());
        table.stripes = value("stripes", "table-stripes");
        let head = table.head.iter().map(Vec::len).sum::<usize>();
        for (index, cell) in table.cells_mut().enumerate() {
            // A repeated cell shares its source: keep its deferred fields for
            // the next copy.
            let range = (cell.span.start, 0)..=(cell.span.end, usize::MAX);
            let saved: Vec<_> = self
                .pending
                .range(range)
                .map(|(key, pending)| (*key, pending.clone()))
                .collect();
            match &mut cell.content {
                crate::ast::CellContent::Paragraphs(paragraphs) => {
                    for nodes in paragraphs.iter_mut() {
                        self.inlines(nodes);
                    }
                    // Ruby splits the converted body text, which drops
                    // trailing empty paragraphs (all of them when empty).
                    if index >= head {
                        while paragraphs.last().is_some_and(|nodes| blank(nodes)) {
                            paragraphs.pop();
                        }
                    }
                }
                // The nested document converts from its own header snapshot
                // and entries; its effects stay inside it.
                crate::ast::CellContent::Blocks {
                    blocks,
                    has_title,
                    footnotes: catalog,
                    ..
                } => {
                    let key = (cell.span.start, cell.span.end);
                    if let Some(nested) = self.nested.get(&key).cloned() {
                        let mut attributes =
                            Some(std::mem::replace(&mut self.attributes, nested.attributes));
                        // The nested document's `content` deletes it too.
                        self.attributes.remove("title");
                        let entries = std::mem::replace(
                            &mut self.entries,
                            nested.entries.into_iter().peekable(),
                        );
                        let locked = std::mem::replace(
                            &mut self.locked,
                            std::borrow::Cow::Owned(nested.locked),
                        );
                        let footnotes = std::mem::replace(&mut self.footnotes, nested.footnotes);
                        let root = self.root.is_none();
                        if root {
                            self.root = Some((attributes.take().unwrap(), locked.clone()));
                        }
                        // Ruby's `convert_embedded`: the title, then an
                        // automatic TOC with the header attributes, then the
                        // body; the outline is the inner document's.
                        let title = usize::from(*has_title);
                        let outline =
                            std::mem::replace(&mut self.outline, Outline::of(&blocks[title..]));
                        let preamble_toc = self.preamble_toc.take();
                        let header = self.attributes.clone();
                        self.blocks(&mut blocks[..title]);
                        if !self.outline.is_empty() && header.get("toc").is_some() {
                            let levels = toclevels(header.get("toclevels"));
                            match header.get("toc-placement") {
                                Some("preamble") => self.preamble_toc = Some(levels),
                                Some("macro") => {}
                                _ => self.toc(levels),
                            }
                        }
                        self.blocks(&mut blocks[title..]);
                        self.outline = outline;
                        self.preamble_toc = preamble_toc;
                        self.attributes = match attributes {
                            Some(attributes) => attributes,
                            None => self.root.take().unwrap().0,
                        };
                        self.entries = entries;
                        self.locked = locked;
                        *catalog = std::mem::replace(&mut self.footnotes, footnotes)
                            .into_definitions(&self.fixed);
                    }
                }
                crate::ast::CellContent::Literal(_) => {}
            }
            self.pending.extend(saved);
            cell.background = self.attributes.get("cellbgcolor").map(str::to_owned);
        }
        let keys: Vec<_> = self
            .pending
            .range((span.start, 0)..=(span.end, usize::MAX))
            .map(|(key, _)| *key)
            .collect();
        for key in keys {
            self.pending.remove(&key);
        }
    }
}

/// Converted inlines without output.
fn blank(nodes: &[Inline]) -> bool {
    nodes.iter().all(|node| match node {
        Inline::Text { value, .. } | Inline::Raw { value, .. } => value.is_empty(),
        Inline::Substituted { value, parts, .. } => {
            value.is_empty()
                && parts
                    .iter()
                    .all(|p| matches!(p, AttributePart::Text(t) if t.is_empty()))
        }
        Inline::AttributeRef { value, parts, .. } => {
            value.as_deref() == Some("")
                && parts
                    .iter()
                    .all(|p| matches!(p, AttributePart::Text(t) if t.is_empty()))
        }
        _ => false,
    })
}

/// Scan a deferred field with `evaluation` and map nodes and diagnostics back
/// to their source.
fn run(
    pending: Pending,
    evaluation: &mut Evaluation<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    tokens: &mut Vec<crate::Token>,
    start: Option<&mut (dyn crate::ConversionStart + '_)>,
) -> Vec<Inline> {
    let first_footnote = evaluation.footnotes().map(|registry| registry.len());
    evaluation.bibliography = pending.bibliography;
    let source = pending.filtered.as_ref().unwrap_or(&pending.source);
    let before = diagnostics.len();
    let mut syntax = Vec::new();
    let mut nodes = if let Some(block) = &pending.block {
        // Selected substitutions return semantic parts rather than source AST
        // nodes. Scan their complete source field without replaying effects,
        // retaining only syntax belonging to enabled stages.
        syntax = crate::inline::scan_for_tokens(&source.text, 0, evaluation);
        syntax.retain(|token| {
            crate::tokenizer::inline_stage(token.kind)
                .is_some_and(|stage| block.stages.contains(&stage))
        });
        let text = source.text.replace("\r\n", "\n");
        let span = Span::new(0, source.text.len());
        // Ruby's `highlight` substitution, through the converter's hook.
        let mut hook = match (start, &block.source_listing) {
            (Some(start), Some((named, options))) => {
                Some(move |source: &str, attributes: &crate::Attributes| {
                    start.highlight(&crate::HighlightRequest {
                        source,
                        named,
                        options,
                        attributes,
                    })
                })
            }
            _ => None,
        };
        let compat = evaluation.get("compat-mode").is_some();
        let frame = crate::inline_pass::SelectedBlock {
            line_comment: block.line_comment.as_deref(),
            highlight: hook
                .as_mut()
                .map(|hook| hook as crate::highlight_stage::Hook<'_>),
        };
        let mut parts = crate::inline_pass::Passes::selected_stages(
            &text,
            &block.stages,
            frame,
            evaluation,
            compat,
            span,
            diagnostics,
        );
        let mut ids = block.callout_ids.iter();
        for part in &mut parts {
            if let AttributePart::Callout(callout) = part {
                callout.id = ids.next().cloned();
            }
        }
        // Verbatim and raw blocks trim empty edge lines *after* substitutions:
        // attribute expansion may create or remove those lines. Preserve
        // semantic parts.
        if block.trim {
            parts = trim_edges(parts);
        }
        vec![Inline::Substituted {
            value: crate::attributes::parts_text(&parts),
            parts,
            span,
        }]
    } else {
        crate::inline::scan_inlines_recorded(
            &source.text,
            0,
            evaluation,
            pending.fold,
            diagnostics,
            Some(&mut syntax),
        )
    };
    if let Some(filtered) = &pending.filtered {
        filtered.restore_inlines(&mut nodes);
        for diagnostic in &mut diagnostics[before..] {
            diagnostic.span = filtered.original_span(diagnostic.span);
        }
    }
    pending.source.restore_inlines(&mut nodes);
    // Replacement text is generated, not syntax at its declaration's offsets.
    if pending.replacement_origin.is_none() {
        tokens.extend(syntax.into_iter().map(|mut token| {
            if let Some(filtered) = &pending.filtered {
                token.span = filtered.original_span(token.span);
            }
            token.span = pending.source.original_span(token.span);
            token
        }));
    }
    for diagnostic in &mut diagnostics[before..] {
        diagnostic.span = pending.source.original_span(diagnostic.span);
    }
    if let Some(origin) = pending.replacement_origin {
        struct ReplacementOrigin(Span);
        impl crate::visit::VisitorMut for ReplacementOrigin {
            fn visit_span(&mut self, span: &mut Span) {
                *span = self.0;
            }
        }
        crate::visit::walk_inlines_mut(&mut ReplacementOrigin(origin), &mut nodes);
        for diagnostic in &mut diagnostics[before..] {
            diagnostic.span = origin;
        }
    }
    if let (Some(first), Some(registry)) = (first_footnote, evaluation.footnotes()) {
        registry.restore_origins(&mut nodes, first);
    }
    nodes
}

fn trim_edges(parts: Vec<AttributePart>) -> Vec<AttributePart> {
    let text = crate::substitution_text::Text::from_parts(&parts);
    if !text.text.contains('\n') {
        return parts;
    }
    let lines = crate::scanner::lines_with_offsets(&text.text);
    let nonempty = |line: &str| {
        !line
            .trim_matches([' ', '\t', '\r', '\x0b', '\x0c'])
            .is_empty()
    };
    let start = lines
        .iter()
        .find(|(_, line)| nonempty(line))
        .map_or(text.text.len(), |(at, _)| *at);
    let end = lines
        .iter()
        .rfind(|(_, line)| nonempty(line))
        .map_or(start, |(at, line)| at + line.len());
    let mut trimmed = crate::substitution_text::Text::default();
    text.copy(start..end, &mut trimmed);
    trimmed.into_parts()
}

fn referable_titles(registry: &crate::resolve::Registry) -> BTreeSet<(usize, usize)> {
    registry
        .values()
        .filter_map(|target| match &target.title {
            Some(crate::resolve::Title::Nodes(nodes)) => crate::resolve::placeholder(nodes),
            _ => None,
        })
        .collect()
}
