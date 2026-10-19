//! Reference registry: assign section ids and catalog every referable id.
//!
//! This is the first pass that needs the *whole* document at once: a `<<id>>`
//! may point forward to a section defined later, so the registry must be
//! complete before any reference resolves. It runs after reading and before
//! conversion. Reference labels are produced during conversion
//! (body_subs.rs), in document order and with the conversion state, like Ruby's
//! `xreftext`: an explicit reftext is formatted again at each reference;
//! otherwise a block's (lazily converted) title is used; otherwise `[id]`.
//! An unknown id emits a diagnostic and also renders `[id]`.

use std::collections::{btree_map::Entry, BTreeMap};

use crate::ast::{Block, BlockKind, Document, Inline, Span};
use crate::diagnostic::{Diagnostic, Severity};
use crate::visit::{self, InlineContext, VisitorMut};

/// What a *textless* reference to an id can display. Ordered so the structure
/// stays deterministic (like [`Attributes`]).
pub(crate) type Registry = BTreeMap<String, Target>;

/// A referable node: its explicit reftext (formatted per reference) and, for a
/// block, its title. Neither ⇒ a known id that displays `[id]` *without* a
/// diagnostic (e.g. a plain paragraph anchor).
pub(crate) struct Target {
    pub reftext: Option<crate::reftext::Read>,
    pub title: Option<Title>,
    /// Registration order, which Ruby's `resolve_id` follows.
    pub order: usize,
    /// What an `xrefstyle` can show besides the title.
    pub numbering: Numbering,
}

/// Ruby's `xreftext` inputs for an xrefstyle. Example blocks and images carry
/// a caption (and its `example-caption`/`figure-caption` signifier, by the
/// caption `context`); a section has its number
/// and `sectname` (unnumbered chapters and appendices are emphasized by any
/// style).
#[derive(Default)]
pub(crate) enum Numbering {
    #[default]
    None,
    Caption {
        /// The caption's parts (Ruby's stored `@caption` markup).
        caption: Vec<crate::AttributePart>,
        numeral: Option<String>,
        context: &'static str,
    },
    Section(crate::section_numbers::Section),
}

/// Apply an xrefstyle to a converted title, like `AbstractBlock#xreftext`
/// and `Section#xreftext` without numbered sections.
pub(crate) fn styled(
    title: Vec<Inline>,
    numbering: &Numbering,
    style: Option<&str>,
    attributes: &crate::Attributes,
    span: Span,
) -> Vec<Inline> {
    let Some(style) = style else {
        return title;
    };
    let text = |value: String| Inline::Text { value, span };
    let (caption, numeral, context) = match numbering {
        Numbering::Section(section) => {
            return styled_section(title, section, style, attributes, span)
        }
        Numbering::Caption {
            caption,
            numeral,
            context,
        } if !crate::attributes::parts_text(caption).is_empty() => (caption, numeral, context),
        _ => return title,
    };
    let signifier = match (
        numeral,
        attributes.value_parts(&format!("{context}-caption")),
    ) {
        (Some(numeral), Some(parts)) => {
            vec![signifier(parts, span), text(format!(" {numeral}"))]
        }
        // Ruby's `@caption.chomp '. '`.
        _ => {
            let mut caption = caption.clone();
            if let Some(crate::AttributePart::Text(last) | crate::AttributePart::Raw(last)) =
                caption.last_mut()
            {
                if let Some(chomped) = last.strip_suffix(". ") {
                    *last = chomped.to_owned();
                }
            }
            vec![signifier(caption, span)]
        }
    };
    match style {
        "full" => {
            let mut label = signifier;
            label.push(text(", \u{201c}".into()));
            label.extend(title);
            label.push(text("\u{201d}".into()));
            label
        }
        "short" => signifier,
        _ => title,
    }
}

/// `Section#xreftext` with an xrefstyle: `full` and `short` show a numbered
/// section's `<sectname>-refsig` and number (`full` also the quoted title);
/// otherwise chapters and appendices are emphasized.
fn styled_section(
    title: Vec<Inline>,
    section: &crate::section_numbers::Section,
    style: &str,
    attributes: &crate::Attributes,
    span: Span,
) -> Vec<Inline> {
    let text = |value: String| Inline::Text { value, span };
    let emphasized = |title| {
        if section.emphasized() {
            vec![Inline::Emphasis {
                children: title,
                span,
            }]
        } else {
            title
        }
    };
    let (append, quoted) = match style {
        _ if !section.is_numbered() => return emphasized(title),
        "full" if section.emphasized() => (",", emphasized(title)),
        "full" => {
            let mut quoted = vec![text("\u{201c}".into())];
            quoted.extend(title);
            quoted.push(text("\u{201d}".into()));
            (",", quoted)
        }
        "short" => ("", Vec::new()),
        _ => return emphasized(title),
    };
    let mut label = Vec::new();
    if let Some(parts) = attributes.value_parts(&format!("{}-refsig", section.sectname)) {
        label.push(signifier(parts, span));
        label.push(text(" ".into()));
    }
    label.push(text(section.sectnum(".", append)));
    if !quoted.is_empty() {
        label.push(text(" ".into()));
        label.extend(quoted);
    }
    label
}

/// An attribute value as label text, keeping generated boundaries.
fn signifier(parts: Vec<crate::AttributePart>, span: Span) -> Inline {
    let value = crate::attributes::parts_text(&parts);
    if parts
        .iter()
        .all(|p| matches!(p, crate::AttributePart::Text(_)))
    {
        Inline::Text { value, span }
    } else {
        Inline::Substituted { value, parts, span }
    }
}

/// Registered ids in registration order.
pub(crate) fn ordered(registry: &Registry) -> Vec<&str> {
    let mut ids: Vec<_> = registry.iter().collect();
    ids.sort_by_key(|(_, target)| target.order);
    ids.into_iter().map(|(id, _)| id.as_str()).collect()
}

/// A cross-reference generated by an attribute definition (its `pass:m`
/// stage). Ruby converts it while defining the attribute, so its natural
/// lookup and label use the ids registered before the definition and the
/// attributes at that point; every use of the value shows that result.
pub(crate) struct Definition {
    pub span: Span,
    pub attributes: std::sync::Arc<crate::Attributes>,
    pub xref: crate::Xref,
    pub textless: bool,
}

/// A definition's reference resolved at the definition: its target and, when
/// textless, its label (`None` leaves the label to the reference, e.g. for
/// the current document).
pub(crate) struct Fixed {
    xref: crate::Xref,
    label: Option<Vec<Inline>>,
}

/// Fixed definition references, indexed by `Xref::definition`.
pub(crate) type FixedXrefs = Vec<Option<Fixed>>;

/// What registration hands to conversion.
pub(crate) type Registered = (Registry, EarlyTitles, FixedXrefs);

/// Restore a definition reference's fixed target and return its fixed label.
pub(crate) fn apply_fixed(fixed: &FixedXrefs, xref: &mut crate::Xref) -> Option<Vec<Inline>> {
    let resolved = fixed.get(xref.definition?)?.as_ref()?;
    *xref = crate::Xref {
        definition: None,
        ..resolved.xref.clone()
    };
    resolved.label.clone()
}

/// Converts a deferred block title while reading (BodySubs::convert_early).
pub(crate) type EarlyTitle<'a> =
    dyn FnMut((usize, usize), &crate::Attributes) -> Option<Vec<Inline>> + 'a;

/// Deferred block titles converted while reading, keyed by placeholder range.
pub(crate) type EarlyTitles = BTreeMap<(usize, usize), Vec<Inline>>;

pub(crate) enum Title {
    /// A block title: final when converted while reading, otherwise a
    /// conversion placeholder that the first reference may convert.
    Nodes(Vec<Inline>),
    /// A section title, keyed by the heading range; conversion memoizes it
    /// once its references are resolved.
    Heading((usize, usize), Vec<Inline>),
    /// The displayed document title, converted before the body.
    Header,
}

