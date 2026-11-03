//! The template contract: the style names the converter asks a template for
//! (`docs/docx.adoc`). A template provides any subset; a missing Word
//! built-in is added with Word's defaults, a missing custom style is
//! reported and its element keeps the base formatting.

/// Paragraph styles.
pub const PARAGRAPH: &[&str] = &[
    "Normal",
    "Title",
    "Subtitle",
    "Document Details",
    "Part Title",
    "Heading 1",
    "Heading 2",
    "Heading 3",
    "Heading 4",
    "Heading 5",
    "Heading 6",
    "Appendix Heading 1",
    "Appendix Heading 2",
    "Appendix Heading 3",
    "Appendix Heading 4",
    "Appendix Heading 5",
    "Appendix Heading 6",
    "Floating Title 1",
    "Floating Title 2",
    "Floating Title 3",
    "Floating Title 4",
    "Floating Title 5",
    "Floating Title 6",
    "Lead",
    "Abstract",
    "List Paragraph",
    "Term",
    "Description",
    "Question",
    "Answer",
    "Code",
    "Literal",
    "Horizontal Rule",
    "Block Title",
    "Caption",
    "Quote",
    "Quote Attribution",
    "Verse",
    "Verse Attribution",
    "Sidebar Title",
    "Admonition Label",
    "Footnote Text",
    "TOC Heading",
    "TOC 1",
    "TOC 2",
    "TOC 3",
    "TOC 4",
    "TOC 5",
    "TOC 6",
];

/// Character styles.
pub const CHARACTER: &[&str] = &[
    "Strong",
    "Emphasis",
    "Monospace",
    "Mark",
    "Hyperlink",
    "Xref",
    "Keyboard",
    "Button",
    "Menu",
    "Callout",
    "Line Number",
    "Footnote Reference",
];

/// Table styles: data tables and the frames of delimited blocks.
pub const TABLE: &[&str] = &[
    "Table",
    "Code Table",
    "Literal Table",
    "Admonition Table",
    "Admonition Note Table",
    "Admonition Tip Table",
    "Admonition Important Table",
    "Admonition Warning Table",
    "Admonition Caution Table",
    "Example Table",
    "Sidebar Table",
    "Quote Table",
    "Verse Table",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_within_a_kind() {
        for list in [PARAGRAPH, CHARACTER, TABLE] {
            let mut sorted: Vec<_> = list.iter().map(|n| n.to_ascii_lowercase()).collect();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), list.len());
        }
    }
}
