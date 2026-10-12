//! Ordered quote substitutions for whole-value pass attributes. Generated
//! formatting is semantic; only the temporary matching view has reference tags.
use crate::substitution_text::Text;
use crate::{AttributeFormat as Format, AttributePart, Span};

struct Stage {
    open: &'static str,
    close: &'static str,
    format: Option<Format>,
    constrained: bool,
}

pub(crate) fn apply(parts: Vec<AttributePart>, compat: bool) -> Vec<AttributePart> {
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
    let mut text = Text::from_parts(&parts);
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
            );
        }
    }
    text.into_parts()
}

fn word(ch: char) -> bool {
    crate::unicode_word::is_alphanumeric(ch) || crate::unicode_word::is_mark_or_connector(ch)
}

fn space(ch: char) -> bool {
    matches!(ch, ' ' | '\t'..='\r')
}

fn apply_stage(text: Text, stage: Stage, double: bool, compat: bool) -> Text {
    let src = &text.text;
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
    while at < src.len() {
        let escaped = src.as_bytes()[at] == b'\\';
        let open = at + usize::from(escaped);
        if !src[open..].starts_with(stage.open) {
            at += src[at..].chars().next().unwrap().len_utf8();
            continue;
        }
        if stage.constrained && !escaped && open > 0 {
            let (prev_at, prev) = src[..open].char_indices().next_back().unwrap();
            let extra = match stage.format {
                Some(Format::Mark) => prev == '&',
                Some(Format::Monospace) if !compat => matches!(prev, '"' | '\'' | '`'),
                None if !double && !compat => prev == '`',
                _ => false,
            };
            // A constrained regex consumes its preceding character; a previous
            // match cannot donate its closing marker as that character.
            if prev_at < copied || word(prev) || matches!(prev, ';' | ':' | '}') || extra {
                at = open + stage.open.len();
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
        if escaped {
            text.copy(open..end, &mut output);
        } else {
            if let Some(format) = stage.format {
                output.append_part(&AttributePart::Open(format), 0, Span::new(0, 0));
            } else {
                output.append(
                    if double { "&#8220;" } else { "&#8216;" },
                    true,
                    0,
                    Span::new(0, 0),
                    false,
                );
            }
            text.copy(inner..close, &mut output);
            if let Some(format) = stage.format {
                output.append_part(&AttributePart::Close(format), 0, Span::new(0, 0));
            } else {
                output.append(
                    if double { "&#8221;" } else { "&#8217;" },
                    true,
                    0,
                    Span::new(0, 0),
                    false,
                );
            }
        }
        copied = end;
        at = end;
    }
    text.copy(copied..src.len(), &mut output);
    output
}