/// What reading recorded for registration, keyed by reader ranges.
pub(crate) struct Read<'a> {
    /// Inline anchors cataloged while reading.
    pub anchors: Vec<(String, crate::anchor_catalog::Entry)>,
    /// Early-expanded block reftext.
    pub reftexts: BTreeMap<(usize, usize), crate::reftext::Read>,
    /// Caption numerals of numbered blocks.
    pub numerals: BTreeMap<(usize, usize), String>,
    /// Blockquotes whose credit line splits after its labels resolve.
    pub credits: std::collections::BTreeSet<(usize, usize)>,
    /// Section titles and their numbers.
    pub sections: &'a BTreeMap<(usize, usize), crate::section_numbers::Section>,
    /// References generated by attribute definitions.
    pub definitions: Vec<Definition>,
    pub nested_titles: std::collections::BTreeSet<(usize, usize)>,
}

/// Assign section ids and register every block id and cataloged inline
/// anchor, pushing a diagnostic for a duplicate id (the first one wins).
pub(crate) fn register(
    doc: &mut Document,
    read: Read<'_>,
    early_title: &mut EarlyTitle<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Registered {
    let Read {
        mut anchors,
        mut reftexts,
        mut numerals,
        credits,
        sections,
        definitions,
        nested_titles,
    } = read;
    // Every heading except the document title gets an automatic id, including
    // discrete and level-0 headings in the body, as in asciidoctor.
    let doctitle = doc
        .blocks
        .first()
        .filter(|_| doc.has_title)
        .map(|block| block.span);
    // Inline anchors join in source order, between the blocks around them.
    anchors.sort_by_key(|(_, entry)| entry.span.start);
    let fixed = (0..definitions.len()).map(|_| None).collect();
    let mut ids = Ids {
        doctitle,
        nested_titles,
        anchors: anchors.into_iter().peekable(),
        definitions: definitions.into_iter().peekable(),
        fixed,
        position: 0,
        reftexts: &mut reftexts,
        numerals: &mut numerals,
        credits,
        sections,
        early_title,
        early: EarlyTitles::new(),
        registry: Registry::new(),
        diagnostics,
    };
    // A manual page's NAME section registers in the header, after the
    // document title; Ruby converts its title there, whatever its id.
    let title = usize::from(doc.has_title).min(doc.blocks.len());
    let (head, body) = doc.blocks.split_at_mut(title);
    visit::walk_blocks_mut(&mut ids, head);
    if let Some(heading) = doc
        .manpage
        .as_mut()
        .and_then(|manpage| manpage.name_section.as_mut())
        .map(|section| &mut section.heading)
    {
        let attributes = std::sync::Arc::clone(&heading.attribute_context);
        if let (Some(_), BlockKind::Heading { inlines, .. }) = (&heading.id, &mut heading.kind) {
            ids.resolve_reading(inlines, &attributes);
        }
        ids.visit_block(heading);
    }
    visit::walk_blocks_mut(&mut ids, body);
    ids.flush(usize::MAX);
    (ids.registry, ids.early, ids.fixed)
}

struct Ids<'a> {
    doctitle: Option<Span>,
    /// Nested documents' title headings (no automatic id).
    nested_titles: std::collections::BTreeSet<(usize, usize)>,
    anchors: std::iter::Peekable<std::vec::IntoIter<(String, crate::anchor_catalog::Entry)>>,
    /// Definition references in reading order.
    definitions: std::iter::Peekable<std::vec::IntoIter<Definition>>,
    fixed: FixedXrefs,
    /// The reading position of the field whose labels are being produced.
    position: usize,
    reftexts: &'a mut BTreeMap<(usize, usize), crate::reftext::Read>,
    numerals: &'a mut BTreeMap<(usize, usize), String>,
    credits: std::collections::BTreeSet<(usize, usize)>,
    sections: &'a BTreeMap<(usize, usize), crate::section_numbers::Section>,
    early_title: &'a mut EarlyTitle<'a>,
    early: EarlyTitles,
    registry: Registry,
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl Ids<'_> {
    /// Register cataloged anchors and resolve definition references that
    /// start before `position`, in source order.
    fn flush(&mut self, position: usize) {
        loop {
            let anchor = self.anchors.peek().map(|(_, entry)| entry.span.start);
            let definition = self.definitions.peek().map(|d| d.span.start);
            match (anchor, definition) {
                (Some(a), d) if a < position && d.map_or(true, |d| a <= d) => self.anchor(),
                (_, Some(d)) if d < position => self.define(),
                _ => break,
            }
        }
    }

    /// Fix a definition reference with the partial registry, as Ruby's
    /// converter does while the attribute is being defined.
    fn define(&mut self) {
        let Definition {
            span,
            attributes,
            mut xref,
            textless,
        } = self.definitions.next().unwrap();
        self.position = span.start;
        self.reading_target(&mut xref, true, &attributes);
        let label = textless
            .then(|| self.reading_xref_label(&xref, true, span, &attributes))
            .flatten();
        if let Some(slot) = xref.definition.and_then(|i| self.fixed.get_mut(i)) {
            *slot = Some(Fixed { xref, label });
        }
    }

    fn anchor(&mut self) {
        let (id, entry) = self.anchors.next().unwrap();
        if entry.bibliography && self.registry.contains_key(&id) {
            self.diagnostics.push(Diagnostic {
                span: entry.span,
                severity: Severity::Warning,
                message: format!("id assigned to bibliography anchor already in use: {id}"),
            });
            return;
        }
        let target = Target {
            reftext: entry.reftext,
            title: None,
            order: self.registry.len(),
            numbering: Numbering::None,
        };
        register_id(
            &mut self.registry,
            id,
            target,
            "anchor",
            entry.span,
            self.diagnostics,
        );
    }
}

impl Ids<'_> {
    /// Ruby converts an automatic-id heading and an anchored block title with
    /// a literal `{` while parsing, before the block registers itself: a
    /// reference to an id not yet registered (forward or self) displays
    /// `[id]`; an earlier target lends its reftext or converted title. A
    /// reference to a title deferred to conversion resolves there.
    fn resolve_reading(&mut self, inlines: &mut [Inline], attributes: &crate::Attributes) {
        for inline in inlines {
            match inline {
                Inline::CrossReference {
                    xref,
                    children,
                    span,
                } => {
                    self.reading_target(xref, false, attributes);
                    if children.is_empty() {
                        if let Some(mut label) =
                            self.reading_xref_label(xref, false, *span, attributes)
                        {
                            strip_label(&mut label);
                            *children = label;
                        }
                    } else {
                        self.resolve_reading(children, attributes)
                    }
                }
                Inline::Strong { children, .. }
                | Inline::Emphasis { children, .. }
                | Inline::Monospace { children, .. }
                | Inline::Mark { children, .. }
                | Inline::Superscript { children, .. }
                | Inline::Subscript { children, .. }
                | Inline::Link { children, .. } => self.resolve_reading(children, attributes),
                Inline::AttributeRef { span, .. }
                | Inline::Substituted { span, .. }
                | Inline::Footnote { span, .. } => {
                    let span = *span;
                    fill_labels(inline, |xref, textless| {
                        if let Some(label) = apply_fixed(&self.fixed, xref) {
                            return Some(label);
                        }
                        self.reading_target(xref, true, attributes);
                        textless
                            .then(|| self.reading_xref_label(xref, true, span, attributes))
                            .flatten()
                    });
                }
                _ => {}
            }
        }
    }

    /// A natural reference while reading looks its text up among the ids
    /// registered so far, without caching (Ruby's `resolve_id` before
    /// `@parsed`): the first whose label spells it wins.
    fn reading_target(
        &mut self,
        xref: &mut crate::Xref,
        lexical: bool,
        attributes: &crate::Attributes,
    ) {
        let Some(fragment) = natural_text(xref, &self.registry, lexical) else {
            return;
        };
        for id in ordered(&self.registry)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
        {
            let label = self.reading_xreftext(&id, None, Span::new(0, 0), attributes);
            if label.as_deref().and_then(crate::xref_target::lexical_label)
                == Some(fragment.clone())
            {
                crate::xref_target::retarget(xref, &id, lexical);
                return;
            }
        }
    }

    /// A textless label while reading: an interdocument path, the referenced
    /// node's label, or `[id]` for an id not registered yet. The current
    /// document's title is left to conversion.
    fn reading_xref_label(
        &mut self,
        xref: &crate::Xref,
        lexical: bool,
        span: Span,
        attributes: &crate::Attributes,
    ) -> Option<Vec<Inline>> {
        if let Some(path) = &xref.path {
            return Some(crate::entity_references::reference_inlines(
                &decoded(path, lexical),
                span,
            ));
        }
        let id = decoded_refid(xref, lexical)?;
        if self.registry.contains_key(&id) {
            let style = match &xref.xrefstyle {
                Some(style) => Some(decoded(style, lexical)),
                None => attributes.get("xrefstyle").map(str::to_owned),
            };
            self.reading_label(&id, style.as_deref(), span, attributes)
        } else if id.is_empty() {
            None
        } else {
            Some(crate::entity_references::reference_inlines(
                &format!("[{id}]"),
                span,
            ))
        }
    }
}

