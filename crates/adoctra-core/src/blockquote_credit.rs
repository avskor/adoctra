//! Ruby's blockquote credit `split ', ', 2`. Ruby splits the converted
//! markup of the credit line at the first `, ` of its text (raw passthrough
//! text included), and renders `attribution<br><cite>citetitle</cite>`. When
//! the separator lies inside elements, their tags stay open in the
//! attribution and close in the citetitle; a browser then keeps the `cite`
//! inside the innermost element and closes it with that element, so the rest
//! of the line follows the `cite` within the outer elements. This module
//! builds that structure from well-formed parts: the attribution, the
//! citetitle (the innermost element's remainder, inside copies of its
//! elements) and the trailing rest (inside copies of the outer elements).
//! Atomic values (images, footnotes, …) never split.

use crate::{AttributePart, Inline, Span};

/// A split credit line: the attribution, the citetitle and the trailing
/// rest after the `cite`.
pub(crate) struct Credit {
    pub author: Vec<Inline>,
    pub citetitle: Option<Vec<Inline>>,
    pub trailing: Vec<Inline>,
}

pub(crate) fn split(inlines: Vec<Inline>) -> Credit {
    match split_inlines(inlines) {
        Ok(split) => Credit {
            author: split.head,
            citetitle: Some(split.tail),
            trailing: split.after,
        },
        Err(inlines) => Credit {
            author: inlines,
            citetitle: None,
            trailing: Vec::new(),
        },
    }
}

/// A sequence split at the separator: `head`, then `tail` (inside the
/// `cite`), then `after` (after it). `after` is empty unless the separator
/// lies inside an element of the sequence.
struct Split<T> {
    head: Vec<T>,
    tail: Vec<T>,
    after: Vec<T>,
}

/// The converted text of a node that renders as text only.
fn leaf_text(inline: &Inline) -> Option<&str> {
    match inline {
        Inline::Text { value, .. } | Inline::Raw { value, .. } => Some(value),
        Inline::AttributeRef {
            value: Some(value),
            parts,
            ..
        } if parts.is_empty() => Some(value),
        _ => None,
    }
}

/// A byte range of a text leaf.
fn slice(leaf: &Inline, range: std::ops::Range<usize>) -> Inline {
    // Replacements may change the length of the text.
    let narrow = |value: &str, span: &Span| {
        if value.len() == span.end - span.start {
            Span {
                start: span.start + range.start,
                end: span.start + range.end,
                ..*span
            }
        } else {
            *span
        }
    };
    match leaf {
        Inline::Text { value, span } => Inline::Text {
            value: value[range.clone()].to_owned(),
            span: narrow(value, span),
        },
        Inline::Raw { value, span } => Inline::Raw {
            value: value[range.clone()].to_owned(),
            span: narrow(value, span),
        },
        Inline::AttributeRef {
            name,
            value: Some(value),
            span,
            ..
        } => Inline::AttributeRef {
            name: name.clone(),
            value: Some(value[range].to_owned()),
            parts: Vec::new(),
            span: *span,
        },
        other => unreachable!("not a text leaf: {other:?}"),
    }
}

/// Split the pieces of a text run (with their lengths) at the separator
/// `at` of the joined text.
fn cut<T>(
    run: Vec<T>,
    len: impl Fn(&T) -> usize,
    slice: impl Fn(&T, std::ops::Range<usize>) -> T,
    at: usize,
) -> (Vec<T>, Vec<T>) {
    let (mut head, mut tail) = (Vec::new(), Vec::new());
    let mut offset = 0;
    for piece in run {
        let size = len(&piece);
        let (from, to) = (offset, offset + size);
        offset = to;
        if to <= at {
            head.push(piece);
        } else if from >= at + 2 {
            tail.push(piece);
        } else {
            if from < at {
                head.push(slice(&piece, 0..at - from));
            }
            if to > at + 2 {
                tail.push(slice(&piece, (at + 2).max(from) - from..size));
            }
        }
    }
    (head, tail)
}

