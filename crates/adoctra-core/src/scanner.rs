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
/// maps to that side. No per-byte table, padding or synthetic source positions.
#[derive(Clone)]
pub(crate) struct FilteredSource {
    pub text: String,
    pub removed: Vec<crate::Span>,
    segments: Vec<(usize, usize)>, // transformed start, original start
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
                result.segments.push((result.text.len(), offset));
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
                result.segments.push((result.text.len(), off));
                result.text.push_str(raw);
            }
            off += raw.len();
        }
        // Removing trailing comments must not add a final soft line break.
        let len = result.text.trim_end_matches(['\n', '\r']).len();
        result.text.truncate(len);
        result
    }

    /// Map retained source trivia into the logical buffer for highlighting.
    /// Callers supply spans produced from these same retained fragments.
    pub fn transformed_span(&self, span: crate::Span) -> crate::Span {
        if self.segments.is_empty() {
            return span;
        }
        let start_idx = self
            .segments
            .partition_point(|(_, original)| *original <= span.start)
            .saturating_sub(1);
        let end_idx = self
            .segments
            .partition_point(|(_, original)| *original < span.end)
            .saturating_sub(1);
        let (s, original_s) = self.segments[start_idx];
        let (e, original_e) = self.segments[end_idx];
        crate::Span::new(s + span.start - original_s, e + span.end - original_e)
    }

    pub fn original_span(&self, span: crate::Span) -> crate::Span {
        if self.segments.is_empty() {
            return span;
        }
        let start_idx = self
            .segments
            .partition_point(|(start, _)| *start <= span.start)
            .saturating_sub(1);
        let end_idx = if span.start == span.end {
            start_idx
        } else {
            self.segments
                .partition_point(|(start, _)| *start < span.end)
                .saturating_sub(1)
        };
        let (s, original_s) = self.segments[start_idx];
        let (e, original_e) = self.segments[end_idx];
        crate::Span::new(original_s + span.start - s, original_e + span.end - e)
    }
}
