//! Shared a-stage for ordinary inline text, definitions and selected pass
//! substitutions. Ordinary references retain their original source ownership.
use crate::attributes::{parts_text, push_value};
use crate::substitution_context::Evaluation;
use crate::substitution_text::Text;
use crate::{AttributePart as P, Diagnostic, Inline, Severity, Span};

#[derive(Clone)]
pub(crate) struct Reference {
    pub span: Span,
    pub name: String,
    pub missing: bool,
    /// A missing reference removed by the `drop-line` policy (its line is
    /// dropped).
    pub drops_line: bool,
}

/// Match only ordinary references. Stateful set/counter expressions have a
/// separate lifecycle and are deliberately left literal here.
pub(crate) fn reference(src: &str, at: usize) -> Option<(usize, &str, bool)> {
    let leading = src[at..].starts_with("\\{");
    let open = at + usize::from(leading);
    if src.as_bytes().get(open) != Some(&b'{') {
        return None;
    }
    let mut end = open + 1;
    for (i, ch) in src[end..].char_indices() {
        if crate::unicode_word::is_word(ch) || (i > 0 && ch == '-') {
            continue;
        }
        end += i;
        let trailing = src.as_bytes().get(end) == Some(&b'\\');
        let close = end + usize::from(trailing);
        return (end > open + 1 && src.as_bytes().get(close) == Some(&b'}')).then_some((
            close + 1,
            &src[open + 1..end],
            leading || trailing,
        ));
    }
    None
}

pub(crate) fn apply(
    text: Text,
    attrs: &mut Evaluation<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Text, Vec<Reference>) {
    let policy = attrs.get("attribute-missing").unwrap_or("skip").to_owned();
    apply_with_policy(text, attrs, &policy, false, diagnostics)
}

