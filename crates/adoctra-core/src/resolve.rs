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

/// Assign section ids and register every block id and cataloged inline
/// anchor, pushing a diagnostic for a duplicate id (the first one wins).
pub(crate) fn register(
    doc: &mut Document,
    mut anchors: Vec<(String, crate::anchor_catalog::Entry)>,
    mut reftexts: BTreeMap<(usize, usize), crate::reftext::Read>,
    early_title: &mut EarlyTitle<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Registry, EarlyTitles) {
    // Every heading except the document title gets an automatic id, including
    // discrete and level-0 headings in the body, as in asciidoctor.
    let doctitle = doc
        .blocks
        .first()
        .filter(|_| doc.has_title)
        .map(|block| block.span);
    // Inline anchors join in source order, between the blocks around them.
    anchors.sort_by_key(|(_, entry)| entry.span.start);
    let mut ids = Ids {
        doctitle,
        anchors: anchors.into_iter().peekable(),
        reftexts: &mut reftexts,
        early_title,
        early: EarlyTitles::new(),
        registry: Registry::new(),
        diagnostics,
    };
    visit::walk_blocks_mut(&mut ids, &mut doc.blocks);
    ids.flush(usize::MAX);
    (ids.registry, ids.early)
}

struct Ids<'a> {
    doctitle: Option<Span>,
    anchors: std::iter::Peekable<std::vec::IntoIter<(String, crate::anchor_catalog::Entry)>>,
    reftexts: &'a mut BTreeMap<(usize, usize), crate::reftext::Read>,
    early_title: &'a mut EarlyTitle<'a>,
    early: EarlyTitles,
    registry: Registry,
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl Ids<'_> {
    /// Register cataloged anchors that start before `position`.
    fn flush(&mut self, position: usize) {
        while self
            .anchors
            .peek()
            .is_some_and(|(_, entry)| entry.span.start < position)
        {
            let (id, entry) = self.anchors.next().unwrap();
            let target = Target {
                reftext: entry.reftext,
                title: None,
            };
            register_id(&mut self.registry, id, target, entry.span, self.diagnostics);
        }
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
                    target,
                    children,
                    span,
                } if children.is_empty() => {
                    let label = match self.registry.get(target.as_str()) {
                        None => Some(vec![Inline::Text {
                            value: format!("[{target}]"),
                            span: *span,
                        }]),
                        Some(_) => self.reading_label(target, *span, attributes),
                    };
                    if let Some(mut label) = label {
                        strip_label(&mut label);
                        *children = label;
                    }
                }
                Inline::Strong { children, .. }
                | Inline::Emphasis { children, .. }
                | Inline::Monospace { children, .. }
                | Inline::Mark { children, .. }
                | Inline::Superscript { children, .. }
                | Inline::Subscript { children, .. }
                | Inline::Link { children, .. }
                | Inline::CrossReference { children, .. } => {
                    self.resolve_reading(children, attributes)
                }
                _ => {}
            }
        }
    }
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
        match &target.title {
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
            Some(Title::Header) => None,
            None => Some(vec![Inline::Text {
                value: format!("[{id}]"),
                span,
            }]),
        }
    }
}

impl VisitorMut for Ids<'_> {
    fn visit_block(&mut self, block: &mut Block) {
        self.flush(block.span.start);
        // The document title lends its text to an explicit id but never
        // receives an automatic one.
        let doctitle = Some(block.span) == self.doctitle;
        let attributes = std::sync::Arc::clone(&block.attribute_context);
        match (&mut block.kind, &mut block.title) {
            (BlockKind::Heading { inlines, .. }, _) if block.id.is_none() && !doctitle => {
                self.resolve_reading(inlines, &attributes)
            }
            (_, Some(title)) if block.id.is_some() && placeholder(title).is_none() => {
                self.resolve_reading(title, &attributes)
            }
            _ => {}
        }
        let title = match &block.kind {
            BlockKind::Heading { .. } if doctitle => Some(Title::Header),
            BlockKind::Heading { inlines, .. } => Some(Title::Heading(
                (block.span.start, block.span.end),
                inlines.clone(),
            )),
            _ => block.title.clone().map(Title::Nodes),
        };
        if let Some(id) = block.id.clone() {
            let reftext = self.reftexts.remove(&(block.span.start, block.span.end));
            let target = Target { reftext, title };
            register_id(&mut self.registry, id, target, block.span, self.diagnostics);
        } else if let (BlockKind::Heading { inlines, .. }, false) = (&block.kind, doctitle) {
            let unique = dedup(generate_id(&inline_text(inlines)), &self.registry);
            let target = Target {
                reftext: None,
                title,
            };
            self.registry.insert(unique.clone(), target);
            block.id = Some(unique);
        }
        visit::walk_block_mut(self, block);
    }
    fn visit_inlines(&mut self, _inlines: &mut [Inline], _context: InlineContext) {}
}