/// The text of a pending natural reference to an unknown id that resembles
/// reftext, in lexical spelling; clears the pending lookup.
pub(crate) fn natural_text(
    xref: &mut crate::Xref,
    registry: &Registry,
    lexical: bool,
) -> Option<String> {
    let spelling = xref.natural.take()?;
    let decoded = decoded(xref.fragment.as_deref()?, lexical);
    (!registry.contains_key(&decoded) && crate::xref_target::looks_like_text(&decoded))
        .then_some(spelling)
}

/// The decoded registry id of an internal reference (`None` for the
/// current document or an interdocument path). Fields of value-generated
/// references are lexical.
pub(crate) fn decoded_refid(xref: &crate::Xref, lexical: bool) -> Option<String> {
    if xref.path.is_some() {
        return None;
    }
    xref.refid.as_deref().map(|refid| decoded(refid, lexical))
}

pub(crate) fn decoded(value: &str, lexical: bool) -> String {
    if lexical {
        crate::attributes::decode_special(value)
    } else {
        value.to_owned()
    }
}

/// Cross-references generated inside a value: `f` resolves each `OpenXref`
/// in order and, when it is directly followed by its `CloseXref` (textless),
/// may return its label, inserted as semantic parts; the node's flat value
/// follows. Labels lose their link and anchor tags, as borrowed AST labels do.
pub(crate) fn fill_labels(
    node: &mut Inline,
    mut f: impl FnMut(&mut crate::Xref, bool) -> Option<Vec<Inline>>,
) {
    match node {
        Inline::AttributeRef {
            value: Some(value),
            parts,
            ..
        }
        | Inline::Substituted { value, parts, .. } => {
            if fill_part_labels(parts, &mut f) {
                *value = crate::attributes::parts_text(parts);
            }
        }
        // References inside a footnote's text.
        Inline::Footnote { footnote, .. } => {
            fill_part_labels(&mut footnote.content, &mut f);
        }
        // References inside a keyboard, button or menu label.
        Inline::Ui { ui, .. } => {
            for label in ui.labels_mut() {
                fill_part_labels(label, &mut f);
            }
        }
        Inline::Stem { stem, .. } => {
            fill_part_labels(&mut stem.parts, &mut f);
        }
        // References inside a flow index term.
        Inline::IndexTerm { term, .. } => {
            for label in term.labels_mut() {
                fill_part_labels(label, &mut f);
            }
        }
        _ => {}
    }
}

fn fill_part_labels(
    parts: &mut Vec<crate::AttributePart>,
    f: &mut impl FnMut(&mut crate::Xref, bool) -> Option<Vec<Inline>>,
) -> bool {
    let mut filled = false;
    let mut i = 0;
    while i < parts.len() {
        if let crate::AttributePart::Footnote(footnote) = &mut parts[i] {
            filled |= fill_part_labels(&mut footnote.content, f);
            i += 1;
            continue;
        }
        if let crate::AttributePart::Ui(ui) = &mut parts[i] {
            for label in ui.labels_mut() {
                filled |= fill_part_labels(label, f);
            }
            i += 1;
            continue;
        }
        if let crate::AttributePart::Stem(stem) = &mut parts[i] {
            filled |= fill_part_labels(&mut stem.parts, f);
            i += 1;
            continue;
        }
        if let crate::AttributePart::IndexTerm(term) = &mut parts[i] {
            for label in term.labels_mut() {
                filled |= fill_part_labels(label, f);
            }
            i += 1;
            continue;
        }
        let textless = matches!(parts.get(i + 1), Some(crate::AttributePart::CloseXref));
        if let crate::AttributePart::OpenXref(xref) = &mut parts[i] {
            if let Some(mut nodes) = f(xref, textless).filter(|_| textless) {
                strip_label(&mut nodes);
                let inserted = label_parts(&nodes);
                let count = inserted.len();
                parts.splice(i + 1..i + 1, inserted);
                i += count;
                filled = true;
            }
        }
        i += 1;
    }
    filled
}

/// A borrowed label as semantic parts (plain formatting becomes boundaries).
pub(crate) fn label_parts(nodes: &[Inline]) -> Vec<crate::AttributePart> {
    use crate::AttributePart as Part;
    let mut parts = Vec::new();
    for node in nodes {
        match node {
            Inline::Text { value, .. } => crate::attributes::push_part(&mut parts, value, false),
            Inline::Raw { value, .. } => crate::attributes::push_part(&mut parts, value, true),
            Inline::Substituted { parts: inner, .. } => {
                for part in inner {
                    crate::attributes::push_value(&mut parts, part.clone());
                }
            }
            Inline::AttributeRef {
                name,
                value,
                parts: inner,
                ..
            } => match value {
                Some(_) if !inner.is_empty() => {
                    for part in inner {
                        crate::attributes::push_value(&mut parts, part.clone());
                    }
                }
                Some(value) => crate::attributes::push_part(&mut parts, value, false),
                None => crate::attributes::push_part(&mut parts, &format!("{{{name}}}"), false),
            },
            Inline::Link { children, .. } | Inline::CrossReference { children, .. } => {
                parts.extend(label_parts(children))
            }
            Inline::Anchor { .. } => {}
            Inline::Image { image, .. } => parts.push(Part::Image(Box::new(image.clone()))),
            Inline::Footnote { footnote, .. } => {
                parts.push(Part::Footnote(Box::new(footnote.clone())))
            }
            Inline::Ui { ui, .. } => parts.push(Part::Ui(Box::new(ui.clone()))),
            Inline::IndexTerm { term, .. } => parts.push(Part::IndexTerm(Box::new(term.clone()))),
            Inline::Stem { stem, .. } => parts.push(Part::Stem(Box::new(stem.clone()))),
            node => {
                let format = crate::attribute_macros::format(node).expect("formatting node");
                let (Inline::Strong { children, .. }
                | Inline::Emphasis { children, .. }
                | Inline::Monospace { children, .. }
                | Inline::Mark { children, .. }
                | Inline::Superscript { children, .. }
                | Inline::Subscript { children, .. }) = node
                else {
                    unreachable!()
                };
                parts.push(Part::Open(format));
                for part in label_parts(children) {
                    crate::attributes::push_value(&mut parts, part);
                }
                parts.push(Part::Close(format));
            }
        }
    }
    parts
}

