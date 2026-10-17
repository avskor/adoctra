//! Conversion-time inline evaluation. Reading records source and evaluated
//! declarations; the displayed document title and surviving body fields execute
//! after the reader finishes. Section titles and eligible anchored block titles
//! have already been cached. Captions replay their reading-time counters at entry.
//! Cross-references resolve here too, after each field's substitutions, so a
//! label sees the conversion state at that reference (see resolve.rs).
use crate::scanner::FilteredSource;
use crate::substitution_context::Evaluation;
use crate::{AttributePart, Attributes, Block, BlockKind, Diagnostic, Inline, Span};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct Pending {
    source: FilteredSource,
    filtered: Option<FilteredSource>,
    fold: bool,
    hardbreaks: bool,
    replacement_origin: Option<Span>,
}

type Declaration = (usize, String, Option<Vec<AttributePart>>);

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
}

#[derive(Default)]
pub(crate) struct BodySubs {
    header: Option<Pending>,
    pending: BTreeMap<(usize, usize), Pending>,
    pub entries: Vec<Declaration>,
    pub caption_counters: BTreeMap<(usize, usize), (&'static str, Vec<AttributePart>)>,
}

impl BodySubs {
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
                source,
                filtered,
                fold,
                hardbreaks: attrs.local_hardbreaks,
                replacement_origin: None,
            },
        );
        // Private placeholder; replaced before source mapping or public output.
        vec![Inline::Text {
            value: String::new(),
            span,
        }]
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
            source,
            filtered: None,
            fold: true,
            hardbreaks: attrs.local_hardbreaks,
            replacement_origin: title.replacement.then_some(title.span),
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
        Some(run(pending, &mut evaluation, diagnostics))
    }

    pub fn execute(mut self, blocks: &mut [Block], attributes: Attributes, conversion: Conversion) {
        let mut execution = Execution {
            pending: &mut self.pending,
            caption_counters: &mut self.caption_counters,
            entries: self.entries.into_iter().peekable(),
            attributes,
            locked: conversion.locked,
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
            first_section: None,
            resolving: false,
        };
        let mut body = 0;
        if let (Some(header), Some(block)) = (self.header, blocks.first_mut()) {
            if let BlockKind::Heading { inlines, .. } = &mut block.kind {
                execution.evaluate(inlines, header);
                execution.resolve(inlines);
                execution.header_title = Some(inlines.clone());
                body = 1;
            }
        }
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
    }
}

struct Execution<'a> {
    pending: &'a mut BTreeMap<(usize, usize), Pending>,
    caption_counters: &'a mut BTreeMap<(usize, usize), (&'static str, Vec<AttributePart>)>,
    entries: std::iter::Peekable<std::vec::IntoIter<Declaration>>,
    attributes: Attributes,
    locked: &'a BTreeSet<String>,
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
    /// Without a document title, Ruby's `doctitle` is the first top-level
    /// section's title (keyed by its heading range).
    first_section: Option<((usize, usize), Vec<Inline>)>,
    /// Ruby's `@resolving_xref`: references inside a label being produced
    /// display `[id]`.
    resolving: bool,
}

