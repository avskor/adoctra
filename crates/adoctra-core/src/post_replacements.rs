//! The final substitution stage, before protected passthroughs are restored.
//! Breaks retain their source owner; attribute expansions keep the reference span.
use crate::{substitution_text::Text, AttributePart};

pub(crate) fn apply(text: Text, hardbreaks: bool, right_edge: bool) -> Text {
    let mut out = Text::default();
    let mut start = 0;
    // Copy contiguous runs: an atomic part whose spelling spans lines (an
    // inline SVG) splits only where a break is inserted.
    let mut copied = 0;
    for line in text.text.split_inclusive('\n') {
        let newline = line.ends_with('\n');
        let end = start + line.len() - usize::from(newline) - usize::from(line.ends_with("\r\n"));
        let explicit = text.text[start..end].ends_with(" +");
        // With hardbreaks enabled Ruby leaves the final line entirely alone,
        // including its explicit marker. A bounded inline run is not a line end.
        if if hardbreaks {
            newline
        } else {
            explicit && (newline || right_edge)
        } {
            let content_end = end - if explicit { 2 } else { 0 };
            text.copy(copied..content_end, &mut out);
            let anchor_end = if explicit { end } else { start + line.len() };
            text.replace_parts(
                content_end..anchor_end,
                &[AttributePart::LineBreak],
                &mut out,
            );
            text.copy(end..start + line.len(), &mut out);
            copied = start + line.len();
        }
        start += line.len();
    }
    text.copy(copied..text.text.len(), &mut out);
    out
}