/// The range of a deferred field's private conversion placeholder.
pub(crate) fn placeholder(nodes: &[Inline]) -> Option<(usize, usize)> {
    match nodes {
        [Inline::Text { value, span }] if value.is_empty() => Some((span.start, span.end)),
        _ => None,
    }
}

impl Ids<'_> {
    /// A label while reading. The reftext uses the reading attributes without
    /// executing expressions; a deferred title is converted early; `None`
    /// leaves the reference to conversion.
    fn reading_label(
        &mut self,
        id: &str,
        style: Option<&str>,
        span: Span,
        attributes: &crate::Attributes,
    ) -> Option<Vec<Inline>> {
        self.reading_xreftext(id, style, span, attributes)
            .or_else(|| {
                self.registry[id]
                    .title
                    .is_none()
                    .then(|| crate::entity_references::reference_inlines(&format!("[{id}]"), span))
            })
    }

    /// Ruby's `xreftext` while reading: a nonempty reftext, else the title;
    /// `None` without either or for the document title (converted later).
    fn reading_xreftext(
        &mut self,
        id: &str,
        style: Option<&str>,
        span: Span,
        attributes: &crate::Attributes,
    ) -> Option<Vec<Inline>> {
        let target = &self.registry[id];
        if let Some(nodes) = target
            .reftext
            .as_ref()
            .map(|read| read.snapshot(attributes))
            .filter(|nodes| !inline_text(nodes).is_empty())
        {
            return Some(nodes);
        }
        let title = match &target.title {
            Some(Title::Nodes(nodes)) => match placeholder(nodes) {
                None => Some(nodes.clone()),
                Some(key) => {
                    let mut converted = (self.early_title)(key, attributes)?;
                    self.resolve_reading(&mut converted, attributes);
                    self.early.insert(key, converted.clone());
                    if let Some(Title::Nodes(nodes)) = &mut self
                        .registry
                        .get_mut(id)
                        .and_then(|target| target.title.as_mut())
                    {
                        *nodes = converted.clone();
                    }
                    Some(converted)
                }
            },
            Some(Title::Heading(_, nodes)) => Some(nodes.clone()),
            Some(Title::Header) | None => None,
        }?;
        // Sections reading has not closed yet have no numerals.
        let masked;
        let numbering = match &self.registry[id].numbering {
            Numbering::Section(section) => {
                masked = Numbering::Section(section.at(self.position));
                &masked
            }
            numbering => numbering,
        };
        Some(styled(title, numbering, style, attributes, span))
    }
}

impl VisitorMut for Ids<'_> {
    fn visit_block(&mut self, block: &mut Block) {
        self.flush(block.span.start);
        self.position = block.span.start;
        // Ruby registers a section when it reads the title
        // (`initialize_section`), any other block at the end of `next_block`:
        // after its content and the inline anchors cataloged in it, converting
        // an anchored title there.
        let heading = matches!(block.kind, BlockKind::Heading { .. });
        if heading {
            self.register_block(block);
        }
        // A quote attribution was substituted while reading: from its
        // attribute line before the content, or from a blockquote credit
        // line after the content was parsed.
        let credit = match &block.kind {
            BlockKind::Quote {
                attribution: Some(attribution),
                ..
            }
            | BlockKind::Verse {
                attribution: Some(attribution),
                ..
            } => attribution_start(attribution),
            _ => None,
        };
        let before = credit.filter(|&start| start < block.span.start);
        if before.is_some() {
            self.resolve_attribution(block);
        }
        visit::walk_block_mut(self, block);
        if let (Some(start), None) = (credit, before) {
            self.flush(start);
            self.position = start;
            self.resolve_attribution(block);
        }
        if !heading {
            self.flush(block.span.end);
            self.position = block.span.end;
            self.register_block(block);
        }
    }
    fn visit_inlines(&mut self, _inlines: &mut [Inline], _context: InlineContext) {}
}

impl Ids<'_> {
    /// Register a block's id (or a heading's automatic one), resolving the
    /// references of a title converted while reading first.
    fn register_block(&mut self, block: &mut Block) {
        // The document title lends its text to an explicit id but never
        // receives an automatic one; nor does a nested document's title.
        let doctitle = Some(block.span) == self.doctitle;
        let nested_title = self
            .nested_titles
            .contains(&(block.span.start, block.span.end));
        let attributes = std::sync::Arc::clone(&block.attribute_context);
        match (&mut block.kind, &mut block.title) {
            (BlockKind::Heading { inlines, .. }, _)
                if block.id.is_none() && !doctitle && !nested_title =>
            {
                self.resolve_reading(inlines, &attributes)
            }
            (_, Some(title)) if block.id.is_some() && placeholder(title).is_none() => {
                self.resolve_reading(title, &attributes)
            }
            _ => {}
        }
        let title = match &block.kind {
            BlockKind::Heading { .. } if doctitle => Some(Title::Header),
            // A title Ruby converts at its first use is a pending block title.
            BlockKind::Heading { inlines, .. } if placeholder(inlines).is_some() => {
                Some(Title::Nodes(inlines.clone()))
            }
            BlockKind::Heading { inlines, .. } => Some(Title::Heading(
                (block.span.start, block.span.end),
                inlines.clone(),
            )),
            _ => block.title.clone().map(Title::Nodes),
        };
        let key = (block.span.start, block.span.end);
        let numbering = match (&block.kind, &block.caption) {
            (BlockKind::Heading { .. }, _) => match self.sections.get(&key) {
                Some(section) if section.is_numbered() || section.emphasized() => {
                    Numbering::Section(section.clone())
                }
                _ => Numbering::None,
            },
            (kind, Some(caption)) => match crate::context::caption_context(kind) {
                Some(context) => Numbering::Caption {
                    caption: if block.caption_parts.is_empty() {
                        vec![crate::AttributePart::Text(caption.clone())]
                    } else {
                        block.caption_parts.clone()
                    },
                    numeral: self.numerals.remove(&key),
                    context,
                },
                None => Numbering::None,
            },
            _ => Numbering::None,
        };
        if let Some(id) = block.id.clone() {
            let reftext = self.reftexts.remove(&key);
            let target = Target {
                reftext,
                title,
                order: self.registry.len(),
                numbering,
            };
            // Ruby names a section (not a discrete heading) in its warning.
            let kind = match &block.kind {
                BlockKind::Heading { .. }
                    if !doctitle
                        && !matches!(block.attrs.style.as_deref(), Some("discrete" | "float")) =>
                {
                    "section"
                }
                _ => "block",
            };
            register_id(
                &mut self.registry,
                id,
                target,
                kind,
                block.span,
                self.diagnostics,
            );
        } else if let (BlockKind::Heading { inlines, .. }, false, Some(policy)) = (
            &block.kind,
            doctitle || nested_title,
            IdPolicy::new(&attributes),
        ) {
            // Without `sectids` the heading keeps no id and is not registered.
            let id = policy.generate(&inline_text(inlines));
            let unique = policy.unique(id, |candidate| self.registry.contains_key(candidate));
            let target = Target {
                reftext: None,
                title,
                order: self.registry.len(),
                numbering,
            };
            self.registry.insert(unique.clone(), target);
            block.id = Some(unique);
        }
    }

    fn resolve_attribution(&mut self, block: &mut Block) {
        let credit = self.credits.contains(&(block.span.start, block.span.end));
        let attributes = std::sync::Arc::clone(&block.attribute_context);
        if let BlockKind::Quote {
            attribution: Some(attribution),
            ..
        }
        | BlockKind::Verse {
            attribution: Some(attribution),
            ..
        } = &mut block.kind
        {
            for part in [&mut attribution.author, &mut attribution.citetitle]
                .into_iter()
                .flatten()
            {
                self.resolve_reading(part, &attributes);
            }
            // Ruby splits the converted credit line.
            if credit {
                if let Some(author) = attribution.author.take() {
                    let credit = crate::blockquote_credit::split(author);
                    attribution.author = Some(credit.author);
                    attribution.citetitle = credit.citetitle;
                    attribution.trailing = credit.trailing;
                }
            }
        }
    }
}