/// Register `id -> reftext` in the registry, warning (at `span`) on a duplicate
/// and keeping the first definition — asciidoctor's behaviour. The duplicate
/// block/anchor still renders its (now ambiguous) id in the backends. Shared by
/// block anchors and inline anchors.
fn register_id(
    registry: &mut Registry,
    id: String,
    target: Target,
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match registry.entry(id) {
        Entry::Occupied(e) => diagnostics.push(Diagnostic {
            span,
            severity: Severity::Warning,
            message: format!("id already in use: '{}'", e.key()),
        }),
        Entry::Vacant(e) => {
            e.insert(target);
        }
    }
}

/// Make `base` unique against ids already in `registry`, appending
/// `_2`, `_3`, … like asciidoctor (the base already carries the `_` prefix).
fn dedup(base: String, registry: &Registry) -> String {
    if !registry.contains_key(&base) {
        return base;
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}_{n}");
        if !registry.contains_key(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Auto-generate a section id from its (already resolved) title text, matching
/// asciidoctor's default `idprefix=_`, `idseparator=_`: lowercase; drop every
/// raw tag or entity, then characters other than word chars, spaces, hyphens
/// and dots; map runs of
/// space/hyphen/dot (and adjacent separators) to a single `_`; trim leading and
/// trailing `_`; prefix `_`. An all-separator title yields an empty id, exactly
/// as asciidoctor emits `id=""`.
fn generate_id(title: &str) -> String {
    // InvalidSectionIdCharsRx operates on the substituted lexical title, not
    // decoded visible text. In particular &#65; is removed, not turned into A.
    let title: String = title.chars().flat_map(char::to_lowercase).collect();
    let mut mapped = String::with_capacity(title.len());
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
        if ch.is_alphanumeric() || ch == '_' {
            mapped.push(ch);
        } else if ch == ' ' || ch == '-' || ch == '.' {
            mapped.push('_');
        }
        // Everything else (apostrophe, slash, punctuation, em dash, thin
        // space, …) is dropped, not turned into a separator.
    }

    // Collapse runs of `_` and trim them off both ends.
    let mut collapsed = String::with_capacity(mapped.len());
    let mut prev_underscore = false;
    for ch in mapped.chars() {
        if ch == '_' {
            if !prev_underscore {
                collapsed.push('_');
            }
            prev_underscore = true;
        } else {
            collapsed.push(ch);
            prev_underscore = false;
        }
    }
    let core = collapsed.trim_matches('_');
    if core.is_empty() {
        String::new()
    } else {
        format!("_{core}")
    }
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
            // An anchor has no visible text but still separates lexical runs.
            Inline::Anchor { .. } => s.push_str("<>"),
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
                parts.retain(|part| {
                    !matches!(
                        part,
                        crate::AttributePart::OpenLink(_)
                            | crate::AttributePart::CloseLink
                            | crate::AttributePart::Anchor(_)
                    )
                });
            }
            visit::walk_inline_mut(self, inline);
        }
    }
    visit::walk_inlines_mut(&mut Strip, inlines);
}

#[cfg(test)]
mod tests {
    use crate::ast::{BlockKind, Inline};
    use crate::diagnostic::Severity;

    /// The id assigned to the n-th heading block (0-based), if any.
    fn heading_id(src: &str, nth: usize) -> Option<String> {
        let doc = crate::parse(src).document;
        doc.blocks
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
                    Inline::CrossReference {
                        target, children, ..
                    } => return Some((target.clone(), children.clone())),
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
        for b in &doc.blocks {
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

    #[test]
    fn generate_id_matches_asciidoctor() {
        // Verified against asciidoctor 2.0.23.
        assert_eq!(super::generate_id("Hello World"), "_hello_world");
        assert_eq!(super::generate_id("Hello, World!"), "_hello_world");
        assert_eq!(super::generate_id("foo & bar"), "_foo_bar");
        assert_eq!(super::generate_id("1. Intro"), "_1_intro");
        assert_eq!(super::generate_id("a.b.c"), "_a_b_c");
        assert_eq!(super::generate_id("a-b-c"), "_a_b_c");
        assert_eq!(super::generate_id("A/B/C"), "_abc");
        assert_eq!(super::generate_id("100%"), "_100");
        assert_eq!(super::generate_id("café déjà"), "_café_déjà");
        // The apostrophe is dropped (the resolved title carries `’`, U+2019).
        assert_eq!(super::generate_id("It\u{2019}s me"), "_its_me");
        // An all-separator title collapses to the empty id, like asciidoctor.
        assert_eq!(super::generate_id("___"), "");
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
            .blocks
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
        let quote = &doc.blocks[0];
        assert!(quote.caption.is_none());
        let inner = match &quote.kind {
            BlockKind::Quote { blocks, .. } => &blocks[0],
            other => panic!("expected quote, got {other:?}"),
        };
        assert_eq!(inner.caption.as_deref(), Some("Example 1. "));
    }
}
