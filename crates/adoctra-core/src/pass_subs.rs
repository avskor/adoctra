//! Selection and individual stages shared by attribute and inline pass macros.
use crate::attributes::reference_text;
use crate::{AttributePart as P, Attributes, Diagnostic, Severity, Span};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Specials,
    Quotes,
    Attributes,
    Replacements,
    Macros,
    Post,
}

pub(crate) fn syntax(subs: &str) -> bool {
    subs.is_empty()
        || subs.split(',').enumerate().all(|(i, name)| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || i > 0 && c == b'-')
        })
}

pub(crate) fn resolve(subs: &str, span: Span, diagnostics: &mut Vec<Diagnostic>) -> Vec<Stage> {
    use Stage::*;
    let mut candidates = Vec::new();
    for name in subs.split(',').filter(|s| !s.is_empty()) {
        let (remove, name) = name.strip_prefix('-').map_or((false, name), |s| (true, s));
        let expanded: &[&str] = match name {
            "normal" | "n" => &[
                "specialcharacters",
                "quotes",
                "attributes",
                "replacements",
                "macros",
                "post_replacements",
            ],
            "verbatim" | "v" | "specialchars" | "c" => &["specialcharacters"],
            "none" => &[],
            "q" => &["quotes"],
            "a" => &["attributes"],
            "r" => &["replacements"],
            "m" => &["macros"],
            "p" => &["post_replacements"],
            _ => std::slice::from_ref(&name),
        };
        if remove {
            candidates.retain(|s| !expanded.contains(s));
        } else {
            candidates.extend_from_slice(expanded);
        }
    }
    let mut stages = Vec::new();
    let mut invalid = Vec::new();
    for name in candidates {
        let stage = match name {
            "specialcharacters" => Specials,
            "quotes" => Quotes,
            "attributes" => Attributes,
            "replacements" => Replacements,
            "macros" => Macros,
            "post_replacements" => Post,
            _ => {
                invalid.push(name);
                continue;
            }
        };
        if !stages.contains(&stage) {
            stages.push(stage);
        }
    }
    if !invalid.is_empty() {
        diagnostics.push(Diagnostic {
            span,
            severity: Severity::Warning,
            message: format!(
                "invalid substitution type{} for passthrough macro: {}",
                if invalid.len() > 1 { "s" } else { "" },
                invalid.join(", ")
            ),
        });
    }
    stages
}

pub(crate) fn apply(
    parts: Vec<P>,
    stage: Stage,
    attrs: &Attributes,
    compat: bool,
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<P> {
    match stage {
        Stage::Specials => vec![P::Text(reference_text(&parts))],
        Stage::Quotes => crate::attribute_quotes::apply(parts, compat, attrs),
        Stage::Replacements => crate::replacements::apply_parts(parts),
        Stage::Macros => crate::attribute_macros::apply(parts, attrs, compat),
        Stage::Attributes => crate::inline_attributes::parts(
            &parts,
            attrs,
            span,
            attrs.get("attribute-missing").unwrap_or("skip"),
            diagnostics,
        ),
        Stage::Post => crate::post_replacements::apply(
            crate::substitution_text::Text::from_parts(&parts),
            attrs.hardbreaks(),
            true,
        )
        .into_parts(),
    }
}