/// Where a quote attribution starts in the source.
fn attribution_start(attribution: &crate::ast::Attribution) -> Option<usize> {
    [&attribution.author, &attribution.citetitle]
        .into_iter()
        .flatten()
        .filter_map(|part| part.first())
        .map(|inline| inline.span().start)
        .min()
}

/// Register `id -> reftext` in the registry, warning (at `span`) on a duplicate
/// and keeping the first definition — asciidoctor's behaviour. The duplicate
/// block/anchor still renders its (now ambiguous) id in the backends. Shared by
/// blocks, sections and inline anchors; `kind` names the owner in Ruby's
/// warning.
fn register_id(
    registry: &mut Registry,
    id: String,
    target: Target,
    kind: &str,
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match registry.entry(id) {
        Entry::Occupied(e) => diagnostics.push(Diagnostic {
            span,
            severity: Severity::Warning,
            message: format!("id assigned to {kind} already in use: {}", e.key()),
        }),
        Entry::Vacant(e) => {
            e.insert(target);
        }
    }
}

/// The automatic id of a manual page's NAME section, read when only the
/// document's id can be registered (`None` with `sectids` unset).
pub(crate) fn section_id(heading: &Block, doc_id: Option<&str>) -> Option<String> {
    let BlockKind::Heading { inlines, .. } = &heading.kind else {
        unreachable!("a section title")
    };
    let policy = IdPolicy::new(&heading.attribute_context)?;
    let id = policy.generate(&inline_text(inlines));
    Some(policy.unique(id, |candidate| doc_id == Some(candidate)))
}

/// Ruby's `Section.generate_id` settings, read from the attributes at the
/// title (`None` when `sectids` is unset, so no id is generated).
struct IdPolicy {
    /// `idprefix`, `_` when unset; inserted verbatim (not lowercased).
    prefix: String,
    /// The first character of `idseparator` (`_` when unset), `None` for an
    /// empty separator, which only deletes spaces.
    separator: Option<char>,
}

impl IdPolicy {
    fn new(attributes: &crate::Attributes) -> Option<Self> {
        attributes.get("sectids")?;
        let prefix = attributes.get("idprefix").unwrap_or("_").to_owned();
        // Ruby keeps only the first character of a longer separator.
        let separator = match attributes.get("idseparator") {
            Some(separator) => separator.chars().next(),
            None => Some('_'),
        };
        Some(Self { prefix, separator })
    }

    /// The id of an (already resolved) title: lowercase it, drop every raw
    /// tag or entity, then characters other than word chars, spaces, hyphens
    /// and dots (InvalidSectionIdCharsRx); prepend the prefix and squeeze runs
    /// of spaces, hyphens, dots and separators into one separator, dropping a
    /// trailing one (and a leading one without a prefix). An all-separator
    /// title yields an empty id, exactly as asciidoctor emits `id=""`.
    fn generate(&self, title: &str) -> String {
        let mut id = self.prefix.clone();
        id.push_str(&strip_invalid(title));
        let Some(separator) = self.separator else {
            return id.replace(' ', "");
        };
        // Ruby's `tr_s " #{sep}.-", sep`.
        let mut squeezed = String::with_capacity(id.len());
        let mut translated = false;
        for ch in id.chars() {
            if matches!(ch, ' ' | '.' | '-') || ch == separator {
                if !translated {
                    squeezed.push(separator);
                }
                translated = true;
            } else {
                squeezed.push(ch);
                translated = false;
            }
        }
        if squeezed.ends_with(separator) {
            squeezed.pop();
        }
        if self.prefix.is_empty() && squeezed.starts_with(separator) {
            squeezed.remove(0);
        }
        squeezed
    }

    /// Make `base` unique against ids already `taken`, appending the
    /// separator and `2`, `3`, … like asciidoctor.
    fn unique(&self, base: String, taken: impl Fn(&str) -> bool) -> String {
        if !taken(&base) {
            return base;
        }
        let separator = self.separator.map(String::from).unwrap_or_default();
        (2u64..)
            .map(|n| format!("{base}{separator}{n}"))
            .find(|candidate| !taken(candidate))
            .expect("an unused id")
    }
}

/// Lowercase `title` and drop what Ruby's InvalidSectionIdCharsRx matches.
fn strip_invalid(title: &str) -> String {
    // InvalidSectionIdCharsRx operates on the substituted lexical title, not
    // decoded visible text. In particular &#65; is removed, not turned into A.
    let title: String = title.chars().flat_map(char::to_lowercase).collect();
    let mut kept = String::with_capacity(title.len());
    let mut offset = 0;
    let mut unclosed_tag = false;
    while offset < title.len() {
        let rest = &title[offset..];
        if rest.starts_with('<') && !unclosed_tag {
            if let Some(end) = rest.find('>') {
                if end > 1 {
                    offset += end + 1;
                    continue;
                }
            } else {
                // Do not repeatedly scan an unfinished run of '<' to EOF.
                unclosed_tag = true;
            }
        }
        if rest.starts_with('&') {
            if let Some(end) =
                crate::entity_references::entity_name_end(title.as_bytes(), offset + 1)
            {
                offset = end;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap();
        offset += ch.len_utf8();
        // Everything else (apostrophe, slash, punctuation, em dash, thin
        // space, …) is dropped, not turned into a separator.
        if matches!(ch, ' ' | '-' | '.') || crate::unicode_word::is_word(ch) {
            kept.push(ch);
        }
    }
    kept
}

/// A lexical view for ID derivation only. Flatten semantic formatting while
/// preserving protection of literal characters and raw attribute boundaries.
pub(crate) fn inline_text(inlines: &[Inline]) -> String {
    let mut s = String::new();
    push_text(inlines, &mut s);
    s
}

fn push_text(inlines: &[Inline], s: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text { value, .. } => push_protected(value, s),
            Inline::Raw { value, .. } => s.push_str(value),
            Inline::Substituted { parts, .. } => {
                s.push_str(&crate::attributes::reference_text(parts))
            }
            // A missing attribute keeps its literal `{name}`, matching how the
            // backends render it (and so the id reflects the visible text).
            Inline::AttributeRef {
                name, value, parts, ..
            } => match value {
                Some(v) if parts.is_empty() => push_protected(v, s),
                Some(_) => {
                    s.push_str(&crate::attributes::reference_text(parts));
                }
                None => {
                    s.push('{');
                    s.push_str(name);
                    s.push('}');
                }
            },
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Monospace { children, .. }
            | Inline::Mark { children, .. }
            | Inline::Superscript { children, .. }
            | Inline::Subscript { children, .. }
            | Inline::Link { children, .. }
            | Inline::CrossReference { children, .. } => {
                // Keep structural boundaries while flattening. Otherwise raw
                // "&co", a formatted "py", and ";" would become a new entity.
                // Empty angle delimiters contribute no ID characters and avoid
                // introducing any backend's choice of formatting tags.
                s.push_str("<>");
                push_text(children, s);
                s.push_str("<>");
            }
            // An anchor or image has no visible text (Ruby removes the
            // generated tags) but still separates lexical runs.
            Inline::Anchor { .. } | Inline::Image { .. } => s.push_str("<>"),
            // Ruby's footnote markup leaves its `[index]` text.
            Inline::Footnote { footnote, .. } => s.push_str(&crate::attributes::reference_text(&[
                crate::AttributePart::Footnote(Box::new(footnote.clone())),
            ])),
            // Ruby's keyboard/button/menu markup leaves its labels (and the
            // caret's entities, which the sanitizer drops).
            Inline::Ui { ui, .. } => s.push_str(&crate::attributes::reference_text(&[
                crate::AttributePart::Ui(Box::new(ui.clone())),
            ])),
            Inline::Stem { stem, .. } => s.push_str(&stem.lexical()),
            // Ruby's flow term leaves its text, a concealed one nothing.
            Inline::IndexTerm { term, .. } => s.push_str(&crate::attributes::reference_text(&[
                crate::AttributePart::IndexTerm(Box::new(term.clone())),
            ])),
        }
    }
}

