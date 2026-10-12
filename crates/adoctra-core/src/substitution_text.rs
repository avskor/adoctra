//! Temporary lexical text for ordered substitutions. Protection, node ownership
//! and source ranges travel with fragments; HTML escaping stays in the backend.
use crate::{AttributePart, Inline, Span};
use std::ops::Range;

pub(crate) struct Fragment {
    pub text: String,
    pub decoded: Option<String>,
    pub raw: bool,
    pub owner: usize,
    pub span: Span,
    pub part: Option<AttributePart>,
    exact: bool,
    extend_span: bool,
}

#[derive(Default)]
pub(crate) struct Text {
    pub text: String,
    pub fragments: Vec<(Range<usize>, Fragment)>,
}

impl Text {
    pub fn append_part(&mut self, part: &AttributePart, owner: usize, span: Span) {
        if matches!(part, AttributePart::Open(_) | AttributePart::Close(_)) {
            self.push(Fragment {
                text: part.lexical().into(),
                decoded: None,
                raw: true,
                owner,
                span,
                part: Some(part.clone()),
                exact: false,
                extend_span: false,
            });
        } else {
            self.append(part.text(), part.is_raw(), owner, span, false);
        }
    }

    pub fn from_parts(parts: &[AttributePart]) -> Self {
        let mut result = Self::default();
        for part in parts {
            result.append_part(part, 0, Span::new(0, 0));
        }
        result
    }

    pub fn into_parts(self) -> Vec<AttributePart> {
        let mut parts = Vec::new();
        for (_, fragment) in self.fragments {
            if let Some(part) = fragment.part {
                crate::attributes::push_value(&mut parts, part);
            } else {
                crate::attributes::push_part(
                    &mut parts,
                    fragment.decoded.as_deref().unwrap_or(&fragment.text),
                    fragment.raw,
                );
            }
        }
        parts
    }

    pub fn push(&mut self, fragment: Fragment) {
        if fragment.text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(&fragment.text);
        self.fragments.push((start..self.text.len(), fragment));
    }

    pub fn append(&mut self, text: &str, raw: bool, owner: usize, span: Span, exact: bool) {
        let mut start = 0;
        for (offset, ch) in text.char_indices() {
            let encoded = match (raw, ch) {
                (false, '&') => "&amp;",
                (false, '<') => "&lt;",
                (false, '>') => "&gt;",
                _ => continue,
            };
            self.literal(
                &text[start..offset],
                raw,
                owner,
                slice_span(span, start..offset, exact),
                exact,
            );
            self.push(Fragment {
                text: encoded.into(),
                decoded: Some(ch.to_string()),
                raw,
                owner,
                span: slice_span(span, offset..offset + ch.len_utf8(), exact),
                part: None,
                exact: false,
                extend_span: exact,
            });
            start = offset + ch.len_utf8();
        }
        self.literal(
            &text[start..],
            raw,
            owner,
            slice_span(span, start..text.len(), exact),
            exact,
        );
    }

    fn literal(&mut self, text: &str, raw: bool, owner: usize, span: Span, exact: bool) {
        self.push(Fragment {
            text: text.into(),
            decoded: None,
            raw,
            owner,
            span,
            part: None,
            exact,
            extend_span: exact,
        });
    }

    pub fn copy(&self, range: Range<usize>, output: &mut Self) {
        let first = self
            .fragments
            .partition_point(|(r, _)| r.end <= range.start);
        for (r, f) in &self.fragments[first..] {
            if r.start >= range.end {
                break;
            }
            let start = range.start.max(r.start) - r.start;
            let end = range.end.min(r.end) - r.start;
            if start != 0 || end != f.text.len() {
                // Matchers consume protected special characters as whole units.
                debug_assert!(f.decoded.is_none());
            }
            // Copy only the selected slice: cloning the complete fragment for
            // every match would make a long run of replacements quadratic.
            output.push(Fragment {
                text: f.text[start..end].into(),
                decoded: f.decoded.clone(),
                raw: f.raw,
                owner: f.owner,
                span: slice_span(f.span, start..end, f.exact),
                part: if start == 0 && end == f.text.len() {
                    f.part.clone()
                } else {
                    None
                },
                exact: f.exact,
                extend_span: f.extend_span,
            });
        }
    }