fn split_inlines(mut inlines: Vec<Inline>) -> Result<Split<Inline>, Vec<Inline>> {
    let mut index = 0;
    while index < inlines.len() {
        // Adjacent text leaves convert to contiguous text.
        let end = (index..inlines.len())
            .find(|&i| leaf_text(&inlines[i]).is_none())
            .unwrap_or(inlines.len());
        if end > index {
            let joined: String = inlines[index..end].iter().filter_map(leaf_text).collect();
            if let Some(at) = joined.find(", ") {
                let rest = inlines.split_off(end);
                let run = inlines.split_off(index);
                let (head, mut tail) =
                    cut(run, |leaf| leaf_text(leaf).map_or(0, str::len), slice, at);
                inlines.extend(head);
                tail.extend(rest);
                return Ok(Split {
                    head: inlines,
                    tail,
                    after: Vec::new(),
                });
            }
            index = end;
            continue;
        }
        if let Some((split, inner)) = split_node(&mut inlines[index]) {
            let rest = inlines.split_off(index + 1);
            inlines.pop();
            inlines.extend(split.head);
            let mut tail = split.tail;
            let mut after = split.after;
            if inner {
                after.extend(rest);
            } else {
                tail.extend(rest);
            }
            return Ok(Split {
                head: inlines,
                tail,
                after,
            });
        }
        index += 1;
    }
    Err(inlines)
}

/// Split inside a node; `true` when the separator lies inside an element
/// (whose remainder alone belongs to the citetitle).
fn split_node(node: &mut Inline) -> Option<(Split<Inline>, bool)> {
    match node {
        Inline::Strong { children, .. }
        | Inline::Emphasis { children, .. }
        | Inline::Monospace { children, .. }
        | Inline::Mark { children, .. }
        | Inline::Superscript { children, .. }
        | Inline::Subscript { children, .. }
        | Inline::Link { children, .. }
        | Inline::CrossReference { children, .. } => {
            match split_inlines(std::mem::take(children)) {
                Ok(split) => {
                    let copy = |children| {
                        let mut copy = node.clone();
                        set_children(&mut copy, children);
                        copy
                    };
                    let after = if split.after.is_empty() {
                        Vec::new()
                    } else {
                        vec![copy(split.after)]
                    };
                    let split = Split {
                        head: vec![copy(split.head)],
                        tail: vec![copy(split.tail)],
                        after,
                    };
                    Some((split, true))
                }
                Err(back) => {
                    *children = back;
                    None
                }
            }
        }
        Inline::Substituted { parts, .. } | Inline::AttributeRef { parts, .. } => {
            let (split, inner) = split_parts(parts)?;
            let copy = |parts| {
                let mut copy = node.clone();
                set_parts(&mut copy, parts);
                copy
            };
            let after = if split.after.is_empty() {
                Vec::new()
            } else {
                vec![copy(split.after)]
            };
            let split = Split {
                head: vec![copy(split.head)],
                tail: vec![copy(split.tail)],
                after,
            };
            Some((split, inner))
        }
        _ => None,
    }
}

fn set_children(node: &mut Inline, value: Vec<Inline>) {
    if let Inline::Strong { children, .. }
    | Inline::Emphasis { children, .. }
    | Inline::Monospace { children, .. }
    | Inline::Mark { children, .. }
    | Inline::Superscript { children, .. }
    | Inline::Subscript { children, .. }
    | Inline::Link { children, .. }
    | Inline::CrossReference { children, .. } = node
    {
        *children = value;
    }
}

fn set_parts(node: &mut Inline, value: Vec<AttributePart>) {
    let text = crate::attributes::parts_text(&value);
    match node {
        Inline::Substituted {
            value: flat, parts, ..
        } => {
            *flat = text;
            *parts = value;
        }
        Inline::AttributeRef {
            value: flat, parts, ..
        } => {
            *flat = Some(text);
            *parts = value;
        }
        _ => {}
    }
}