fn push_protected(text: &str, output: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(ch),
        }
    }
}

/// Ruby's `DropAnchorRx` over a borrowed label: link and anchor tags are
/// dropped, keeping their text, so a reference never nests links or repeats
/// the target's anchors. Formatting is retained; the original heading and
/// source reference positions remain unchanged.
pub(crate) fn strip_label(inlines: &mut Vec<Inline>) {
    let mut stripped = Vec::with_capacity(inlines.len());
    for mut inline in inlines.drain(..) {
        match &mut inline {
            Inline::Anchor { .. } => {}
            Inline::Link { children, .. } | Inline::CrossReference { children, .. } => {
                strip_label(children);
                stripped.append(children);
            }
            // A footnote marker loses its link: Ruby's DropAnchorRx leaves
            // `<sup class="footnote">[1]</sup>`.
            Inline::Footnote { footnote, span }
                if footnote.kind != crate::FootnoteKind::Unresolved =>
            {
                use crate::AttributePart as Part;
                let definition = footnote.kind == crate::FootnoteKind::Definition;
                let marker = footnote.marker();
                stripped.push(Inline::Substituted {
                    value: marker.clone(),
                    parts: vec![
                        Part::OpenWithAttributes {
                            format: crate::AttributeFormat::Superscript,
                            id: footnote
                                .id
                                .as_ref()
                                .filter(|_| definition)
                                .map(|id| format!("_footnote_{id}")),
                            role: Some(
                                if definition {
                                    "footnote"
                                } else {
                                    "footnoteref"
                                }
                                .into(),
                            ),
                        },
                        Part::Text(marker),
                        Part::Close(crate::AttributeFormat::Superscript),
                    ],
                    span: *span,
                });
            }
            // An image keeps its markup without its link tag.
            Inline::Image { image, .. } => {
                image.attributes.remove("link");
                stripped.push(inline);
            }
            // A keyboard, button or menu macro keeps its markup; links inside
            // its labels lose their tags.
            Inline::Ui { ui, .. } => {
                for label in ui.labels_mut() {
                    strip_parts(label);
                }
                stripped.push(inline);
            }
            Inline::Stem { stem, .. } => {
                strip_parts(&mut stem.parts);
                stripped.push(inline);
            }
            Inline::IndexTerm { term, .. } => {
                for label in term.labels_mut() {
                    strip_parts(label);
                }
                stripped.push(inline);
            }
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Monospace { children, .. }
            | Inline::Mark { children, .. }
            | Inline::Superscript { children, .. }
            | Inline::Subscript { children, .. } => {
                strip_label(children);
                stripped.push(inline);
            }
            _ => stripped.push(inline),
        }
    }
    *inlines = stripped;
    strip_attribute_links(inlines);
}

fn strip_attribute_links(inlines: &mut [Inline]) {
    struct Strip;
    impl VisitorMut for Strip {
        fn visit_inline(&mut self, inline: &mut Inline) {
            if let Inline::AttributeRef { parts, .. } | Inline::Substituted { parts, .. } = inline {
                strip_parts(parts);
            }
            visit::walk_inline_mut(self, inline);
        }
    }
    visit::walk_inlines_mut(&mut Strip, inlines);
}

/// Ruby's DropAnchorRx over generated parts: link, anchor and cross-reference
/// tags go, images lose their link, labels of keyboard, button and menu
/// macros are stripped too.
fn strip_parts(parts: &mut Vec<crate::AttributePart>) {
    for part in parts.iter_mut() {
        match part {
            crate::AttributePart::Image(image) => {
                image.attributes.remove("link");
            }
            crate::AttributePart::Ui(ui) => {
                for label in ui.labels_mut() {
                    strip_parts(label);
                }
            }
            crate::AttributePart::Stem(stem) => strip_parts(&mut stem.parts),
            crate::AttributePart::IndexTerm(term) => {
                for label in term.labels_mut() {
                    strip_parts(label);
                }
            }
            _ => {}
        }
    }
    parts.retain(|part| {
        !matches!(
            part,
            crate::AttributePart::OpenLink(_)
                | crate::AttributePart::CloseLink
                | crate::AttributePart::Anchor(_)
                | crate::AttributePart::OpenXref(_)
                | crate::AttributePart::CloseXref
        )
    });
}

#[cfg(test)]
mod tests {
    use crate::ast::{BlockKind, Inline};
    use crate::diagnostic::Severity;

