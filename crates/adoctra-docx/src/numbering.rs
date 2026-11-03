//! List numbering: one single-level `w:abstractNum` and `w:num` per list.
//!
//! Each AsciiDoc list gets its own definition, so it numbers from its own
//! start and keeps its own marker style however lists nest and alternate
//! (Ruby restarts every list). The level's indentation includes the
//! enclosing content's, so a list nests visually without Word's multilevel
//! machinery. Definitions are appended to the template's numbering part,
//! after its own ids.

use crate::xml;

/// Indentation of a list level's text from the enclosing content (twips).
pub const LIST_INDENT: u32 = 720;
/// The marker's hanging indent (twips).
pub const HANGING: u32 = 360;

/// A list's marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level {
    /// `w:numFmt`: `bullet`, `decimal`, `lowerLetter`, `none`…
    pub format: &'static str,
    /// `w:lvlText`: the bullet or `%1.`.
    pub text: String,
    pub start: u32,
    /// The text's left indent (twips).
    pub left: u32,
    /// No tab after the marker (a checklist's empty marker).
    pub no_suffix: bool,
}

/// The numbering part's added definitions.
#[derive(Debug, Clone, Default)]
pub struct Numbering {
    next_abstract: u32,
    next_num: u32,
    abstracts: String,
    nums: String,
}

impl Numbering {
    /// The numbering of a template whose numbering part is `text` (empty
    /// when it has none): new ids follow the largest ones it uses.
    pub fn parse(text: &str) -> Self {
        let mut max_abstract = None::<u32>;
        let mut max_num = 0;
        for tag in xml::tags(text, 0).filter(|t| t.opens()) {
            let id = |name| tag.attr(name).and_then(|v| v.parse::<u32>().ok());
            match tag.name {
                "w:abstractNum" => {
                    max_abstract = max_abstract.max(id("w:abstractNumId"));
                }
                "w:num" => max_num = max_num.max(id("w:numId").unwrap_or(0)),
                _ => {}
            }
        }
        Self {
            next_abstract: max_abstract.map_or(0, |n| n + 1),
            next_num: max_num + 1,
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nums.is_empty()
    }

    /// A new list numbered as `level`; its `w:numId`.
    pub fn add(&mut self, level: &Level) -> u32 {
        let abstract_id = self.next_abstract;
        let num_id = self.next_num;
        self.next_abstract += 1;
        self.next_num += 1;
        let suffix = if level.no_suffix {
            r#"<w:suff w:val="nothing"/>"#
        } else {
            ""
        };
        self.abstracts.push_str(&format!(
            r#"<w:abstractNum w:abstractNumId="{abstract_id}"><w:multiLevelType w:val="singleLevel"/><w:lvl w:ilvl="0"><w:start w:val="{}"/><w:numFmt w:val="{}"/>{suffix}<w:lvlText w:val="{}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{}" w:hanging="{HANGING}"/></w:pPr></w:lvl></w:abstractNum>"#,
            level.start,
            level.format,
            xml::escape_attr(&level.text),
            level.left
        ));
        self.nums.push_str(&format!(
            r#"<w:num w:numId="{num_id}"><w:abstractNumId w:val="{abstract_id}"/></w:num>"#
        ));
        num_id
    }

    /// The numbering part with the added definitions: abstract definitions
    /// before the first `w:num` (the schema's order), lists at the end.
    pub fn apply(&self, text: &str) -> String {
        if self.is_empty() {
            return text.to_string();
        }
        let first_num = xml::tags(text, 0)
            .find(|t| (t.name == "w:num" || t.name == "w:numIdMacAtCleanup") && t.opens())
            .map(|t| t.range.start);
        let mut out = match first_num {
            Some(at) => format!("{}{}{}", &text[..at], self.abstracts, &text[at..]),
            None => crate::package::insert_before_close(text, "w:numbering", &self.abstracts),
        };
        // Lists before a trailing `w:numIdMacAtCleanup`, else at the end.
        let cleanup = xml::tags(&out, 0)
            .find(|t| t.name == "w:numIdMacAtCleanup" && t.opens())
            .map(|t| t.range.start);
        match cleanup {
            Some(at) => out.insert_str(at, &self.nums),
            None => out = crate::package::insert_before_close(&out, "w:numbering", &self.nums),
        }
        out
    }
}

/// The bullet of an unordered list: Ruby's style (`square`, `circle`,
/// `disc`, `none`…) or the browser default for its nesting depth.
pub fn bullet(style: Option<&str>, depth: u8) -> Option<&'static str> {
    match style {
        Some("square") => Some("\u{25aa}"),
        Some("circle") => Some("\u{25e6}"),
        Some("disc") => Some("\u{2022}"),
        Some("none" | "no-bullet" | "unstyled" | "inline") => None,
        _ => Some(match depth {
            0 => "\u{2022}",
            1 => "\u{25e6}",
            _ => "\u{25aa}",
        }),
    }
}