fn is_open(part: &AttributePart) -> bool {
    matches!(
        part,
        AttributePart::Open(_)
            | AttributePart::OpenWithAttributes { .. }
            | AttributePart::OpenLink(_)
            | AttributePart::OpenXref(_)
    )
}

fn is_close(part: &AttributePart) -> bool {
    matches!(
        part,
        AttributePart::Close(_) | AttributePart::CloseLink | AttributePart::CloseXref
    )
}

/// The boundary closing an open one.
fn closing(open: &AttributePart) -> AttributePart {
    match open {
        AttributePart::Open(format) | AttributePart::OpenWithAttributes { format, .. } => {
            AttributePart::Close(*format)
        }
        AttributePart::OpenLink(_) => AttributePart::CloseLink,
        AttributePart::OpenXref(_) => AttributePart::CloseXref,
        other => unreachable!("not an opening boundary: {other:?}"),
    }
}

fn part_text(part: &AttributePart) -> Option<&str> {
    match part {
        AttributePart::Text(text) | AttributePart::Raw(text) => Some(text),
        _ => None,
    }
}

fn slice_part(part: &AttributePart, range: std::ops::Range<usize>) -> AttributePart {
    match part {
        AttributePart::Text(text) => AttributePart::Text(text[range].to_owned()),
        AttributePart::Raw(text) => AttributePart::Raw(text[range].to_owned()),
        other => unreachable!("not a text part: {other:?}"),
    }
}