    /// The id assigned to the n-th heading block (0-based), if any.
    fn heading_id(src: &str, nth: usize) -> Option<String> {
        let doc = crate::parse(src).document;
        doc.flat_blocks()
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::Heading { .. }))
            .map(|b| b.id.clone())
            .nth(nth)
            .flatten()
    }

    /// The first cross-reference (target, children) found anywhere in the doc.
    fn first_xref(src: &str) -> (String, Vec<Inline>) {
        fn find(inlines: &[Inline]) -> Option<(String, Vec<Inline>)> {
            for n in inlines {
                match n {
                    Inline::CrossReference { xref, children, .. } => {
                        return Some((xref.refid.clone().unwrap_or_default(), children.clone()))
                    }
                    Inline::Strong { children, .. }
                    | Inline::Emphasis { children, .. }
                    | Inline::Monospace { children, .. }
                    | Inline::Mark { children, .. }
                    | Inline::Superscript { children, .. }
                    | Inline::Subscript { children, .. }
                    | Inline::Link { children, .. } => {
                        if let Some(hit) = find(children) {
                            return Some(hit);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        let doc = crate::parse(src).document;
        for b in &doc.flat_blocks() {
            let inlines = match &b.kind {
                BlockKind::Heading { inlines, .. } | BlockKind::Paragraph { inlines } => inlines,
                _ => continue,
            };
            if let Some(hit) = find(inlines) {
                return hit;
            }
        }
        panic!("no cross-reference in {src:?}");
    }

    fn text_of(children: &[Inline]) -> String {
        // These resolution assertions compare text, without the temporary
        // structural boundaries used only while deriving IDs.
        super::inline_text(children).replace("<>", "")
    }

    fn generate_id(title: &str) -> String {
        let policy = super::IdPolicy {
            prefix: "_".into(),
            separator: Some('_'),
        };
        policy.generate(title)
    }

    #[test]
    fn generate_id_matches_asciidoctor() {
        // Verified against asciidoctor 2.0.23.
        assert_eq!(generate_id("Hello World"), "_hello_world");
        assert_eq!(generate_id("Hello, World!"), "_hello_world");
        assert_eq!(generate_id("foo & bar"), "_foo_bar");
        assert_eq!(generate_id("1. Intro"), "_1_intro");
        assert_eq!(generate_id("a.b.c"), "_a_b_c");
        assert_eq!(generate_id("a-b-c"), "_a_b_c");
        assert_eq!(generate_id("A/B/C"), "_abc");
        assert_eq!(generate_id("100%"), "_100");
        assert_eq!(generate_id("café déjà"), "_café_déjà");
        // The apostrophe is dropped (the resolved title carries `’`, U+2019).
        assert_eq!(generate_id("It\u{2019}s me"), "_its_me");
        // An all-separator title collapses to the empty id, like asciidoctor.
        assert_eq!(generate_id("___"), "");
    }

    fn policy(prefix: &str, separator: Option<char>) -> super::IdPolicy {
        super::IdPolicy {
            prefix: prefix.into(),
            separator,
        }
    }

    #[test]
    fn id_policy_matches_section_generate_id() {
        // Verified against asciidoctor 2.0.23 (Section.generate_id).
        let cases = [
            (policy("", Some('-')), "-Lead and trail-", "lead-and-trail"),
            (policy("", Some('.')), "a. b - c__d", "a.b.c__d"),
            (policy("Sec.", Some('_')), "Hello World", "Sec_hello_world"),
            (policy("Pre ", Some('-')), "Über  Größe", "Pre-über-größe"),
            // A letter separator is squeezed like Ruby's tr_s.
            (policy("_", Some('x')), "xxlarge x y", "_xlargexy"),
            // An empty separator only deletes spaces.
            (
                policy("sec-", None),
                "-Lead and trail-",
                "sec--leadandtrail-",
            ),
            (policy("sec-", None), "a. b - c", "sec-a.b-c"),
            // The prefix is neither lowercased nor sanitized.
            (policy("A&b;", Some('_')), "C D", "A&b;c_d"),
            (policy("", Some('_')), "___", ""),
        ];
        for (policy, title, expected) in cases {
            assert_eq!(policy.generate(title), expected, "{title}");
        }
        // Combining marks are word characters, as in Ruby's \p{Word}.
        assert_eq!(generate_id("e\u{301}t\u{e9}"), "_e\u{301}t\u{e9}");
    }

    #[test]
    fn id_policy_deduplicates_with_its_separator() {
        let taken = ["dup", "dup-2", "x"];
        let is_taken = |id: &str| taken.contains(&id);
        assert_eq!(
            policy("", Some('-')).unique("dup".into(), is_taken),
            "dup-3"
        );
        assert_eq!(policy("", None).unique("dup".into(), is_taken), "dup2");
        assert_eq!(policy("", None).unique("new".into(), is_taken), "new");
    }

    #[test]
    fn id_policy_reads_attributes_at_each_title() {
        let src = "== One Two\n\n:idprefix: p-\n\n== One Two\n\n:idseparator: --\n\n\
                   === One Two\n\n:idprefix!:\n\n== One Two\n\n[discrete]\n== One Two\n";
        let ids: Vec<_> = (0..5).map(|n| heading_id(src, n)).collect();
        assert_eq!(
            ids,
            [
                "_one_two",
                "p_one_two",
                "p-one-two",
                "_one-two",
                "_one-two-2"
            ]
            .map(|id| Some(id.to_owned()))
        );
    }

    #[test]
    fn unset_sectids_leaves_headings_without_ids() {
        let src = "= Doc\n:sectids!:\n\n== First\n\n[discrete]\n=== Float\n\n\
                   [[x]]\n== Explicit\n\n== Second [[y]]\n\n:sectids:\n\n== Later\n\n\
                   <<_first>> <<First>> <<x>>\n";
        let ids: Vec<_> = (1..6).map(|n| heading_id(src, n)).collect();
        assert_eq!(
            ids,
            [
                None,
                None,
                Some("x".into()),
                Some("y".into()),
                Some("_later".into())
            ]
        );
        // Neither the generated nor the natural id of an unnamed section resolves.
        let messages: Vec<_> = crate::parse(src)
            .diagnostics
            .into_iter()
            .map(|d| d.message)
            .collect();
        assert_eq!(
            messages,
            [
                "unresolved cross reference: '_first'",
                "unresolved cross reference: 'First'"
            ]
        );
    }

    #[test]
    fn generated_ids_cut_a_longer_separator_for_later_reading() {
        // Ruby stores the first character back into the document attribute
        // when it generates an id; explicit ids leave it alone, and body text
        // (converted from the header attributes) still sees the whole value.
        let src = ":idseparator: -+\n\nBefore {idseparator}\n\n[[a]]\n== A {idseparator}\n\n\
                   == B C {idseparator}\n\n== D {idseparator}\n\nAfter {idseparator}\n";
        let doc = crate::parse(src).document;
        let texts: Vec<_> = doc
            .flat_blocks()
            .iter()
            .filter_map(|b| match &b.kind {
                BlockKind::Heading { inlines, .. } | BlockKind::Paragraph { inlines } => {
                    Some((b.id.clone(), text_of(inlines)))
                }
                _ => None,
            })
            .collect();
        let owned = |id: Option<&str>, text: &str| (id.map(str::to_owned), text.to_owned());
        assert_eq!(
            texts,
            [
                owned(None, "Before -+"),
                owned(Some("a"), "A -+"),
                owned(Some("_b-c"), "B C -+"),
                owned(Some("_d"), "D -"),
                owned(None, "After -+"),
            ]
        );
    }

    #[test]
    fn section_gets_auto_id_doctitle_does_not() {
        // Level-1 section gets an id; the level-0 doctitle does not.
        assert_eq!(
            heading_id("= Doc Title\n\n== Real Section\n\nbody", 0),
            None
        );
        assert_eq!(
            heading_id("= Doc Title\n\n== Real Section\n\nbody", 1),
            Some("_real_section".into())
        );
    }

    #[test]
    fn duplicate_titles_get_numbered_ids() {
        let src = "== Intro\n\na\n\n== Intro\n\nb";
        assert_eq!(heading_id(src, 0), Some("_intro".into()));
        assert_eq!(heading_id(src, 1), Some("_intro_2".into()));
    }

    #[test]
    fn textless_reference_resolves_to_section_title() {
        let src = "== Section Two\n\nSee <<_section_two>>.";
        let result = crate::parse(src);
        assert!(
            result.diagnostics.is_empty(),
            "no diagnostic for a known id"
        );
        let (target, children) = first_xref(src);
        assert_eq!(target, "_section_two");
        assert_eq!(text_of(&children), "Section Two");
    }

    #[test]
    fn forward_reference_resolves() {
        // The reference precedes the section it points at.
        let src = "Intro: <<_later>>.\n\n== Later\n\nbody";
        let result = crate::parse(src);
        assert!(result.diagnostics.is_empty());
        let (_, children) = first_xref(src);
        assert_eq!(text_of(&children), "Later");
    }

    #[test]
    fn explicit_reftext_is_kept() {
        let src = "== Section Two\n\nSee <<_section_two,custom>>.";
        let (_, children) = first_xref(src);
        assert_eq!(text_of(&children), "custom");
        assert!(crate::parse(src).diagnostics.is_empty());
    }

    #[test]
    fn resolved_reftext_preserves_formatting() {
        // asciidoctor's basic xref keeps the title's inline markup.
        let src = "== The _good_ part\n\nRef: <<_the_good_part>>.";
        let (_, children) = first_xref(src);
        assert!(
            children
                .iter()
                .any(|n| matches!(n, Inline::Emphasis { .. })),
            "expected emphasis in resolved reftext, got {children:?}"
        );
        assert_eq!(text_of(&children), "The good part");
    }

    #[test]
    fn xref_macro_resolves_like_angle_form() {
        let src = "== Section Two\n\nSee xref:_section_two[].";
        let (target, children) = first_xref(src);
        assert_eq!(target, "_section_two");
        assert_eq!(text_of(&children), "Section Two");
    }

    #[test]
    fn cross_reference_inside_admonition_resolves() {
        // A `<<id>>` in an admonition body must resolve — the resolve pass descends
        // into the admonition's child blocks, not skipping them like verbatim text.
        let src = "== Later\n\nNOTE: see <<_later>>.";
        let result = crate::parse(src);
        assert!(result.diagnostics.is_empty(), "known id, no diagnostic");
        let inlines = result
            .document
            .flat_blocks()
            .iter()
            .find_map(|b| match &b.kind {
                BlockKind::Admonition { blocks, .. } => match &blocks[0].kind {
                    BlockKind::Paragraph { inlines } => Some(inlines.clone()),
                    _ => None,
                },
                _ => None,
            })
            .expect("an admonition block with a paragraph body");
        let children = inlines
            .iter()
            .find_map(|n| match n {
                Inline::CrossReference { children, .. } => Some(children.clone()),
                _ => None,
            })
            .expect("a cross-reference in the admonition body");
        assert_eq!(text_of(&children), "Later");
    }

    #[test]
    fn unknown_id_warns_and_falls_back_to_bracketed_text() {
        let src = "See <<nope>>.";
        let result = crate::parse(src);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].severity, Severity::Warning);
        // The diagnostic span covers the whole `<<nope>>` construct.
        let span = result.diagnostics[0].span;
        assert_eq!(&src[span.start..span.end], "<<nope>>");
        let (target, children) = first_xref(src);
        assert_eq!(target, "nope");
        assert_eq!(text_of(&children), "[nope]");
    }

    #[test]
    fn unknown_id_with_explicit_text_keeps_text_but_warns() {
        let src = "See <<nope,go there>>.";
        let result = crate::parse(src);
        assert_eq!(result.diagnostics.len(), 1);
        let (_, children) = first_xref(src);
        assert_eq!(text_of(&children), "go there");
    }

    #[test]
    fn textless_ref_to_paragraph_anchor_falls_back_without_diagnostic() {
        // The id is *known* (a paragraph anchor) but has no natural text, so the
        // display falls back to `[id]` — and, unlike an unknown id, no warning.
        let src = "[[pa]]\nHello.\n\nSee <<pa>>.";
        let result = crate::parse(src);
        assert!(
            result.diagnostics.is_empty(),
            "no diagnostic for a known id"
        );
        let (target, children) = first_xref(src);
        assert_eq!(target, "pa");
        assert_eq!(text_of(&children), "[pa]");
    }

    #[test]
    fn textless_ref_to_anchor_reftext_is_display_text() {
        let src = "[[pa,The Para]]\nHello.\n\nSee <<pa>>.";
        let result = crate::parse(src);
        assert!(result.diagnostics.is_empty());
        let (_, children) = first_xref(src);
        assert_eq!(text_of(&children), "The Para");
    }

    #[test]
    fn explicit_anchor_overrides_section_auto_id() {
        // `[[custom]]` before a section replaces the auto id `_real_title`; a
        // textless `<<custom>>` still shows the section's title.
        let src = "[[custom]]\n== Real Title\n\nSee <<custom>>.";
        let result = crate::parse(src);
        assert!(result.diagnostics.is_empty());
        assert_eq!(heading_id(src, 0), Some("custom".into()));
        let (target, children) = first_xref(src);
        assert_eq!(target, "custom");
        assert_eq!(text_of(&children), "Real Title");
    }

    #[test]
    fn explicit_reftext_overrides_section_title() {
        let src = "[[s,Custom Ref]]\n== Real Title\n\nSee <<s>>.";
        let (_, children) = first_xref(src);
        assert_eq!(text_of(&children), "Custom Ref");
        assert!(crate::parse(src).diagnostics.is_empty());
    }

    #[test]
    fn duplicate_explicit_id_warns_once_and_first_wins() {
        let src = "[[dup]]\nFirst.\n\n[[dup]]\nSecond.\n\nSee <<dup>>.";
        let result = crate::parse(src);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].severity, Severity::Warning);
        assert!(
            result.diagnostics[0].message.contains("already in use"),
            "got: {}",
            result.diagnostics[0].message
        );
        // The first definition (no reftext) wins → textless ref renders `[dup]`.
        let (_, children) = first_xref(src);
        assert_eq!(text_of(&children), "[dup]");
    }

    #[test]
    fn xref_resolves_to_inline_anchor_reftext() {
        // A textless `<<id>>` to an *inline* `[[id,reftext]]` shows the reftext —
        // and resolves forward, the anchor coming after the reference.
        let src = "Go to <<point>>.\n\nSee the [[point,the Important Part]]here.";
        let result = crate::parse(src);
        assert!(result.diagnostics.is_empty(), "known id → no diagnostic");
        let (target, children) = first_xref(src);
        assert_eq!(target, "point");
        assert_eq!(text_of(&children), "the Important Part");
    }

    #[test]
    fn xref_to_textless_inline_anchor_falls_back_without_diagnostic() {
        // The inline anchor's id is known but has no reftext → `[id]`, no warning.
        let src = "Go to <<spot>>.\n\nSee the [[spot]]here.";
        let result = crate::parse(src);
        assert!(result.diagnostics.is_empty(), "known id → no diagnostic");
        let (_, children) = first_xref(src);
        assert_eq!(text_of(&children), "[spot]");
    }

    #[test]
    fn anchor_macro_id_resolves_like_the_shorthand() {
        let src = "Go to <<point>>.\n\nSee anchor:point[the Part] now.";
        let (_, children) = first_xref(src);
        assert_eq!(text_of(&children), "the Part");
    }

    #[test]
    fn duplicate_inline_anchor_and_block_anchor_warns() {
        // An inline `[[dup]]` colliding with an earlier block `[[dup]]` warns once
        // (the first definition wins), like two block anchors.
        let src = "[[dup]]\nFirst.\n\nthen [[dup]]again.";
        let result = crate::parse(src);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].severity, Severity::Warning);
        assert!(result.diagnostics[0].message.contains("already in use"));
    }

    /// The `caption` of every top-level example block, in document order.
    fn example_captions(src: &str) -> Vec<Option<String>> {
        crate::parse(src)
            .document
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::Example { .. }))
            .map(|b| b.caption.clone())
            .collect()
    }

    #[test]
    fn titled_example_gets_numbered_caption() {
        let src = ".First\n====\none\n====";
        assert_eq!(example_captions(src), vec![Some("Example 1. ".into())]);
    }

    #[test]
    fn two_titled_examples_number_sequentially() {
        let src = ".First\n====\none\n====\n\n.Second\n====\ntwo\n====";
        assert_eq!(
            example_captions(src),
            vec![Some("Example 1. ".into()), Some("Example 2. ".into())]
        );
    }

    #[test]
    fn untitled_example_has_no_caption_and_does_not_advance_counter() {
        // titled, untitled, titled → 1, (none), 2 — the untitled block neither
        // gets a caption nor consumes a number, matching asciidoctor.
        let src = ".First\n====\none\n====\n\n====\nmid\n====\n\n.Third\n====\nthree\n====";
        assert_eq!(
            example_captions(src),
            vec![Some("Example 1. ".into()), None, Some("Example 2. ".into())]
        );
    }

    #[test]
    fn example_caption_attribute_overrides_prefix() {
        let src = ":example-caption: Esempio\n\n.Custom\n====\nx\n====";
        assert_eq!(example_captions(src), vec![Some("Esempio 1. ".into())]);
    }

    #[test]
    fn titled_listing_gets_no_caption() {
        // Only example blocks are numbered; a titled listing keeps a bare title
        // (asciidoctor's `listing-caption` is unset by default).
        let src = ".Code\n----\nputs 1\n----";
        let caption = crate::parse(src)
            .document
            .blocks
            .iter()
            .find(|b| matches!(b.kind, BlockKind::Listing { .. }))
            .and_then(|b| b.caption.clone());
        assert_eq!(caption, None);
    }

    #[test]
    fn example_nested_in_quote_is_numbered_but_quote_is_not() {
        // The caption pass descends into a quote container: a titled example
        // inside a `____` block still gets numbered; the quote carries no caption.
        let src = "____\n.Inner\n====\nx\n====\n____";
        let doc = crate::parse(src).document;
        let quote = &doc.flat_blocks()[0];
        assert!(quote.caption.is_none());
        let inner = match &quote.kind {
            BlockKind::Quote { blocks, .. } => &blocks[0],
            other => panic!("expected quote, got {other:?}"),
        };
        assert_eq!(inner.caption.as_deref(), Some("Example 1. "));
    }
}
