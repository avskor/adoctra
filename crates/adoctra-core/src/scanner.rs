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

/// Text after deleting continuation lines, with a piecewise map back to the original
/// buffer. A span across a gap encloses the removed bytes; a span on either side
/// maps to that side. No per-byte table, padding or synthetic source positions.
/// The first line is retained: the slice may begin after a list/admonition prefix.
pub(crate) struct FilteredSource {
    pub text: String,
    pub removed: Vec<crate::Span>,
    segments: Vec<(usize, usize)>, // transformed start, original start
}

impl FilteredSource {
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

    pub fn original_span(&self, span: crate::Span) -> crate::Span {
        let start_idx = self
            .segments
            .partition_point(|(start, _)| *start <= span.start)
            .saturating_sub(1);
        let end_idx = self
            .segments
            .partition_point(|(start, _)| *start < span.end)
            .saturating_sub(1);
        let (s, original_s) = self.segments[start_idx];
        let (e, original_e) = self.segments[end_idx];
        crate::Span::new(original_s + span.start - s, original_e + span.end - e)
    }
}
