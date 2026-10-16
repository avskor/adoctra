//! Post-parse resolution: assign section ids and resolve cross-references.
//!
//! This is the first pass that needs the *whole* document at once. The inline
//! resolution done while scanning (`{attr}` references, typographic
//! replacements) is block-local, but a `<<id>>` may point forward to a section
//! defined later, so the registry of section ids must be complete before any
//! reference resolves.
//!
//! Two phases over the block tree:
//!   1. give every section (`level >= 1`) an auto-generated id and record
//!      `id -> title inlines` in a registry (asciidoctor's `sectids`);
//!   2. walk every inline and, for each [`Inline::CrossReference`], substitute
//!      the section's title as display text (when the reference was textless),
//!      keep an explicit reftext, and — for an unknown id — emit a diagnostic
//!      and fall back to the `[id]` text asciidoctor renders for unresolved
//!      references.

use std::collections::{btree_map::Entry, BTreeMap};

use crate::ast::{Block, BlockKind, Document, Inline, Span};
use crate::diagnostic::{Diagnostic, Severity};
use crate::visit::{self, InlineContext, VisitorMut};

/// A registry of anchor ids → the inlines a *textless* reference displays. The
/// value is `Some(text)` for a section (its title) or an explicit
/// `[[id,reftext]]` anchor; `None` for an id whose block has no natural text
/// (e.g. a plain paragraph anchor), where a textless reference falls back to
/// `[id]` *without* a diagnostic. Ordered so the structure stays deterministic
/// (like [`Attributes`]).
type Registry = BTreeMap<String, Option<Vec<Inline>>>;

/// Inline anchors cataloged while reading, keyed by their physical start.
type Anchors = BTreeMap<(crate::SourceId, usize), (String, Option<Vec<Inline>>)>;

/// Assign section ids and resolve cross-references over `doc`, pushing a
/// diagnostic for every reference whose id is not in the registry (and for a
/// duplicate explicit id).
pub(crate) fn resolve(
    doc: &mut Document,
    anchors: Vec<(String, crate::anchor_catalog::Entry)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let anchors = anchors
        .into_iter()
        .map(|(id, entry)| {
            let span = entry.span;
            ((span.source_id, span.start), (id, entry.reftext))
        })
        .collect();
    // Every heading except the document title gets an automatic id, including
    // discrete and level-0 headings in the body, as in asciidoctor.
    let doctitle = doc
        .blocks
        .first()
        .filter(|_| doc.has_title)
        .map(|block| block.span);
    let mut registry = Registry::new();
    assign_ids(
        &mut doc.blocks,
        anchors,
        &mut registry,
        diagnostics,
        doctitle,
    );
    resolve_xrefs(&mut doc.blocks, &registry, diagnostics);
}

/// Register block ids and body anchors before resolving forward references.
fn assign_ids(
    blocks: &mut [Block],
    mut anchors: Anchors,
    registry: &mut Registry,
    diagnostics: &mut Vec<Diagnostic>,
    doctitle: Option<Span>,
) {
    struct Ids<'a> {
        doctitle: Option<Span>,
        anchors: &'a mut Anchors,
        registry: &'a mut Registry,
        diagnostics: &'a mut Vec<Diagnostic>,
    }
    impl VisitorMut for Ids<'_> {
        fn visit_block(&mut self, block: &mut Block) {
            let heading_title = match &block.kind {
                BlockKind::Heading { inlines, .. } => Some(inlines.clone()),
                _ => None,
            };
            // The document title lends its text to an explicit id but never
            // receives an automatic one.
            let automatic = Some(block.span) != self.doctitle;
            if let Some(id) = block.id.clone() {
                let reftext = block
                    .reftext
                    .clone()
                    .filter(|nodes| !inline_text(nodes).is_empty())
                    .or(heading_title);
                register_id(self.registry, id, reftext, block.span, self.diagnostics);
            } else if let Some(title) = heading_title.filter(|_| automatic) {
                let unique = dedup(generate_id(&inline_text(&title)), self.registry);
                self.registry.insert(unique.clone(), Some(title));
                block.id = Some(unique);
            }
            visit::walk_block_mut(self, block);
        }
        fn visit_inlines(&mut self, inlines: &mut [Inline], context: InlineContext) {
            // Metadata follows different substitution/registration rules in Ruby.
            // Preserve the existing body-anchor semantics until those are ported.
            if context == InlineContext::Body {
                visit::walk_inlines_mut(self, inlines);
            }
        }
        fn visit_inline(&mut self, inline: &mut Inline) {
            // Only anchors cataloged while reading join the registry, with
            // their reading-time reftext; others still render.
            if let Inline::Anchor { id, span, .. } = inline {
                let key = (span.source_id, span.start);
                if self
                    .anchors
                    .get(&key)
                    .is_some_and(|(cataloged, _)| cataloged == id)
                {
                    let (id, reftext) = self.anchors.remove(&key).unwrap();
                    register_id(self.registry, id, reftext, *span, self.diagnostics);
                }
            }
            visit::walk_inline_mut(self, inline);
        }
    }
    visit::walk_blocks_mut(
        &mut Ids {
            doctitle,
            anchors: &mut anchors,
            registry,
            diagnostics,
        },
        blocks,
    );
    // Ruby registers cataloged anchors even where the rendered text does not
    // form an anchor (for example, quote metadata inside the reftext).
    for ((source_id, start), (id, reftext)) in anchors {
        let span = Span {
            source_id,
            ..Span::new(start, start)
        };
        register_id(registry, id, reftext, span, diagnostics);
    }
}

