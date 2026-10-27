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
    if copied == 0 {
        return text;
    }
    text.copy(copied..text.text.len(), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::substitution_text::tests::{assert_reused, marked_text};
    use crate::{SourceId, Span};

    #[test]
    fn unchanged_break_stage_reuses_buffers_for_both_modes_and_edges() {
        for hardbreaks in [false, true] {
            for right_edge in [false, true] {
                for source in ["Ж🙂 & text", "plus + in text"] {
                    assert_reused(marked_text(source), |text| {
                        apply(text, hardbreaks, right_edge)
                    });
                }
            }
        }
        assert_reused(marked_text("Ж🙂\r\nnext\n"), |text| {
            apply(text, false, true)
        });
        for (hardbreaks, right_edge) in [(true, true), (true, false), (false, false)] {
            let mut text = Text::default();
            text.append_source("Ж🙂 +", false, 2, Span::in_source(SourceId(7), 10, 18));
            assert_reused(text, |text| apply(text, hardbreaks, right_edge));
        }
    }

    #[test]
    fn break_marker_across_fragments_still_changes_text_and_keeps_its_origin() {
        let source = Span::in_source(SourceId(7), 10, 17);
        let attribute = Span::in_source(SourceId(8), 50, 57);
        let mut text = Text::default();
        text.append_source("Ж🙂 ", false, 2, source);
        text.append_part(&AttributePart::Text("+".into()), 3, attribute);
        let result = apply(text, false, true);
        let linebreak = result
            .fragments
            .iter()
            .find(|(_, f)| f.part == Some(AttributePart::LineBreak))
            .unwrap();
        assert_eq!(linebreak.1.owner, 2);
        assert_eq!(linebreak.1.span.source_id, source.source_id);
        assert_eq!(
            result.into_parts(),
            [AttributePart::Text("Ж🙂".into()), AttributePart::LineBreak]
        );
        for (source, hardbreaks) in [(" +", false), ("\n", true)] {
            let text = Text::from_parts(&[AttributePart::Text(source.into())]);
            assert!(apply(text, hardbreaks, true)
                .into_parts()
                .contains(&AttributePart::LineBreak));
        }
    }
}
