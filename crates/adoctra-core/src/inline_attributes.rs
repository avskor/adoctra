//! The ordinary a-stage, after passthrough extraction and quotes. References
//! retain source ownership even when a missing-value policy removes their line.
use crate::attributes::{parts_text, push_value};
use crate::substitution_text::Text;
use crate::{AttributePart as P, Attributes, Diagnostic, Inline, Severity, Span};

pub(crate) struct Reference {
    pub span: Span,
    pub name: String,
    pub missing: bool,
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
    attrs: &Attributes,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Text, Vec<Reference>) {
    let mut output = Text::default();
    let mut refs = Vec::new();
    let mut dropped = Vec::new();
    let policy = attrs.get("attribute-missing").unwrap_or("skip");
    let mut at = 0;
    let mut copied = 0;
    while at < text.text.len() {
        let Some((end, name, escaped)) = reference(&text.text, at) else {
            at += text.text[at..].chars().next().unwrap().len_utf8();
            continue;
        };
        text.copy(copied..at, &mut output);
        if escaped {
            let literal = format!("{{{name}}}");
            text.replacement(at..end, &literal, Some(&literal), &mut output);
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
            let removed = missing && matches!(policy, "drop" | "drop-line");
            if !metadata {
                refs.push(Reference {
                    span,
                    name: name.into(),
                    missing: missing && !removed,
                });
            }
            if removed {
                dropped.push(output.text.len());
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
        let mut start = 0;
        let mut previous_end = None;
        for line in output.text.split('\n') {
            let end = start + line.len();
            let mut missing = false;
            while positions.peek().is_some_and(|&pos| pos <= end) {
                positions.next();
                missing = true;
            }
            if !missing || (policy != "drop-line" && !line.is_empty()) {
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