/// Definitions and selected stages carry an enclosing physical origin: their
/// text may already have been joined, unescaped or rewritten by another stage.
pub(crate) fn parts(
    parts: &[P],
    attrs: &mut Evaluation<'_>,
    span: Span,
    policy: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<P> {
    let mut text = Text::default();
    for part in parts {
        text.append_part(part, 0, span);
    }
    apply_with_policy(text, attrs, policy, false, diagnostics)
        .0
        .into_parts()
}

/// The implicit doctitle uses an explicit skip policy even if an expression
/// changes attribute-missing while this early a-stage is running.
pub(crate) fn implicit_title(
    value: &str,
    attrs: &mut Evaluation<'_>,
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<P> {
    let mut text = Text::default();
    text.append_part(&P::Text(value.into()), 0, span);
    apply_with_policy(text, attrs, "skip", true, diagnostics)
        .0
        .into_parts()
}

fn apply_with_policy(
    text: Text,
    attrs: &mut Evaluation<'_>,
    policy: &str,
    fixed_policy: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Text, Vec<Reference>) {
    let mut output = Text::default();
    let mut refs = Vec::new();
    let mut dropped = Vec::new();
    let mut dropped_lines = Vec::new();
    let closes: Vec<_> = text.text.match_indices('}').map(|(i, _)| i).collect();
    let mut at = 0;
    let mut copied = 0;
    let mut missing_policy = None;
    let mut undefined_policy = None;
    while at < text.text.len() {
        let stateful = attrs
            .stateful()
            .then(|| crate::substitution_context::expression(&text.text, at, &closes))
            .flatten();
        let Some((end, name, escaped)) = stateful.or_else(|| reference(&text.text, at)) else {
            at += text.text[at..].chars().next().unwrap().len_utf8();
            continue;
        };
        text.copy(copied..at, &mut output);
        if escaped {
            let literal = format!("{{{name}}}");
            text.replacement(at..end, &literal, Some(&literal), &mut output);
        } else if stateful.is_some() {
            let span = text.match_span(at..end);
            let effect = attrs.execute(name, span, &mut undefined_policy, diagnostics);
            if effect.remove_empty {
                dropped.push(output.text.len());
            }
            if effect.remove_line {
                dropped_lines.push(output.text.len());
            }
            let metadata = text.fragments[text.fragments.partition_point(|(r, _)| r.end <= at)]
                .1
                .part
                .as_ref()
                .is_some_and(P::is_boundary);
            if metadata {
                text.replacement(
                    at..end,
                    &crate::attributes::reference_text(&effect.parts),
                    None,
                    &mut output,
                );
            } else {
                refs.push(Reference {
                    span,
                    name: name.into(),
                    missing: false,
                    drops_line: false,
                });
                for part in &effect.parts {
                    output.append_part(part, refs.len(), span);
                }
            }
        } else {
            let span = text.match_span(at..end);
            let metadata = text.fragments[text.fragments.partition_point(|(r, _)| r.end <= at)]
                .1
                .part
                .as_ref()
                .is_some_and(P::is_boundary);
            let parts = attrs.value_parts(name).or_else(|| {
                crate::context::intrinsic(&name.to_lowercase())
                    .map(|value| vec![P::Raw(value.into())])
            });
            let missing = parts.is_none();
            let policy = if missing && attrs.stateful() && !fixed_policy {
                missing_policy
                    .get_or_insert_with(|| {
                        attrs.get("attribute-missing").unwrap_or("skip").to_owned()
                    })
                    .as_str()
            } else {
                policy
            };
            let removed = missing && matches!(policy, "drop" | "drop-line");
            if !metadata {
                refs.push(Reference {
                    span,
                    name: name.into(),
                    missing: missing && !removed,
                    drops_line: removed && policy == "drop-line",
                });
            }
            if removed {
                dropped.push(output.text.len());
                if policy == "drop-line" {
                    dropped_lines.push(output.text.len());
                }
            }
            if missing && matches!(policy, "warn" | "drop-line") {
                diagnostics.push(Diagnostic {
                    span,
                    severity: if policy == "warn" {
                        Severity::Warning
                    } else {
                        Severity::Info
                    },
                    message: format!(
                        "{} reference to missing attribute: {}",
                        if policy == "warn" {
                            "skipping"
                        } else {
                            "dropping line containing"
                        },
                        name.to_lowercase()
                    ),
                });
            }
            let parts = parts.unwrap_or_else(|| {
                if removed {
                    Vec::new()
                } else {
                    vec![P::Text(text.text[at..end].into())]
                }
            });
            if metadata {
                text.replacement(
                    at..end,
                    &crate::attributes::reference_text(&parts),
                    None,
                    &mut output,
                );
            } else {
                for part in &parts {
                    output.append_part(part, refs.len(), span);
                }
            }
        }
        copied = end;
        at = end;
    }
    text.copy(copied..text.text.len(), &mut output);
    if !dropped.is_empty() {
        let mut retained = Text::default();
        let mut positions = dropped.into_iter().peekable();
        let mut line_positions = dropped_lines.into_iter().peekable();
        let mut start = 0;
        let mut previous_end = None;
        for line in output.text.split('\n') {
            let end = start + line.len();
            let mut missing = false;
            while positions.peek().is_some_and(|&pos| pos <= end) {
                positions.next();
                missing = true;
            }
            let mut drop_line = false;
            while line_positions.peek().is_some_and(|&pos| pos <= end) {
                line_positions.next();
                drop_line = true;
            }
            if !drop_line && (!missing || !line.is_empty()) {
                if let Some(end) = previous_end {
                    output.copy(end..end + 1, &mut retained);
                }
                output.copy(start..end, &mut retained);
                previous_end = Some(end);
            }
            start = end + 1;
        }
        output = retained;
    }
    (output, refs)
}

/// Turn the lexical a-stage result into source edits. Empty references remain
/// addressable by editors; a deleted unresolved reference becomes Some("").
pub(crate) fn nodes(text: Text, refs: Vec<Reference>) -> Vec<Inline> {
    let mut parts = vec![Vec::new(); refs.len()];
    let mut nodes = Vec::new();
    for f in text.into_fragments() {
        let part = f.part.unwrap_or_else(|| {
            let value = f.decoded.unwrap_or(f.text);
            if f.raw {
                P::Raw(value)
            } else {
                P::Text(value)
            }
        });
        if f.owner > 0 {
            push_value(&mut parts[f.owner - 1], part);
        } else {
            nodes.push(match part {
                P::Text(value) => Inline::Text {
                    value,
                    span: f.span,
                },
                P::Raw(value) => Inline::Raw {
                    value,
                    span: f.span,
                },
                part => Inline::Substituted {
                    value: String::new(),
                    parts: vec![part],
                    span: f.span,
                },
            });
        }
    }
    for (r, mut parts) in refs.into_iter().zip(parts) {
        let value = if r.missing && !parts.is_empty() {
            None
        } else {
            Some(parts_text(&parts))
        };
        if value.is_none() || !parts.iter().any(P::is_special) {
            parts.clear();
        }
        nodes.push(Inline::AttributeRef {
            name: r.name,
            value,
            parts,
            span: r.span,
        });
    }
    nodes.sort_by_key(|n| n.span().start);
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Attributes;

    #[test]
    fn shared_stage_matches_across_parts_and_drops_the_transformed_line() {
        let mut attrs = Attributes::new();
        attrs.set_parts("name", vec![P::Raw("first\nsecond".into())]);
        let span = Span::new(12, 42);
        let mut diagnostics = Vec::new();
        let output = parts(
            &[
                P::Text("before {na".into()),
                P::Raw("me} {missing}\nafter".into()),
            ],
            &mut Evaluation::read_only(&attrs),
            span,
            "drop-line",
            &mut diagnostics,
        );
        assert_eq!(
            output,
            vec![P::Text("before ".into()), P::Raw("first\nafter".into())]
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].span, span);
        assert_eq!(diagnostics[0].severity, Severity::Info);
    }
}