impl Execution<'_> {
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
                    self.target(xref, false, *span);
                    if children.is_empty() {
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
                Inline::AttributeRef { span, .. } | Inline::Substituted { span, .. } => {
                    let span = *span;
                    crate::resolve::fill_labels(node, |xref, textless| {
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
        if let Some(id) = crate::resolve::decoded_refid(xref, lexical) {
            if !self.registry.contains_key(&id) {
                self.references.push(Diagnostic {
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
            return vec![Inline::Text {
                value: crate::resolve::decoded(path, lexical),
                span,
            }];
        }
        match crate::resolve::decoded_refid(xref, lexical) {
            Some(id) if !id.is_empty() || self.registry.contains_key(&id) => {
                // Ruby's `node.attr 'xrefstyle', nil, true`.
                let style = match &xref.xrefstyle {
                    Some(style) => Some(crate::resolve::decoded(style, lexical)),
                    None => self.attributes.get("xrefstyle").map(str::to_owned),
                };
                self.label(&id, style.as_deref(), span)
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
        if self.header_title.is_some() {
            return self.header_title.clone();
        }
        let (key, nodes) = self.first_section.clone()?;
        Some(self.heading(key, &nodes))
    }

    /// Ruby's `xreftext`: a nonempty reftext, else the (styled) title, else
    /// `[id]`. References inside a label being produced display `[id]`.
    fn label(&mut self, id: &str, style: Option<&str>, span: Span) -> Vec<Inline> {
        let fallback = || {
            vec![Inline::Text {
                value: format!("[{id}]"),
                span,
            }]
        };
        if !self.registry.contains_key(id) || self.resolving {
            return fallback();
        }
        self.resolving = true;
        let label = self.xreftext(id, style, span);
        self.resolving = false;
        let Some(mut label) = label else {
            return fallback();
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
            Some(crate::resolve::Title::Header) => return self.header_title.clone(),
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

    /// Ruby's `apply_reftext_subs`, executed again at each reference.
    fn reftext(&mut self, read: crate::reftext::Read) -> Vec<Inline> {
        let compat = self.attributes.get("compat-mode").is_some();
        let mut evaluation =
            Evaluation::definition(&self.attributes, self.locked, self.counters, compat);
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
        let mut resolved = nodes.to_vec();
        self.resolve(&mut resolved);
        self.headings.insert(key, resolved.clone());
        resolved
    }

    fn evaluate(&mut self, nodes: &mut Vec<Inline>, pending: Pending) {
        self.attributes.local_hardbreaks = pending.hardbreaks;
        let compat = self.attributes.get("compat-mode").is_some();
        let mut evaluation =
            Evaluation::definition(&self.attributes, self.locked, self.counters, compat)
                .with_xrefs(self.xrefs);
        *nodes = run(pending, &mut evaluation, self.diagnostics);
        if let Some(attributes) = evaluation.changed_attributes() {
            self.attributes = attributes;
        }
    }

    fn blocks(&mut self, blocks: &mut [Block]) {
        for block in blocks {
            self.replay(block.span.start);
            if let Some((context, parts)) = self
                .caption_counters
                .remove(&(block.span.start, block.span.end))
            {
                self.attributes
                    .set_parts(format!("{context}-number"), parts);
            }
            // Ruby's `image_uri` reads `imagesdir` from the document at
            // conversion.
            if let BlockKind::Image { target, src, .. } = &mut block.kind {
                *src = crate::image::web_path(target, self.attributes.get("imagesdir"));
            }
            if let Some(title) = &mut block.title {
                self.inlines(title);
            }
            // A section number uses the attributes at its title, like Ruby's
            // `convert_section`.
            if let (BlockKind::Heading { .. }, Some(section)) = (
                &block.kind,
                self.sections.get(&(block.span.start, block.span.end)),
            ) {
                block.caption = section.caption(&self.attributes);
            }
            match &mut block.kind {
                BlockKind::Paragraph { inlines } | BlockKind::Verse { inlines, .. } => {
                    self.inlines(inlines)
                }
                // Section titles were converted while reading; a forward
                // reference may already have resolved their references.
                BlockKind::Heading { inlines, .. } => {
                    let key = (block.span.start, block.span.end);
                    match self.headings.get(&key) {
                        Some(resolved) => *inlines = resolved.clone(),
                        None => {
                            self.resolve(inlines);
                            self.headings.insert(key, inlines.clone());
                        }
                    }
                }
                BlockKind::List { items, .. } => {
                    for item in items {
                        self.inlines(&mut item.inlines);
                        self.blocks(&mut item.blocks);
                    }
                }
                BlockKind::Admonition { blocks, .. }
                | BlockKind::Example { blocks }
                | BlockKind::Quote { blocks, .. }
                | BlockKind::Sidebar { blocks }
                | BlockKind::Open { blocks }
                | BlockKind::Section { blocks, .. }
                | BlockKind::Preamble { blocks } => self.blocks(blocks),
                _ => {}
            }
        }
    }
}

/// Scan a deferred field with `evaluation` and map nodes and diagnostics back
/// to their source.
fn run(
    pending: Pending,
    evaluation: &mut Evaluation<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Inline> {
    let source = pending.filtered.as_ref().unwrap_or(&pending.source);
    let before = diagnostics.len();
    let mut nodes = crate::inline::scan_inlines_evaluated(
        &source.text,
        0,
        evaluation,
        pending.fold,
        diagnostics,
    );
    if let Some(filtered) = &pending.filtered {
        filtered.restore_inlines(&mut nodes);
        for diagnostic in &mut diagnostics[before..] {
            diagnostic.span = filtered.original_span(diagnostic.span);
        }
    }
    pending.source.restore_inlines(&mut nodes);
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
    nodes
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