/// Split semantic parts at the first `, ` of adjacent text parts; `true`
/// when boundaries are open there. The boundaries open at the separator
/// close the head and reopen the tail, which ends with the innermost one;
/// the parts after it follow the `cite` inside the outer boundaries.
fn split_parts(parts: &[AttributePart]) -> Option<(Split<AttributePart>, bool)> {
    let mut open: Vec<&AttributePart> = Vec::new();
    let mut index = 0;
    while index < parts.len() {
        let part = &parts[index];
        if part_text(part).is_none() {
            if is_open(part) {
                open.push(part);
            } else if is_close(part) {
                open.pop();
            }
            index += 1;
            continue;
        }
        let end = (index..parts.len())
            .find(|&i| part_text(&parts[i]).is_none())
            .unwrap_or(parts.len());
        let joined: String = parts[index..end].iter().filter_map(part_text).collect();
        let Some(at) = joined.find(", ") else {
            index = end;
            continue;
        };
        let (cut_head, cut_tail) = cut(
            parts[index..end].to_vec(),
            |part| part_text(part).map_or(0, str::len),
            slice_part,
            at,
        );
        let mut head = parts[..index].to_vec();
        head.extend(cut_head);
        head.extend(open.iter().rev().map(|&part| closing(part)));
        let mut tail: Vec<AttributePart> = open.iter().map(|&part| part.clone()).collect();
        tail.extend(cut_tail);
        let Some(innermost) = open.last() else {
            tail.extend_from_slice(&parts[end..]);
            return Some((
                Split {
                    head,
                    tail,
                    after: Vec::new(),
                },
                false,
            ));
        };
        // The innermost boundary's close ends the `cite`.
        let mut depth = 0usize;
        let close = (end..parts.len()).find(|&i| {
            if is_open(&parts[i]) {
                depth += 1;
            } else if is_close(&parts[i]) {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            false
        });
        let outer = &open[..open.len() - 1];
        let mut after = Vec::new();
        match close {
            Some(close) => {
                tail.extend_from_slice(&parts[end..=close]);
                let rest = &parts[close + 1..];
                if rest.iter().any(|part| !is_close(part)) {
                    after.extend(outer.iter().map(|&part| part.clone()));
                    after.extend_from_slice(rest);
                }
            }
            None => {
                tail.extend_from_slice(&parts[end..]);
                tail.push(closing(innermost));
            }
        }
        tail.extend(outer.iter().rev().map(|&part| closing(part)));
        return Some((Split { head, tail, after }, true));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AttributeFormat;

    fn text(value: &str, start: usize) -> Inline {
        Inline::Text {
            value: value.into(),
            span: Span::new(start, start + value.len()),
        }
    }

    fn strong(children: Vec<Inline>) -> Inline {
        Inline::Strong {
            children,
            span: Span::new(0, 9),
        }
    }

    fn emphasis(children: Vec<Inline>) -> Inline {
        Inline::Emphasis {
            children,
            span: Span::new(0, 20),
        }
    }

    #[test]
    fn splits_text_and_keeps_spans() {
        let credit = split(vec![text("A, B, C", 3)]);
        assert_eq!(credit.author, [text("A", 3)]);
        assert_eq!(credit.citetitle, Some(vec![text("B, C", 6)]));
        assert!(credit.trailing.is_empty());
        let credit = split(vec![text("A B", 0)]);
        assert_eq!(credit.author, [text("A B", 0)]);
        assert_eq!(credit.citetitle, None);
    }

    #[test]
    fn splits_across_adjacent_text_leaves() {
        let reference = |value: &str| Inline::AttributeRef {
            name: "x".into(),
            value: Some(value.into()),
            parts: Vec::new(),
            span: Span::new(2, 5),
        };
        let credit = split(vec![text("A,", 0), reference(""), reference(" B")]);
        assert_eq!(credit.author, [text("A", 0)]);
        assert_eq!(credit.citetitle, Some(vec![reference("B")]));
        // Markup between a comma and a space is no separator.
        let credit = split(vec![text("A,", 0), strong(vec![text(" B", 2)])]);
        assert_eq!(credit.citetitle, None);
    }

    #[test]
    fn the_cite_closes_with_the_innermost_element() {
        // `_x *A, B* y_ z`: the browser keeps `B` in the cite and the rest
        // after it, inside the emphasis.
        let credit = split(vec![
            emphasis(vec![
                text("x ", 1),
                strong(vec![text("A, B", 4)]),
                text(" y", 9),
            ]),
            text(" z", 12),
        ]);
        assert_eq!(
            credit.author,
            [emphasis(vec![text("x ", 1), strong(vec![text("A", 4)])])]
        );
        assert_eq!(
            credit.citetitle,
            Some(vec![emphasis(vec![strong(vec![text("B", 7)])])])
        );
        assert_eq!(
            credit.trailing,
            [emphasis(vec![text(" y", 9)]), text(" z", 12)]
        );
    }

    #[test]
    fn splits_semantic_parts_and_reopens_boundaries() {
        use AttributePart::{Close, Open, Raw, Text};
        let node = |parts: Vec<AttributePart>| Inline::Substituted {
            value: crate::attributes::parts_text(&parts),
            parts,
            span: Span::new(0, 8),
        };
        let strong = AttributeFormat::Strong;
        let em = AttributeFormat::Emphasis;
        let credit = split(vec![
            node(vec![
                Open(em),
                Open(strong),
                Raw("A, ".into()),
                Text("B".into()),
                Close(strong),
                Text(" y".into()),
                Close(em),
            ]),
            text(" z", 9),
        ]);
        assert_eq!(
            credit.author,
            [node(vec![
                Open(em),
                Open(strong),
                Raw("A".into()),
                Close(strong),
                Close(em),
            ])]
        );
        assert_eq!(
            credit.citetitle,
            Some(vec![node(vec![
                Open(em),
                Open(strong),
                Text("B".into()),
                Close(strong),
                Close(em),
            ])])
        );
        assert_eq!(
            credit.trailing,
            [
                node(vec![Open(em), Text(" y".into()), Close(em)]),
                text(" z", 9)
            ]
        );
    }
}
