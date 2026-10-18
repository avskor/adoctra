//! Scanner helpers: split source into lines while tracking byte offsets.
//!
//! Byte offsets (not line/column) are what the parser and tokenizer attach to
//! nodes; a JS editor like CodeMirror/Monaco converts offsets to positions on
//! its own, and offsets are cheaper to carry around.

/// Yield `(byte_offset, line_without_eol)` for each line of `source`.
pub(crate) fn lines_with_offsets(source: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for raw in source.split_inclusive('\n') {
        let full_len = raw.len();
        let content = raw.strip_suffix('\n').unwrap_or(raw);
        let content = content.strip_suffix('\r').unwrap_or(content);
        out.push((start, content));
        start += full_len;
    }
    out
}

/// Retained text fragments with a piecewise map back to the original buffer. A span across a gap encloses the removed bytes; a span on either side
/// maps to that side. No per-byte table: generated text (verbatim padding or
/// expanded tabs) is one segment pinned to the source bytes it replaces.
#[derive(Clone)]
pub(crate) struct FilteredSource {
    pub text: String,
    pub removed: Vec<crate::Span>,
    segments: Vec<Segment>,
}

/// A run of the transformed text starting at `start` that maps to `original`.
/// Generated text maps every position to `original`; a span ending in it ends
/// after the `width` source bytes it replaces (zero for padding).
#[derive(Clone, Copy)]
struct Segment {
    start: usize,
    original: usize,
    generated: Option<usize>,
}

/// A piece of a [`FilteredSource::remap`] result: a range of the source's
/// text, or generated text standing for the source range `at..at + width`.
pub(crate) enum Piece {
    Copy(std::ops::Range<usize>),
    Generated {
        text: String,
        at: usize,
        width: usize,
    },
}

impl FilteredSource {
    pub fn from_fragments<'a>(fragments: impl IntoIterator<Item = (usize, &'a str)>) -> Self {
        let mut result = Self {
            text: String::new(),
            removed: Vec::new(),
            segments: Vec::new(),
        };
        for (offset, text) in fragments {
            if !text.is_empty() {
                result.segments.push(Segment {
                    start: result.text.len(),
                    original: offset,
                    generated: None,
                });
                result.text.push_str(text);
            }
        }
        result
    }

    pub fn restore_inlines(&self, inlines: &mut [crate::Inline]) {
        struct Restore<'a>(&'a FilteredSource);
        impl crate::visit::VisitorMut for Restore<'_> {
            fn visit_span(&mut self, span: &mut crate::Span) {
                *span = self.0.original_span(*span);
            }
        }
        crate::visit::walk_inlines_mut(&mut Restore(self), inlines);
    }

    /// Retain the first line: a slice may begin after a list/admonition prefix.
    pub fn without_lines(src: &str, base: usize, remove: impl Fn(&str) -> bool) -> Self {
        let mut result = Self {
            text: String::new(),
            removed: Vec::new(),
            segments: Vec::new(),
        };
        let mut off = base;
        for (index, raw) in src.split_inclusive('\n').enumerate() {
            let line = raw.trim_end_matches(['\n', '\r']);
            if index > 0 && remove(line) {
                result.removed.push(crate::Span::new(off, off + line.len()));
            } else {
                result.segments.push(Segment {
                    start: result.text.len(),
                    original: off,
                    generated: None,
                });
                result.text.push_str(raw);
            }
            off += raw.len();
        }
        // Removing trailing comments must not add a final soft line break.
        let len = result.text.trim_end_matches(['\n', '\r']).len();
        result.text.truncate(len);
        result
    }

    /// A new text assembled from `pieces` of this one, keeping the map back
    /// to the original buffer (generated pieces pin to the bytes they stand for).
    pub fn remap(&self, pieces: &[Piece]) -> Self {
        let mut result = Self {
            text: String::new(),
            removed: self.removed.clone(),
            segments: Vec::new(),
        };
        for piece in pieces {
            match piece {
                Piece::Copy(range) if range.is_empty() => {}
                Piece::Copy(range) => {
                    if self.segments.is_empty() {
                        result.segments.push(Segment {
                            start: result.text.len(),
                            original: range.start,
                            generated: None,
                        });
                    } else {
                        let first = self
                            .segments
                            .partition_point(|s| s.start <= range.start)
                            .saturating_sub(1);
                        for segment in self.segments[first..]
                            .iter()
                            .take_while(|s| s.start < range.end)
                        {
                            let from = range.start.max(segment.start);
                            result.segments.push(Segment {
                                start: result.text.len() + from - range.start,
                                original: match segment.generated {
                                    Some(_) => segment.original,
                                    None => segment.original + from - segment.start,
                                },
                                generated: segment.generated,
                            });
                        }
                    }
                    result.text.push_str(&self.text[range.clone()]);
                }
                Piece::Generated { text, .. } if text.is_empty() => {}
                Piece::Generated { text, at, width } => {
                    let original = self.original_span(crate::Span::new(*at, at + width));
                    result.segments.push(Segment {
                        start: result.text.len(),
                        original: original.start,
                        generated: Some(original.end - original.start),
                    });
                    result.text.push_str(text);
                }
            }
        }
        result
    }

    /// The transformed end of segment `index`.
    fn segment_end(&self, index: usize) -> usize {
        self.segments
            .get(index + 1)
            .map_or(self.text.len(), |s| s.start)
    }

    /// Map retained source trivia into the logical buffer for highlighting.
    /// Callers supply spans produced from these same retained fragments.
    pub fn transformed_span(&self, span: crate::Span) -> crate::Span {
        if self.segments.is_empty() {
            return span;
        }
        let start_idx = self
            .segments
            .partition_point(|s| s.original <= span.start)
            .saturating_sub(1);
        let end_idx = self
            .segments
            .partition_point(|s| s.original < span.end)
            .saturating_sub(1);
        let (s, e) = (self.segments[start_idx], self.segments[end_idx]);
        let start = match s.generated {
            Some(_) => s.start,
            None => s.start + span.start - s.original,
        };
        let end = match e.generated {
            Some(_) => self.segment_end(end_idx),
            None => e.start + span.end - e.original,
        };
        crate::Span::new(start, end.max(start))
    }

    pub fn original_span(&self, span: crate::Span) -> crate::Span {
        if self.segments.is_empty() {
            return span;
        }
        let start_idx = self
            .segments
            .partition_point(|s| s.start <= span.start)
            .saturating_sub(1);
        let s = self.segments[start_idx];
        let start = match s.generated {
            Some(_) => s.original,
            None => s.original + span.start - s.start,
        };
        if span.start == span.end {
            return crate::Span::new(start, start);
        }
        let end_idx = self
            .segments
            .partition_point(|s| s.start < span.end)
            .saturating_sub(1);
        let e = self.segments[end_idx];
        let end = match e.generated {
            Some(width) => e.original + width,
            None => e.original + span.end - e.start,
        };
        crate::Span::new(start, end.max(start))
    }
}
