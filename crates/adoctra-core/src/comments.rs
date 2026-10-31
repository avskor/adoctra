//! Context-sensitive comments. Callers decide whether a region is parsed text
//! or verbatim; this is deliberately not a global deletion pass.
use crate::context::ParseContext;
use crate::diagnostic::{Diagnostic, Severity};
use crate::reader::Reader;
use crate::ruby_strip::RubyStrip;
use crate::scanner::FilteredSource;
use crate::Span;

pub(crate) fn is_line_comment(line: &str) -> bool {
    line.starts_with("//") && !line.starts_with("///")
}

pub(crate) fn comment_fence(line: &str) -> Option<&str> {
    let line = line.ruby_rstrip();
    (line.len() >= 4 && line.bytes().all(|b| b == b'/')).then_some(line)
}

/// Consume a comment at a block/header boundary, including a matching fence of
/// the same length. Hidden attributes, delimiters and blank lines have no effect.
pub(crate) fn skip_comment(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
) -> bool {
    if !lines.has(*i, context) {
        return false;
    }
    let (start, line_buffer) = lines.line(*i);
    let line = line_buffer.as_ref();
    if is_line_comment(line) {
        context.comments.push(Span::new(start, start + line.len()));
        *i += 1;
        return true;
    }
    let Some(fence) = comment_fence(line) else {
        return false;
    };
    consume_delimited_comment(lines, i, context, fence);
    true
}

pub(crate) fn consume_delimited_comment(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    fence: &str,
) {
    let (start, line_buffer) = lines.line(*i);
    let line = line_buffer.as_ref();
    let mut end = start + line.len();
    *i += 1;
    let mut closed = false;
    while lines.raw_has(*i) {
        let (off, line_buffer) = lines.line(*i);
        let line = line_buffer.as_ref();
        end = off + line.len();
        *i += 1;
        if line.ruby_rstrip() == fence {
            closed = true;
            break;
        }
    }
    context.comments.push(Span::new(start, end));
    if !closed {
        context.comment_diagnostics.push(Diagnostic {
            omitted_span: None,
            report: crate::DiagnosticReport::unterminated("comment"),
            span: Span::new(start, start + fence.len()),
            severity: Severity::Warning,
            message: format!("unterminated comment block (missing closing `{fence}`)"),
        });
    }
}

pub(crate) fn filtered_text(src: &str, base: usize) -> FilteredSource {
    FilteredSource::without_lines(src, base, is_line_comment)
}
