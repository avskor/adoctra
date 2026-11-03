//! Definitions of Word's built-in styles with Word's default formatting.
//!
//! Word writes a built-in style (`heading 4`, `toc 3`, `caption`…) into
//! `styles.xml` only once it is used or modified; otherwise only its
//! `w:latentStyles` entry is saved. A template made in Word therefore often
//! lacks built-in styles the converter needs. Such a style is appended from
//! here, so the output looks like Word's own default instead of plain
//! `Normal`. Custom styles have no default and are reported instead.

use crate::styles::StyleKind;

/// A built-in style definition. `{based_on}` in `body` is replaced by the
/// resolved id of the base style.
pub struct Builtin {
    pub kind: StyleKind,
    /// Word's style name (as written in `w:name`).
    pub name: &'static str,
    /// The id Word gives the style in an English UI.
    pub id: &'static str,
    pub based_on: Option<&'static str>,
    pub next: Option<&'static str>,
    /// The definition's properties (`w:pPr`, `w:rPr`, `w:tblPr`…).
    pub body: String,
}

const MAJOR_FONT: &str = r#"<w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia" w:hAnsiTheme="majorHAnsi" w:cstheme="majorBidi"/>"#;

/// The built-in style named `name` (case-insensitive), if Word has one.
pub fn builtin(name: &str) -> Option<Builtin> {
    let lower = name.to_ascii_lowercase();
    let para = |name, id, based_on, body: String| Builtin {
        kind: StyleKind::Paragraph,
        name,
        id,
        based_on,
        next: Some("Normal"),
        body,
    };
    if let Some(level) = lower.strip_prefix("heading ") {
        let level: usize = level.parse().ok().filter(|n| (1..=9).contains(n))?;
        const NAMES: [&str; 9] = [
            "heading 1",
            "heading 2",
            "heading 3",
            "heading 4",
            "heading 5",
            "heading 6",
            "heading 7",
            "heading 8",
            "heading 9",
        ];
        const IDS: [&str; 9] = [
            "Heading1", "Heading2", "Heading3", "Heading4", "Heading5", "Heading6", "Heading7",
            "Heading8", "Heading9",
        ];
        // Word 2016+ defaults (Office theme).
        let (before, color, size, italic) = match level {
            1 => (240, "2F5496", 32, false),
            2 => (40, "2F5496", 26, false),
            3 => (40, "1F3763", 24, false),
            4 => (40, "2F5496", 0, true),
            5 => (40, "2F5496", 0, false),
            6 => (40, "1F3763", 0, false),
            7 => (40, "1F3763", 0, true),
            8 => (40, "272727", 21, false),
            _ => (40, "272727", 21, true),
        };
        let size = if size > 0 {
            format!(r#"<w:sz w:val="{size}"/><w:szCs w:val="{size}"/>"#)
        } else {
            String::new()
        };
        let italic = if italic { "<w:i/><w:iCs/>" } else { "" };
        return Some(para(
            NAMES[level - 1],
            IDS[level - 1],
            Some("Normal"),
            format!(
                r#"<w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="{before}" w:after="0"/><w:outlineLvl w:val="{}"/></w:pPr><w:rPr>{MAJOR_FONT}{italic}<w:color w:val="{color}"/>{size}</w:rPr>"#,
                level - 1
            ),
        ));
    }
    if let Some(level) = lower
        .strip_prefix("toc ")
        .and_then(|level| level.parse::<usize>().ok())
    {
        if !(1..=9).contains(&level) {
            return None;
        }
        const NAMES: [&str; 9] = [
            "toc 1", "toc 2", "toc 3", "toc 4", "toc 5", "toc 6", "toc 7", "toc 8", "toc 9",
        ];
        const IDS: [&str; 9] = [
            "TOC1", "TOC2", "TOC3", "TOC4", "TOC5", "TOC6", "TOC7", "TOC8", "TOC9",
        ];
        return Some(para(
            NAMES[level - 1],
            IDS[level - 1],
            Some("Normal"),
            format!(
                r#"<w:pPr><w:spacing w:after="100"/><w:ind w:left="{}"/></w:pPr>"#,
                (level - 1) * 220
            ),
        ));
    }
    let def = match lower.as_str() {
        "normal" => Builtin {
            kind: StyleKind::Paragraph,
            name: "Normal",
            id: "Normal",
            based_on: None,
            next: None,
            body: String::new(),
        },
        "title" => para(
            "Title",
            "Title",
            Some("Normal"),
            format!(
                r#"<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:contextualSpacing/></w:pPr><w:rPr>{MAJOR_FONT}<w:spacing w:val="-10"/><w:kern w:val="28"/><w:sz w:val="56"/><w:szCs w:val="56"/></w:rPr>"#
            ),
        ),
        "subtitle" => para(
            "Subtitle",
            "Subtitle",
            Some("Normal"),
            r#"<w:pPr><w:spacing w:after="160"/></w:pPr><w:rPr><w:color w:val="5A5A5A"/><w:spacing w:val="15"/><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr>"#.into(),
        ),
        "toc heading" => Builtin {
            next: Some("Normal"),
            ..para(
                "TOC Heading",
                "TOCHeading",
                Some("heading 1"),
                r#"<w:pPr><w:outlineLvl w:val="9"/></w:pPr>"#.into(),
            )
        },
        "caption" => para(
            "caption",
            "Caption",
            Some("Normal"),
            r#"<w:pPr><w:spacing w:after="200" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:i/><w:iCs/><w:color w:val="44546A"/><w:sz w:val="18"/><w:szCs w:val="18"/></w:rPr>"#.into(),
        ),
        "list paragraph" => para(
            "List Paragraph",
            "ListParagraph",
            Some("Normal"),
            r#"<w:pPr><w:ind w:left="720"/><w:contextualSpacing/></w:pPr>"#.into(),
        ),
        "footnote text" => para(
            "footnote text",
            "FootnoteText",
            Some("Normal"),
            r#"<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr>"#.into(),
        ),
        "quote" => para(
            "Quote",
            "Quote",
            Some("Normal"),
            r#"<w:pPr><w:spacing w:before="200" w:after="160"/><w:ind w:left="864" w:right="864"/><w:jc w:val="center"/></w:pPr><w:rPr><w:i/><w:iCs/><w:color w:val="404040"/></w:rPr>"#.into(),
        ),
        "footnote reference" => character(
            "footnote reference",
            "FootnoteReference",
            r#"<w:rPr><w:vertAlign w:val="superscript"/></w:rPr>"#,
        ),
        "hyperlink" => character(
            "Hyperlink",
            "Hyperlink",
            r#"<w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr>"#,
        ),
        "strong" => character("Strong", "Strong", r#"<w:rPr><w:b/><w:bCs/></w:rPr>"#),
        "emphasis" => character("Emphasis", "Emphasis", r#"<w:rPr><w:i/><w:iCs/></w:rPr>"#),
        "table grid" => Builtin {
            kind: StyleKind::Table,
            name: "Table Grid",
            id: "TableGrid",
            based_on: None,
            next: None,
            body: format!(
                r#"<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders>{}</w:tblBorders><w:tblCellMar><w:left w:w="108" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr>"#,
                ["top", "left", "bottom", "right", "insideH", "insideV"]
                    .iter()
                    .map(|side| format!(
                        r#"<w:{side} w:val="single" w:sz="4" w:space="0" w:color="auto"/>"#
                    ))
                    .collect::<String>()
            ),
        },
        _ => return None,
    };
    Some(def)
}

fn character(name: &'static str, id: &'static str, body: &str) -> Builtin {
    Builtin {
        kind: StyleKind::Character,
        name,
        id,
        based_on: None,
        next: None,
        body: body.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_word_builtins_case_insensitively() {
        let h = builtin("Heading 3").unwrap();
        assert_eq!(
            (h.name, h.id, h.kind),
            ("heading 3", "Heading3", StyleKind::Paragraph)
        );
        assert!(h.body.contains(r#"<w:outlineLvl w:val="2"/>"#));
        assert_eq!(builtin("TOC 2").unwrap().id, "TOC2");
        assert_eq!(builtin("toc heading").unwrap().based_on, Some("heading 1"));
        assert_eq!(builtin("hyperlink").unwrap().kind, StyleKind::Character);
        assert_eq!(builtin("Table Grid").unwrap().kind, StyleKind::Table);
    }

    #[test]
    fn has_no_custom_or_out_of_range_styles() {
        assert!(builtin("Code").is_none());
        assert!(builtin("heading 10").is_none());
        assert!(builtin("toc 0").is_none());
    }
}