    pub fn replacement(
        &self,
        range: Range<usize>,
        lexical: &str,
        decoded: Option<&str>,
        output: &mut Self,
    ) {
        let first = self
            .fragments
            .partition_point(|(r, _)| r.end <= range.start);
        let (r, f) = &self.fragments[first];
        let mut span = slice_span(
            f.span,
            range.start - r.start..range.end.min(r.end) - r.start,
            f.exact,
        );
        // AttributeRef keeps its own source token. A replacement starting in
        // ordinary text covers all source bytes it consumes, including refs.
        if f.extend_span {
            for (r, next) in &self.fragments[first + 1..] {
                if r.start >= range.end {
                    break;
                }
                let end = slice_span(next.span, 0..range.end.min(r.end) - r.start, next.exact).end;
                span.end = span.end.max(end);
            }
        }
        output.push(Fragment {
            text: lexical.into(),
            decoded: decoded.map(str::to_owned),
            raw: decoded.is_none(),
            owner: f.owner,
            span,
            part: None,
            exact: false,
            extend_span: f.extend_span,
        });
    }
}

fn slice_span(span: Span, range: Range<usize>, exact: bool) -> Span {
    if exact {
        Span {
            start: span.start + range.start,
            end: span.start + range.end,
            ..span
        }
    } else {
        span
    }
}

/// Restore AST nodes, retaining attribute references even if a replacement
/// consumed their whole value. This keeps editor tokens and provenance stable.
pub(crate) fn write_back(nodes: Vec<Inline>, text: Text, fold_newlines: bool) -> Vec<Inline> {
    let mut fragments = text.fragments.into_iter().peekable();
    let mut result = Vec::new();
    for (owner, mut node) in nodes.into_iter().enumerate() {
        let mut pieces = Vec::new();
        while fragments.peek().is_some_and(|(_, f)| f.owner == owner) {
            let (_, mut f) = fragments.next().unwrap();
            f.text = f.decoded.take().unwrap_or(f.text);
            if fold_newlines && !f.raw {
                f.text = f.text.replace(['\r', '\n'], " ");
            }
            pieces.push(f);
        }
        match &mut node {
            Inline::AttributeRef {
                value: Some(value),
                parts,
                ..
            } => {
                parts.clear();
                for f in pieces {
                    if let Some(part) = f.part {
                        crate::attributes::push_value(parts, part);
                    } else {
                        crate::attributes::push_part(parts, &f.text, f.raw);
                    }
                }
                *value = crate::attributes::parts_text(parts);
                if !parts.iter().any(crate::AttributePart::is_special) {
                    parts.clear();
                }
                result.push(node);
            }
            Inline::Text {
                span: original_span,
                ..
            } => {
                if !pieces.is_empty() && pieces.iter().all(|f| !f.raw) {
                    let end = pieces.iter().map(|f| f.span.end).max().unwrap();
                    result.push(Inline::Text {
                        value: pieces.into_iter().map(|f| f.text).collect(),
                        span: Span {
                            end: original_span.end.max(end),
                            ..*original_span
                        },
                    });
                    continue;
                }
                let start = result.len();
                for f in pieces {
                    if result.len() > start {
                        if let Some(previous) = result.last_mut() {
                            match previous {
                                Inline::Text { value, span } if !f.raw => {
                                    value.push_str(&f.text);
                                    span.end = span.end.max(f.span.end);
                                    continue;
                                }
                                Inline::Raw { value, span } if f.raw => {
                                    value.push_str(&f.text);
                                    span.end = span.end.max(f.span.end);
                                    continue;
                                }
                                _ => {}
                            }
                        }
                    }
                    result.push(if f.raw {
                        Inline::Raw {
                            value: f.text,
                            span: f.span,
                        }
                    } else {
                        Inline::Text {
                            value: f.text,
                            span: f.span,
                        }
                    });
                }
            }
            _ => unreachable!("only resolved text enters the replacement stream"),
        }
    }
    result
}
