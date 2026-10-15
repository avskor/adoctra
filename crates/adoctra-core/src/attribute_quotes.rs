//! Ordered quote substitutions shared by inline source and whole-value pass attributes. Generated
//! formatting is semantic; only the temporary matching view has reference tags.
use crate::substitution_text::Text;
use crate::{AttributeFormat as Format, AttributePart, Attributes, Span};

struct Stage {
    open: &'static str,
    close: &'static str,
    format: Option<Format>,
    constrained: bool,
}

pub(crate) fn apply(
    parts: Vec<AttributePart>,
    compat: bool,
    attributes: &Attributes,
) -> Vec<AttributePart> {
    apply_text(
        Text::from_parts(&parts),
        compat,
        attributes,
        &mut Vec::new(),
        false,
    )
    .into_parts()
}

#[derive(Clone)]
pub(crate) struct Quote {
    pub span: Span,
    pub format: Option<Format>,
    pub decorated: bool,
}

pub(crate) fn apply_text(
    mut text: Text,
    compat: bool,
    attributes: &Attributes,
    quotes: &mut Vec<Quote>,
    source: bool,
) -> Text {
    use Format::*;
    let stages = [
        ("**", "**", Some(Strong), false),
        ("*", "*", Some(Strong), true),
        (
            if compat { "``" } else { "\"`" },
            if compat { "''" } else { "`\"" },
            None,
            true,
        ),
        (if compat { "'" } else { "" }, "'", Some(Emphasis), true),
        (
            if compat { "`" } else { "'`" },
            if compat { "'" } else { "`'" },
            None,
            true,
        ),
        (
            if compat { "++" } else { "``" },
            if compat { "++" } else { "``" },
            Some(Monospace),
            false,
        ),
        (
            if compat { "+" } else { "`" },
            if compat { "+" } else { "`" },
            Some(Monospace),
            true,
        ),
        ("__", "__", Some(Emphasis), false),
        ("_", "_", Some(Emphasis), true),
        ("##", "##", Some(Mark), false),
        ("#", "#", Some(Mark), true),
        ("^", "^", Some(Superscript), false),
        ("~", "~", Some(Subscript), false),
    ];
    for (index, (open, close, format, constrained)) in stages.into_iter().enumerate() {
        if !open.is_empty() {
            text = apply_stage(
                text,
                Stage {
                    open,
                    close,
                    format,
                    constrained,
                },
                index == 2,
                compat,
                attributes,
                quotes,
                source,
            );
        }
    }
    text
}

fn word(ch: char) -> bool {
    crate::unicode_word::is_alphanumeric(ch) || crate::unicode_word::is_mark_or_connector(ch)
}

fn space(ch: char) -> bool {
    matches!(ch, ' ' | '\t'..='\r')
}

