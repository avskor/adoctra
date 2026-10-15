//! Temporary lexical text for ordered substitutions. Protection, node ownership
//! and source ranges travel with fragments; HTML escaping stays in the backend.
use crate::{AttributePart, Inline, Span};
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone)]
struct Boundary {
    original: Arc<AttributePart>,
    range: Range<usize>,
}

impl Boundary {
    fn slice(&self, range: Range<usize>, length: usize) -> Option<Self> {
        let selected = if range == (0..length) {
            self.range.clone()
        } else if self.range.len() == length {
            self.range.start + range.start..self.range.start + range.end
        } else {
            return None;
        };
        Some(Self {
            original: self.original.clone(),
            range: selected,
        })
    }
}

pub(crate) struct Fragment {
    pub text: String,
    pub decoded: Option<String>,
    pub raw: bool,
    pub owner: usize,
    pub span: Span,
    pub part: Option<AttributePart>,
    boundary: Option<Boundary>,
    exact: bool,
    extend_span: bool,
}

#[derive(Default)]
pub(crate) struct Text {
    pub text: String,
    pub fragments: Vec<(Range<usize>, Fragment)>,
}

impl Text {
    /// A source text node may already have undergone replacements. Its byte
    /// mapping is then coarse, but a macro starting here still covers later
    /// source constructs it consumes.
    pub fn append_source(&mut self, text: &str, raw: bool, owner: usize, span: Span) {
        let first = self.fragments.len();
        self.append(text, raw, owner, span, span.end - span.start == text.len());
        for (_, fragment) in &mut self.fragments[first..] {
            fragment.extend_span = true;
        }
    }

    pub fn append_part(&mut self, part: &AttributePart, owner: usize, span: Span) {
        if part.is_boundary() {
            let lexical = part.lexical().into_owned();
            let boundary = matches!(
                part,
                AttributePart::OpenWithAttributes { .. } | AttributePart::OpenLink(_)
            )
            .then(|| Boundary {
                original: Arc::new(part.clone()),
                range: 0..lexical.len(),
            });
            self.push(Fragment {
                text: lexical,
                decoded: None,
                raw: true,
                owner,
                span,
                part: Some(part.clone()),
                boundary,
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

    pub fn into_fragments(mut self) -> Vec<Fragment> {
        self.restore_boundaries();
        self.fragments
            .into_iter()
            .map(|(_, fragment)| fragment)
            .collect()
    }

    pub fn into_parts(mut self) -> Vec<AttributePart> {
        self.restore_boundaries();
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
                boundary: None,
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
            boundary: None,
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
                boundary: f
                    .boundary
                    .as_ref()
                    .and_then(|b| b.slice(start..end, f.text.len())),
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
        let boundary = f.boundary.as_ref().and_then(|b| {
            let mut selected = b.slice(
                range.start - r.start..range.end.min(r.end) - r.start,
                f.text.len(),
            )?;
            for (next_range, next) in &self.fragments[first + 1..] {
                if next_range.start >= range.end {
                    break;
                }
                let next_boundary = next.boundary.as_ref()?.slice(
                    0..range.end.min(next_range.end) - next_range.start,
                    next.text.len(),
                )?;
                if !Arc::ptr_eq(&selected.original, &next_boundary.original)
                    || selected.range.end > next_boundary.range.start
                {
                    return None;
                }
                selected.range.end = next_boundary.range.end;
            }
            Some(selected)
        });
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
            boundary,
            exact: false,
            extend_span: f.extend_span,
        });
    }

    /// Physical coverage of a match, using the same ownership rules as replacements.
    pub fn match_span(&self, range: Range<usize>) -> Span {
        let mut anchor = Self::default();
        self.replacement(range, " ", None, &mut anchor);
        anchor.fragments[0].1.span
    }

    /// Anchor a generated semantic value to its first source owner. References
    /// consumed later in the range are retained as empty nodes by write_back.
    pub fn replace_parts(&self, range: Range<usize>, parts: &[AttributePart], output: &mut Self) {
        let mut anchor = Self::default();
        self.replacement(range, " ", None, &mut anchor);
        let (_, fragment) = &anchor.fragments[0];
        for part in parts {
            output.append_part(part, fragment.owner, fragment.span);
        }
    }

    /// Reassemble only boundaries we created, after replacements edit their
    /// lexical id/role values. Arbitrary raw tags never become semantic nodes.
    fn restore_boundaries(&mut self) {
        let mut pending = std::mem::take(&mut self.fragments).into_iter().peekable();
        while let Some((range, mut fragment)) = pending.next() {
            if fragment.part.is_some() {
                self.fragments.push((range, fragment));
                continue;
            }
            let Some(boundary) = fragment.boundary.clone().filter(|b| b.range.start == 0) else {
                self.fragments.push((range, fragment));
                continue;
            };
            let mut group = Vec::new();
            let mut end = boundary.range.end;
            let mut lexical_end = range.end;
            let mut lexical = fragment.text.clone();
            while let Some((_, next)) = pending.peek() {
                if !next.boundary.as_ref().is_some_and(|b| {
                    // Escape removal can leave gaps in the original range.
                    Arc::ptr_eq(&boundary.original, &b.original) && b.range.start >= end
                }) {
                    break;
                }
                let (r, f) = pending.next().unwrap();
                end = f.boundary.as_ref().unwrap().range.end;
                lexical_end = r.end;
                lexical.push_str(&f.text);
                group.push((r, f));
            }
            if end == boundary.original.lexical().len() {
                if let Some(part) = boundary.original.with_lexical_attributes(&lexical) {
                    fragment.text = lexical;
                    fragment.decoded = None;
                    fragment.part = Some(part);
                    fragment.boundary = None;
                    self.fragments.push((range.start..lexical_end, fragment));
                    continue;
                }
            }
            self.fragments.push((range, fragment));
            self.fragments.extend(group);
        }
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
pub(crate) fn write_back(nodes: Vec<Inline>, mut text: Text, fold_newlines: bool) -> Vec<Inline> {
    text.restore_boundaries();
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
            }
            | Inline::Raw {
                span: original_span,
                ..
            }
            | Inline::Substituted {
                span: original_span,
                ..
            } => {
                if pieces.iter().any(|f| f.part.is_some()) {
                    // Keep generated constructs separate from their surrounding
                    // source text so direct links have precise physical spans.
                    let mut pieces = pieces.into_iter().peekable();
                    while let Some(first) = pieces.next() {
                        let span = first.span;
                        let mut group = vec![first];
                        while pieces.peek().is_some_and(|f| f.span == span) {
                            group.push(pieces.next().unwrap());
                        }
                        let mut parts = Vec::new();
                        let semantic = group.iter().any(|f| f.part.is_some());
                        for f in group {
                            if let Some(part) = f.part {
                                crate::attributes::push_value(&mut parts, part);
                            } else {
                                crate::attributes::push_part(&mut parts, &f.text, f.raw);
                            }
                        }
                        if semantic {
                            result.push(Inline::Substituted {
                                value: crate::attributes::parts_text(&parts),
                                parts,
                                span,
                            });
                        } else {
                            for part in parts {
                                result.push(if part.is_raw() {
                                    Inline::Raw {
                                        value: part.text().into(),
                                        span,
                                    }
                                } else {
                                    Inline::Text {
                                        value: part.text().into(),
                                        span,
                                    }
                                });
                            }
                        }
                    }
                    continue;
                }
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
