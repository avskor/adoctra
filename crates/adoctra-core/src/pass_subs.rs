//! Selection and individual stages shared by attribute and inline pass macros.
use crate::attributes::reference_text;
use crate::substitution_context::Evaluation;
use crate::{AttributePart as P, Diagnostic, Severity, Span};

pub(crate) const MAX_DEPTH: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Specials,
    Quotes,
    Attributes,
    Replacements,
    Macros,
    Post,
    /// Ruby's `sub_callouts` (blocks only).
    Callouts,
}

/// A block's content model decides its default substitutions (Ruby's
/// `commit_subs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Defaults {
    /// `NORMAL_SUBS`: paragraphs, simple content and verse.
    Normal,
    /// `VERBATIM_SUBS`: listing and literal blocks.
    Verbatim,
    /// `BASIC_SUBS`: STEM blocks.
    Basic,
    /// `NO_SUBS`: passthrough blocks.
    None,
}

impl Defaults {
    pub(crate) fn stages(self) -> Vec<Stage> {
        use Stage::*;
        match self {
            Self::Normal => vec![Specials, Quotes, Attributes, Replacements, Macros, Post],
            Self::Verbatim => vec![Specials, Callouts],
            Self::Basic => vec![Specials],
            Self::None => Vec::new(),
        }
    }
}

const NORMAL: &[&str] = &[
    "specialcharacters",
    "quotes",
    "attributes",
    "replacements",
    "macros",
    "post_replacements",
];

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Specials => "specialcharacters",
        Stage::Quotes => "quotes",
        Stage::Attributes => "attributes",
        Stage::Replacements => "replacements",
        Stage::Macros => "macros",
        Stage::Post => "post_replacements",
        Stage::Callouts => "callouts",
    }
}

/// Ruby's `resolve_block_subs`: block selectors differ from inline pass
/// selectors (no one-letter hints, spaces are ignored) and +/- modifiers
/// start with the block's defaults. Invalid names warn with the block's
/// context as the subject.
pub(crate) fn resolve_block(
    subs: &str,
    defaults: Defaults,
    subject: &str,
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Stage> {
    let subs = subs.replace(' ', "");
    let modifiers = subs.contains(['+', '-']);
    let mut candidates: Option<Vec<&str>> = None;
    // Like Ruby's String#split, trailing empty fields are dropped.
    let mut keys: Vec<&str> = subs.split(',').collect();
    while keys.last() == Some(&"") {
        keys.pop();
    }
    for key in keys {
        let (operation, name) = if !modifiers {
            (' ', key)
        } else if let Some(name) = key.strip_prefix('+') {
            ('+', name)
        } else if let Some(name) = key.strip_prefix('-') {
            ('-', name)
        } else if let Some(name) = key.strip_suffix('+') {
            ('^', name)
        } else {
            (' ', key)
        };
        let expanded: &[&str] = match name {
            "normal" => NORMAL,
            "verbatim" => &["specialcharacters", "callouts"],
            "specialchars" => &["specialcharacters"],
            "none" => &[],
            _ => std::slice::from_ref(&name),
        };
        let candidates = candidates.get_or_insert_with(|| {
            if operation == ' ' {
                Vec::new()
            } else {
                defaults.stages().into_iter().map(stage_name).collect()
            }
        });
        match operation {
            '-' => candidates.retain(|name| !expanded.contains(name)),
            '^' => candidates
                .splice(0..0, expanded.iter().copied())
                .for_each(drop),
            _ => candidates.extend_from_slice(expanded),
        }
    }
    let mut stages = Vec::new();
    let mut invalid = Vec::new();
    for name in candidates.unwrap_or_default() {
        let stage = match name {
            "specialcharacters" => Stage::Specials,
            "quotes" => Stage::Quotes,
            "attributes" => Stage::Attributes,
            "replacements" => Stage::Replacements,
            "macros" => Stage::Macros,
            "post_replacements" => Stage::Post,
            "callouts" => Stage::Callouts,
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
            omitted_span: None,
            report: Default::default(),
            span,
            severity: Severity::Warning,
            message: format!(
                "invalid substitution type{} for {subject}: {}",
                if invalid.len() > 1 { "s" } else { "" },
                invalid.join(", ")
            ),
        });
    }
    stages
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
            omitted_span: None,
            report: Default::default(),
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
    attrs: &mut Evaluation<'_>,
    compat: bool,
    span: Span,
    depth: usize,
    passes: &mut crate::inline_pass::Passes,
) -> Vec<P> {
    match stage {
        Stage::Specials => vec![P::Text(reference_text(&parts))],
        Stage::Quotes => {
            crate::attribute_quotes::apply(parts, compat, attrs, span, &mut passes.diagnostics)
        }
        Stage::Replacements => crate::replacements::apply_parts(parts),
        Stage::Macros => crate::attribute_macros::apply(parts, attrs, compat, span, depth, passes),
        Stage::Attributes => {
            let policy = attrs.get("attribute-missing").unwrap_or("skip").to_owned();
            crate::inline_attributes::parts(&parts, attrs, span, &policy, &mut passes.diagnostics)
        }
        Stage::Post => crate::post_replacements::apply(
            crate::substitution_text::Text::from_parts(&parts),
            attrs.hardbreaks(),
            true,
        )
        .into_parts(),
        Stage::Callouts => crate::callouts::apply_parts(
            parts,
            passes.line_comment.as_deref(),
            attrs,
            span,
            &mut passes.diagnostics,
        ),
    }
}
