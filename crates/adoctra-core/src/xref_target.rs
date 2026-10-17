//! Cross-reference targets, like the body of Ruby's `InlineXrefMacroRx`
//! substitution: a `path#fragment` refid names another document unless it is
//! this document (`docname`) or one included in full; a bare id may name a
//! title or reftext (`Document#resolve_id`, resolved after the registry exists).
use crate::{AttributePart, Attributes, Inline, Xref};
use std::collections::BTreeMap;

/// Ruby's `ASCIIDOC_EXTENSIONS`.
pub(crate) const ASCIIDOC_EXTENSIONS: [&str; 5] = [".adoc", ".asciidoc", ".asc", ".ad", ".txt"];

/// Ruby's `catalog[:includes]`: AsciiDoc includes read so far, keyed by their
/// path relative to the document directory without the extension. A partial
/// include (selected lines or tags) is tracked but not visible to references.
pub(crate) type Includes = BTreeMap<String, bool>;

/// The document state an interdocument reference needs while substituting.
#[derive(Clone, Copy)]
pub(crate) struct Scope<'a> {
    pub includes: &'a Includes,
    /// `outfilesuffix` pinned at the end of the header; `None` before it.
    pub outfilesuffix: Option<&'a str>,
}

/// Record an include in the catalog; a full include makes the key visible.
pub(crate) fn register_include(includes: &mut Includes, key: String, full: bool) {
    *includes.entry(key).or_insert(false) |= full;
}

/// Ruby's `PathResolver#relative_path` of an included file against the
/// document directory, without the AsciiDoc extension.
pub(crate) fn include_key(document: &str, include: &str) -> String {
    let base: Vec<_> = document
        .rsplit_once('/')
        .map_or("", |(dir, _)| dir)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let path: Vec<_> = include.split('/').filter(|s| !s.is_empty()).collect();
    let common = base.iter().zip(&path).take_while(|(a, b)| a == b).count();
    let mut relative: Vec<&str> = vec![".."; base.len() - common];
    relative.extend(&path[common..]);
    let relative = relative.join("/");
    match relative.rfind('.') {
        Some(dot) => relative[..dot].to_owned(),
        None => relative,
    }
}

/// Ruby's `Helpers.extname?`.
fn extname(path: &str) -> bool {
    path.rfind('.')
        .is_some_and(|dot| !path[dot..].contains('/'))
}

/// Resolve a lexical refid (entity protection retained) as the macros stage
/// does. A natural reference keeps `#id` until the registry can look it up.
pub(crate) fn resolve(
    refid: &str,
    macro_form: bool,
    compat: bool,
    attributes: &Attributes,
    scope: Option<Scope<'_>>,
) -> Xref {
    let mut path = None;
    let mut fragment = None;
    let mut src2src = false;
    let mut explicit = None;
    // A `#` preceded by `&` belongs to a character reference; Ruby's index
    // -1 makes a leading `#` check the last character.
    let hash = refid.find('#').filter(|&i| {
        !if i == 0 {
            refid.ends_with('&')
        } else {
            refid[..i].ends_with('&')
        }
    });
    if compat {
        fragment = Some(refid.to_owned());
    } else if let Some(hash) = hash {
        if hash > 0 {
            let mut name = &refid[..hash];
            if hash + 1 < refid.len() {
                fragment = Some(refid[hash + 1..].to_owned());
            }
            if macro_form {
                if let Some(stem) = name.strip_suffix(".adoc") {
                    name = stem;
                    src2src = true;
                } else {
                    src2src = !extname(name);
                }
            } else {
                if ASCIIDOC_EXTENSIONS.iter().any(|ext| name.ends_with(ext)) {
                    name = &name[..name.rfind('.').unwrap()];
                }
                src2src = true;
            }
            path = Some(name.to_owned());
        } else {
            explicit = Some(refid.to_owned());
            fragment = Some(refid[1..].to_owned());
        }
    } else if macro_form {
        if let Some(stem) = refid.strip_suffix(".adoc") {
            src2src = true;
            path = Some(stem.to_owned());
        } else if extname(refid) {
            path = Some(refid.to_owned());
        } else {
            fragment = Some(refid.to_owned());
        }
    } else {
        fragment = Some(refid.to_owned());
    }

    if let Some(target) = explicit {
        return Xref {
            target,
            refid: fragment.clone(),
            fragment,
            ..Default::default()
        };
    }
    let Some(name) = path else {
        let fragment = fragment.unwrap_or_default();
        return Xref {
            natural: !compat,
            ..Xref::to_id(fragment)
        };
    };
    let included = scope.is_some_and(|scope| scope.includes.get(&name) == Some(&true));
    if src2src && (attributes.get("docname") == Some(name.as_str()) || included) {
        return match fragment {
            Some(fragment) => Xref::to_id(fragment),
            None => Xref {
                target: "#".into(),
                ..Default::default()
            },
        };
    }
    let suffix = if src2src {
        attributes
            .get("relfilesuffix")
            .or(scope.and_then(|scope| scope.outfilesuffix))
            .unwrap_or("")
    } else {
        ""
    };
    let output = format!(
        "{}{name}{suffix}",
        attributes.get("relfileprefix").unwrap_or("")
    );
    match fragment {
        Some(fragment) => Xref {
            target: format!("{output}#{fragment}"),
            refid: Some(format!("{name}#{fragment}")),
            path: Some(output),
            fragment: Some(fragment),
            ..Default::default()
        },
        None => Xref {
            target: output.clone(),
            refid: Some(name),
            path: Some(output),
            ..Default::default()
        },
    }
}