fn apply_stage(
    text: Text,
    stage: Stage,
    double: bool,
    compat: bool,
    attributes: &Attributes,
    quotes: &mut Vec<Quote>,
    source: bool,
) -> Text {
    let src = &text.text;
    if !src.contains(stage.open) {
        return text;
    }
    // Index eligible closers once. Repeated unmatched openers ("*a *a …")
    // must not rescan the entire suffix for every marker in an editor buffer.
    let closings: Vec<_> = src
        .char_indices()
        .filter_map(|(at, _)| {
            if !src[at..].starts_with(stage.close) {
                return None;
            }
            if stage.constrained {
                if src[..at].chars().next_back().map_or(true, space) {
                    return None;
                }
                if src[at + stage.close.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| {
                        word(c)
                            || (!compat
                                && stage.format == Some(Format::Monospace)
                                && matches!(c, '"' | '\'' | '`'))
                    })
                {
                    return None;
                }
            }
            Some(at)
        })
        .collect();
    let mut output = Text::default();
    let mut copied = 0;
    let mut at = 0;
    let brackets: Vec<_> = src.match_indices(']').map(|(i, _)| i).collect();
    while at < src.len() {
        let escaped = src.as_bytes()[at] == b'\\';
        let prefix = at + usize::from(escaped);
        let attr_end = if src[prefix..].starts_with('[') {
            brackets
                .get(brackets.partition_point(|&i| i <= prefix))
                .copied()
                .filter(|&end| end > prefix + 1 && src[end + 1..].starts_with(stage.open))
        } else {
            None
        };
        let open = attr_end.map_or(prefix, |end| end + 1);
        if !src[open..].starts_with(stage.open) {
            at += src[at..].chars().next().unwrap().len_utf8();
            continue;
        }
        if stage.constrained && !escaped && prefix > 0 {
            let (prev_at, prev) = src[..prefix].char_indices().next_back().unwrap();
            let extra = match stage.format {
                Some(Format::Mark) => prev == '&',
                Some(Format::Monospace) if !compat => matches!(prev, '"' | '\'' | '`'),
                None if !double && !compat => prev == '`',
                _ => false,
            };
            // A constrained regex consumes its preceding character; a previous
            // match cannot donate its closing marker as that character.
            if prev_at < copied || word(prev) || matches!(prev, ';' | ':' | '}') || extra {
                at += src[at..].chars().next().unwrap().len_utf8();
                continue;
            }
        }
        let inner = open + stage.open.len();
        let Some(first) = src[inner..].chars().next() else {
            break;
        };
        let no_spaces = matches!(stage.format, Some(Format::Superscript | Format::Subscript));
        if (stage.constrained || no_spaces) && space(first) {
            at = inner;
            continue;
        }
        let next = closings.partition_point(|&close| close < inner + first.len_utf8());
        let Some(&close) = closings.get(next) else {
            at = inner;
            continue;
        };
        if no_spaces && src[inner..close].contains(space) {
            at = inner;
            continue;
        }
        let end = close + stage.close.len();
        text.copy(copied..at, &mut output);
        if escaped && (!stage.constrained || attr_end.is_none()) {
            text.copy(prefix..end, &mut output);
        } else {
            if escaped {
                text.copy(prefix..open, &mut output);
            }
            let attrs = attr_end
                .filter(|_| !escaped)
                .map(|end| parse_attributes(&src[prefix + 1..end], attributes));
            let format = if stage.format == Some(Format::Mark) && attrs.is_some() {
                Some(Format::Span)
            } else {
                stage.format
            };
            let (id, role) = attrs.unwrap_or_default();
            let decorated = id.is_some() || role.is_some();
            let wrapper = if decorated {
                Some(format.unwrap_or(Format::Span))
            } else {
                format.filter(|f| *f != Format::Span)
            };
            if let Some(format) = wrapper {
                let part = if decorated {
                    AttributePart::OpenWithAttributes { format, id, role }
                } else {
                    AttributePart::Open(format)
                };
                text.replace_parts(
                    if escaped { open } else { prefix }..inner,
                    &[part],
                    &mut output,
                );
            }
            if stage.format.is_none() {
                text.replacement(
                    open..inner,
                    if double { "&#8220;" } else { "&#8216;" },
                    source.then_some(if double { "“" } else { "‘" }),
                    &mut output,
                );
            }
            text.copy(inner..close, &mut output);
            if stage.format.is_none() {
                text.replacement(
                    close..end,
                    if double { "&#8221;" } else { "&#8217;" },
                    source.then_some(if double { "”" } else { "’" }),
                    &mut output,
                );
            }
            if let Some(format) = wrapper {
                text.replace_parts(close..end, &[AttributePart::Close(format)], &mut output);
            }
            if source {
                quotes.push(Quote {
                    span: text.match_span(if escaped { open } else { prefix }..end),
                    format: wrapper,
                    decorated,
                });
            }
        }
        copied = end;
        at = end;
    }
    text.copy(copied..src.len(), &mut output);
    output
}

/// Quoted text has its own attribute grammar, not the block attribute list.
fn parse_attributes(input: &str, attributes: &Attributes) -> (Option<String>, Option<String>) {
    let expanded;
    let input = if input.contains('{') {
        let mut lexical = attributes.clone();
        for (key, _) in attributes.iter() {
            if let Some(parts) = attributes.raw_parts(key) {
                lexical.set(key, crate::attributes::reference_text(parts));
            } else if let Some(value) = attributes.get(key) {
                lexical.set(
                    key,
                    crate::attributes::reference_text(&[AttributePart::Text(value.into())]),
                );
            }
        }
        expanded = crate::include_attributes::substitute(
            input,
            &lexical,
            attributes.get("attribute-missing").unwrap_or("skip"),
        )
        .0;
        &expanded
    } else {
        input
    };
    let input = input
        .split(',')
        .next()
        .unwrap()
        .trim_matches(|c| matches!(c, ' ' | '\t'..='\r' | '\0'));
    if input.is_empty() {
        return (None, None);
    }
    if !input.starts_with(['.', '#']) {
        return (None, Some(input.into()));
    }
    let (before, after) = input.split_once('#').unwrap_or((input, ""));
    let (id, roles) = after.split_once('.').unwrap_or((after, ""));
    let role = if roles.is_empty() {
        (before.len() > 1).then(|| before.replace('.', " ").trim_start_matches(' ').to_owned())
    } else if before.len() > 1 {
        Some(
            format!("{before}.{roles}")
                .replace('.', " ")
                .trim_start_matches(' ')
                .to_owned(),
        )
    } else {
        Some(roles.replace('.', " "))
    };
    ((!id.is_empty()).then(|| id.to_owned()), role)
}