/// `w:numFmt` of an ordered list style (Ruby's `arabic`, `loweralpha`…);
/// `None` for an unnumbered style. Word has no Greek numbering: `lowergreek`
/// uses letters (the caller reports it).
pub fn number_format(style: Option<&str>) -> Option<&'static str> {
    Some(match style {
        Some("loweralpha" | "lowergreek") => "lowerLetter",
        Some("upperalpha") => "upperLetter",
        Some("lowerroman") => "lowerRoman",
        Some("upperroman") => "upperRoman",
        Some("decimal") => "decimalZero",
        Some("none" | "no-bullet" | "unstyled" | "unnumbered") => return None,
        _ => "decimal",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(format: &'static str, text: &str) -> Level {
        Level {
            format,
            text: text.into(),
            start: 1,
            left: 720,
            no_suffix: false,
        }
    }

    #[test]
    fn new_ids_follow_the_template() {
        let mut n = Numbering::parse(
            r#"<w:numbering><w:abstractNum w:abstractNumId="4"/><w:num w:numId="9"><w:abstractNumId w:val="4"/></w:num></w:numbering>"#,
        );
        assert_eq!(n.add(&level("decimal", "%1.")), 10);
        assert_eq!(n.add(&level("bullet", "•")), 11);
        let mut empty = Numbering::parse("");
        assert_eq!(empty.add(&level("decimal", "%1.")), 1);
        assert!(empty.abstracts.contains(r#"w:abstractNumId="0""#));
    }

    #[test]
    fn keeps_the_schema_order() {
        let mut n = Numbering::parse(
            r#"<w:numbering><w:abstractNum w:abstractNumId="0"/><w:num w:numId="1"/><w:numIdMacAtCleanup w:val="1"/></w:numbering>"#,
        );
        n.add(&Level {
            no_suffix: true,
            start: 3,
            ..level("none", "")
        });
        let out = n.apply(
            r#"<w:numbering><w:abstractNum w:abstractNumId="0"/><w:num w:numId="1"/><w:numIdMacAtCleanup w:val="1"/></w:numbering>"#,
        );
        assert_eq!(
            out,
            r#"<w:numbering><w:abstractNum w:abstractNumId="0"/><w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="singleLevel"/><w:lvl w:ilvl="0"><w:start w:val="3"/><w:numFmt w:val="none"/><w:suff w:val="nothing"/><w:lvlText w:val=""/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"/><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num><w:numIdMacAtCleanup w:val="1"/></w:numbering>"#
        );
        let mut fresh = Numbering::parse("");
        fresh.add(&level("decimal", "%1."));
        let out = fresh.apply("<w:numbering/>");
        assert!(out.starts_with("<w:numbering><w:abstractNum"));
        assert!(out.ends_with("</w:num></w:numbering>"));
        assert_eq!(Numbering::default().apply("<x/>"), "<x/>");
    }

    #[test]
    fn maps_ruby_list_styles() {
        assert_eq!(bullet(None, 0), Some("\u{2022}"));
        assert_eq!(bullet(None, 1), Some("\u{25e6}"));
        assert_eq!(bullet(None, 5), Some("\u{25aa}"));
        assert_eq!(bullet(Some("square"), 0), Some("\u{25aa}"));
        assert_eq!(bullet(Some("no-bullet"), 0), None);
        assert_eq!(number_format(Some("upperroman")), Some("upperRoman"));
        assert_eq!(number_format(Some("decimal")), Some("decimalZero"));
        assert_eq!(number_format(Some("arabic")), Some("decimal"));
        assert_eq!(number_format(Some("unnumbered")), None);
    }
}
