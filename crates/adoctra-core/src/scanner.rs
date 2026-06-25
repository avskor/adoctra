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