/// Whether an unknown natural id resembles reftext (Ruby: it contains a
/// space or an uppercase character).
pub(crate) fn looks_like_text(fragment: &str) -> bool {
    fragment.contains(' ') || fragment.chars().any(|c| c.to_lowercase().ne(Some(c)))
}

/// Point a natural reference at the id its text resolved to.
pub(crate) fn retarget(xref: &mut Xref, id: &str, lexical: bool) {
    let target = if lexical { protect(id) } else { id.to_owned() };
    xref.target = format!("#{target}");
    xref.refid = Some(target.clone());
    xref.fragment = Some(target);
}

/// The lexical spelling of a decoded value.
pub(crate) fn protect(value: &str) -> String {
    crate::attributes::reference_text(&[AttributePart::Text(value.to_owned())])
}

/// A label's converted spelling, for comparison with a natural reference's
/// lexical text as Ruby compares `xreftext`. Formatting uses its tags; a
/// label with links, references or anchors cannot be written as a refid.
pub(crate) fn lexical_label(nodes: &[Inline]) -> Option<String> {
    fn plain(nodes: &[Inline]) -> bool {
        nodes.iter().all(|node| match node {
            Inline::Link { .. } | Inline::CrossReference { .. } | Inline::Anchor { .. } => false,
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Monospace { children, .. }
            | Inline::Mark { children, .. }
            | Inline::Superscript { children, .. }
            | Inline::Subscript { children, .. } => plain(children),
            Inline::Substituted { parts, .. } | Inline::AttributeRef { parts, .. } => {
                !parts.iter().any(|part| {
                    matches!(
                        part,
                        AttributePart::OpenLink(_)
                            | AttributePart::OpenXref(_)
                            | AttributePart::Anchor(_)
                    )
                })
            }
            _ => true,
        })
    }
    plain(nodes).then(|| crate::attributes::reference_text(&crate::resolve::label_parts(nodes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xref(refid: &str, macro_form: bool) -> Xref {
        let mut attributes = Attributes::new();
        attributes.set("docname", "main");
        let includes = Includes::from([("ch/full".into(), true), ("part".into(), false)]);
        resolve(
            refid,
            macro_form,
            false,
            &attributes,
            Some(Scope {
                includes: &includes,
                outfilesuffix: Some(".html"),
            }),
        )
    }

    #[test]
    fn interdocument_paths_follow_the_macro_form() {
        let x = xref("other.adoc#frag", false);
        assert_eq!(x.target, "other.html#frag");
        assert_eq!(x.refid.as_deref(), Some("other#frag"));
        assert_eq!(x.path.as_deref(), Some("other.html"));
        assert_eq!(x.fragment.as_deref(), Some("frag"));
        assert_eq!(xref("other.md#x", false).target, "other.md.html#x");
        assert_eq!(xref("other.md#x", true).target, "other.md#x");
        assert_eq!(xref("other.adoc", true).target, "other.html");
        assert_eq!(xref("other.html", true).target, "other.html");
        assert_eq!(xref("other#", true).target, "other.html");
        assert_eq!(xref("dir/x.y/z#a", true).target, "dir/x.y/z.html#a");
        // The angle form never treats a refid without `#` as a path.
        let x = xref("other.adoc", false);
        assert_eq!((x.target.as_str(), x.natural), ("#other.adoc", true));
        let x = xref("other", true);
        assert_eq!((x.target.as_str(), x.path), ("#other", None));
    }

    #[test]
    fn this_document_and_full_includes_stay_internal() {
        let x = xref("main.adoc#top", false);
        assert_eq!(
            (x.target.as_str(), x.path, x.natural),
            ("#top", None, false)
        );
        let x = xref("main.adoc", true);
        assert_eq!((x.target.as_str(), x.refid), ("#", None));
        assert_eq!(xref("ch/full.adoc#s", true).target, "#s");
        assert_eq!(xref("part.adoc#s", true).target, "part.html#s");
        let x = xref("#", false);
        assert_eq!((x.target.as_str(), x.refid.as_deref()), ("#", Some("")));
    }

    #[test]
    fn character_references_do_not_split_paths() {
        assert_eq!(xref("a&#38;b", true).target, "#a&#38;b");
        assert_eq!(xref("#x&", false).target, "##x&");
    }

    #[test]
    fn prefix_and_suffix_attributes_apply_to_source_paths() {
        let mut attributes = Attributes::new();
        attributes.set("relfileprefix", "../");
        attributes.set("relfilesuffix", "/");
        let x = resolve("other.adoc#x", false, false, &attributes, None);
        assert_eq!(x.target, "../other/#x");
        let x = resolve("file.pdf", true, false, &attributes, None);
        assert_eq!(x.target, "../file.pdf");
        // Compat mode keeps the whole refid as an id.
        let x = resolve("other.adoc#x", false, true, &attributes, None);
        assert_eq!((x.target.as_str(), x.natural), ("#other.adoc#x", false));
    }

    #[test]
    fn include_keys_are_relative_to_the_document_directory() {
        assert_eq!(include_key("/doc/main.adoc", "/doc/ch/a.adoc"), "ch/a");
        assert_eq!(include_key("/doc/main.adoc", "/lib/a.adoc"), "../lib/a");
        let mut includes = Includes::new();
        register_include(&mut includes, "a".into(), false);
        register_include(&mut includes, "a".into(), true);
        register_include(&mut includes, "a".into(), false);
        assert_eq!(includes.get("a"), Some(&true));
    }

    #[test]
    fn natural_text_detection() {
        assert!(looks_like_text("Section One"));
        assert!(looks_like_text("Title"));
        assert!(!looks_like_text("section_one"));
    }
}