/// Register `id -> reftext` in the registry, warning (at `span`) on a duplicate
/// and keeping the first definition — asciidoctor's behaviour. The duplicate
/// block/anchor still renders its (now ambiguous) id in the backends. Shared by
/// block anchors and inline anchors.
fn register_id(
    registry: &mut Registry,
    id: String,
    reftext: Option<Vec<Inline>>,
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
            e.insert(reftext);
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
fn inline_text(inlines: &[Inline]) -> String {
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

/// Phase 2: walk the block tree and resolve every cross-reference's display
/// text against the registry.
fn resolve_xrefs(blocks: &mut [Block], registry: &Registry, diagnostics: &mut Vec<Diagnostic>) {
    struct References<'a> {
        registry: &'a Registry,
        diagnostics: &'a mut Vec<Diagnostic>,
    }
    impl VisitorMut for References<'_> {
        fn visit_inlines(&mut self, inlines: &mut [Inline], context: InlineContext) {
            // Titles use normal substitutions. Reftext and attribution have their
            // own rules; the traversal exposes them without treating them as body.
            if matches!(context, InlineContext::Body | InlineContext::Title) {
                visit::walk_inlines_mut(self, inlines);
            }
        }
        fn visit_inline(&mut self, inline: &mut Inline) {
            if let Inline::CrossReference {
                target,
                children,
                span,
            } = inline
            {
                let reftext = self.registry.get(target.as_str());
                if reftext.is_none() {
                    self.diagnostics.push(Diagnostic {
                        span: *span,
                        severity: Severity::Warning,
                        message: format!("unresolved cross reference: '{target}'"),
                    });
                }
                if children.is_empty() {
                    *children = match reftext {
                        Some(Some(text)) => {
                            let mut label = text.clone();
                            strip_label(&mut label);
                            label
                        }
                        _ => vec![Inline::Text {
                            value: format!("[{target}]"),
                            span: *span,
                        }],
                    };
                    // Borrowed display text can itself contain a reference to this
                    // target. Do not recursively expand generated children.
                    return;
                }
            }
            visit::walk_inline_mut(self, inline);
        }
    }
    visit::walk_blocks_mut(
        &mut References {
            registry,
            diagnostics,
        },
        blocks,
    );
}

/// Implicit xref labels retain formatting but drop generated link boundaries.
/// The original heading and source reference positions remain unchanged.
/// A borrowed label drops generated link boundaries and inline anchors, so the
/// reference does not nest links or repeat the target's anchors.
fn strip_label(inlines: &mut Vec<Inline>) {
    inlines.retain(|inline| !matches!(inline, Inline::Anchor { .. }));
    for inline in inlines.iter_mut() {
        if let Inline::Strong { children, .. }
        | Inline::Emphasis { children, .. }
        | Inline::Monospace { children, .. }
        | Inline::Mark { children, .. }
        | Inline::Superscript { children, .. }
        | Inline::Subscript { children, .. }
        | Inline::Link { children, .. }
        | Inline::CrossReference { children, .. } = inline
        {
            strip_label(children);
        }
    }
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
                        crate::AttributePart::OpenLink(_) | crate::AttributePart::CloseLink
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
