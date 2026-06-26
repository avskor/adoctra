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

use std::collections::BTreeMap;

use crate::ast::{Block, BlockKind, Document, Inline};
use crate::diagnostic::{Diagnostic, Severity};

/// A registry of section ids → the title inlines used as a reference's display
/// text. Ordered so the structure stays deterministic (like [`Attributes`]).
type Registry = BTreeMap<String, Vec<Inline>>;

/// Assign section ids and resolve cross-references over `doc`, pushing a
/// diagnostic for every reference whose id is not in the registry.
pub(crate) fn resolve(doc: &mut Document, diagnostics: &mut Vec<Diagnostic>) {
    let mut registry = Registry::new();
    assign_ids(&mut doc.blocks, &mut registry);
    resolve_xrefs(&mut doc.blocks, &registry, diagnostics);
}

/// Phase 1: walk the block tree, give each section an id and register its title.
fn assign_ids(blocks: &mut [Block], registry: &mut Registry) {
    for block in blocks {
        match &mut block.kind {
            BlockKind::Heading { level, id, inlines } => {
                // The level-0 document title is not a section and gets no id.
                if *level >= 1 {
                    let unique = dedup(generate_id(&inline_text(inlines)), registry);
                    registry.insert(unique.clone(), inlines.clone());
                    *id = Some(unique);
                }
            }
            BlockKind::List { items, .. } => {
                for item in items {
                    assign_ids(&mut item.blocks, registry);
                }
            }
            BlockKind::Paragraph { .. } | BlockKind::Listing { .. } => {}
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
/// char that is not a word char, space, hyphen or dot; map runs of
/// space/hyphen/dot (and adjacent separators) to a single `_`; trim leading and
/// trailing `_`; prefix `_`. An all-separator title yields an empty id, exactly
/// as asciidoctor emits `id=""`.
fn generate_id(title: &str) -> String {
    let mut mapped = String::with_capacity(title.len());
    for ch in title.chars().flat_map(char::to_lowercase) {
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

/// The visible text of an inline run, with formatting flattened — used to
/// generate a section id from its title.
fn inline_text(inlines: &[Inline]) -> String {
    let mut s = String::new();
    push_text(inlines, &mut s);
    s
}

fn push_text(inlines: &[Inline], s: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text { value, .. } => s.push_str(value),
            // A missing attribute keeps its literal `{name}`, matching how the
            // backends render it (and so the id reflects the visible text).
            Inline::AttributeRef { name, value, .. } => match value {
                Some(v) => s.push_str(v),
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
            | Inline::CrossReference { children, .. } => push_text(children, s),
        }
    }
}

/// Phase 2: walk the block tree and resolve every cross-reference's display
/// text against the registry.
fn resolve_xrefs(blocks: &mut [Block], registry: &Registry, diagnostics: &mut Vec<Diagnostic>) {
    for block in blocks {
        match &mut block.kind {
            BlockKind::Heading { inlines, .. } | BlockKind::Paragraph { inlines } => {
                resolve_inlines(inlines, registry, diagnostics);
            }
            BlockKind::List { items, .. } => {
                for item in items {
                    resolve_inlines(&mut item.inlines, registry, diagnostics);
                    resolve_xrefs(&mut item.blocks, registry, diagnostics);
                }
            }
            BlockKind::Listing { .. } => {}
        }
    }
}

fn resolve_inlines(inlines: &mut [Inline], registry: &Registry, diagnostics: &mut Vec<Diagnostic>) {
    for inline in inlines {
        match inline {
            Inline::CrossReference {
                target,
                children,
                span,
            } => match registry.get(target.as_str()) {
                // Known id: a textless reference borrows the section title as
                // display text; an explicit reftext is kept (and still scanned
                // for nested references the author may have written).
                Some(title) => {
                    if children.is_empty() {
                        *children = title.clone();
                    } else {
                        resolve_inlines(children, registry, diagnostics);
                    }
                }
                // Unknown id: warn, and for a textless reference fall back to the
                // `[id]` text asciidoctor renders for an unresolved reference.
                None => {
                    diagnostics.push(Diagnostic {
                        span: *span,
                        severity: Severity::Warning,
                        message: format!("unresolved cross reference: '{target}'"),
                    });
                    if children.is_empty() {
                        *children = vec![Inline::Text {
                            value: format!("[{target}]"),
                            span: *span,
                        }];
                    } else {
                        resolve_inlines(children, registry, diagnostics);
                    }
                }
            },
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Monospace { children, .. }
            | Inline::Mark { children, .. }
            | Inline::Superscript { children, .. }
            | Inline::Subscript { children, .. }
            | Inline::Link { children, .. } => {
                resolve_inlines(children, registry, diagnostics);
            }
            Inline::Text { .. } | Inline::AttributeRef { .. } => {}
        }
    }
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
            .filter_map(|b| match &b.kind {
                BlockKind::Heading { id, .. } => Some(id.clone()),
                _ => None,
            })
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
        super::inline_text(children)
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
}
