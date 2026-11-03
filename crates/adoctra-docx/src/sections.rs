//! Word sections of the body: page orientation changes (`page-layout` on a
//! page break, `%rotate` on a table) and, in a book whose body section has a
//! distinct first page, a section per part and chapter.
//!
//! Every section is a copy of the template's body section properties (its
//! headers, footers, margins), with the page turned for a landscape
//! section. A section's properties are written where it ends (a paragraph
//! holding `w:sectPr`); the last section's are the body's final ones. Only
//! the first section keeps the template's page-number restart.

use crate::xml;

/// The text width of a page when the template's section does not say (A4
/// with 2 cm margins, twips).
pub const DEFAULT_TEXT_WIDTH: u32 = 9638;

/// The properties a section differs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Kind {
    landscape: bool,
    /// The section's first page has its own header and footer.
    title_page: bool,
}

/// The sections written so far and the current one.
#[derive(Debug, Clone)]
pub struct Sections {
    base: String,
    base_landscape: bool,
    base_title_page: bool,
    /// Parts and chapters start sections.
    chapters: bool,
    current: Kind,
    breaks: usize,
}

impl Sections {
    /// The sections of a body whose final section properties are `base`;
    /// `chapters` makes parts and chapters start sections.
    pub fn new(base: String, chapters: bool) -> Self {
        let base_landscape = page_size(&base).is_some_and(|(w, h, orient)| orient || w > h);
        let base_title_page = has_title_page(&base);
        Self {
            current: Kind {
                landscape: base_landscape,
                title_page: base_title_page,
            },
            base,
            base_landscape,
            base_title_page,
            chapters,
            breaks: 0,
        }
    }

    /// The break before a part or chapter, if they start sections.
    pub fn chapter(&mut self) -> Option<String> {
        if !self.chapters {
            return None;
        }
        Some(self.break_to(Kind {
            landscape: self.current.landscape,
            title_page: self.base_title_page,
        }))
    }

    /// The break turning the page to landscape (or back), if it changes.
    pub fn orientation(&mut self, landscape: bool) -> Option<String> {
        if landscape == self.current.landscape {
            return None;
        }
        // A turned section continues the chapter: no first-page header.
        Some(self.break_to(Kind {
            landscape,
            title_page: false,
        }))
    }

    pub fn landscape(&self) -> bool {
        self.current.landscape
    }

    /// The current section's text width (twips).
    pub fn text_width(&self) -> u32 {
        text_width(&self.render(self.current))
    }

    /// The final section properties when the body has more than one section.
    pub fn final_sect_pr(&self) -> Option<String> {
        (self.breaks > 0).then(|| self.render(self.current))
    }

    fn break_to(&mut self, next: Kind) -> String {
        let sect_pr = self.render(self.current);
        self.breaks += 1;
        self.current = next;
        format!("<w:p><w:pPr>{sect_pr}</w:pPr></w:p>")
    }

    /// The properties of a section of `kind`: the first one keeps the page
    /// numbering restart.
    fn render(&self, kind: Kind) -> String {
        let mut sect_pr = self.base.clone();
        if kind.landscape != self.base_landscape {
            sect_pr = turn(&sect_pr, kind.landscape);
        }
        if !kind.title_page {
            sect_pr = remove_title_page(&sect_pr);
        }
        if self.breaks > 0 {
            sect_pr = continue_numbering(&sect_pr);
        }
        sect_pr
    }
}

/// `w:pgSz` width, height and whether it says landscape.
fn page_size(sect_pr: &str) -> Option<(i64, i64, bool)> {
    let tag = xml::tags(sect_pr, 0).find(|t| t.name == "w:pgSz" && t.opens())?;
    let number = |name| tag.attr(name).and_then(|v| v.parse::<i64>().ok());
    Some((
        number("w:w")?,
        number("w:h")?,
        tag.attr("w:orient").as_deref() == Some("landscape"),
    ))
}

/// The section turned to (or from) landscape: width and height swapped,
/// `w:orient` set accordingly. Margins stay.
fn turn(sect_pr: &str, landscape: bool) -> String {
    let Some(tag) = xml::tags(sect_pr, 0).find(|t| t.name == "w:pgSz" && t.opens()) else {
        return sect_pr.to_string();
    };
    let Some((w, h, _)) = page_size(sect_pr) else {
        return sect_pr.to_string();
    };
    let (w, h) = if (w > h) == landscape { (w, h) } else { (h, w) };
    let code = tag
        .attr("w:code")
        .map(|c| format!(r#" w:code="{}""#, xml::escape_attr(&c)))
        .unwrap_or_default();
    let orient = if landscape {
        r#" w:orient="landscape""#
    } else {
        ""
    };
    format!(
        r#"{}<w:pgSz w:w="{w}" w:h="{h}"{orient}{code}/>{}"#,
        &sect_pr[..tag.range.start],
        &sect_pr[element_end(sect_pr, &tag)..]
    )
}

fn element_end(text: &str, tag: &xml::Tag<'_>) -> usize {
    xml::element(text, tag).map_or(tag.range.end, |(range, _)| range.end)
}

fn has_title_page(sect_pr: &str) -> bool {
    xml::tags(sect_pr, 0)
        .find(|t| t.name == "w:titlePg" && t.opens())
        .is_some_and(|t| !matches!(t.attr("w:val").as_deref(), Some("0" | "false" | "off")))
}

fn remove_title_page(sect_pr: &str) -> String {
    match xml::tags(sect_pr, 0).find(|t| t.name == "w:titlePg" && t.opens()) {
        Some(tag) => {
            let end = element_end(sect_pr, &tag);
            format!("{}{}", &sect_pr[..tag.range.start], &sect_pr[end..])
        }
        None => sect_pr.to_string(),
    }
}

/// Section properties without a page-number restart.
pub fn continue_numbering(sect_pr: &str) -> String {
    let Some(tag) = xml::tags(sect_pr, 0).find(|t| t.name == "w:pgNumType" && t.opens()) else {
        return sect_pr.to_string();
    };
    let tag_text = &sect_pr[tag.range.clone()];
    let Some(at) = tag_text.find(" w:start=") else {
        return sect_pr.to_string();
    };
    let quote = tag_text.as_bytes()[at + 9];
    let end = tag_text[at + 10..]
        .find(quote as char)
        .map_or(tag_text.len(), |e| at + 11 + e);
    let rewritten = format!("{}{}", &tag_text[..at], &tag_text[end..]);
    format!(
        "{}{rewritten}{}",
        &sect_pr[..tag.range.start],
        &sect_pr[tag.range.end..]
    )
}

/// The text width (twips) of section properties: page width minus margins
/// and gutter.
pub fn text_width(sect_pr: &str) -> u32 {
    let attr = |tag: &str, name: &str| {
        xml::tags(sect_pr, 0)
            .find(|t| t.name == tag && t.opens())
            .and_then(|t| t.attr(name))
            .and_then(|v| v.parse::<i64>().ok())
    };
    let Some(page) = attr("w:pgSz", "w:w") else {
        return DEFAULT_TEXT_WIDTH;
    };
    let margins = ["w:left", "w:right", "w:gutter"]
        .iter()
        .map(|m| attr("w:pgMar", m).unwrap_or(0))
        .sum::<i64>();
    u32::try_from(page - margins)
        .ok()
        .filter(|w| *w > 0)
        .unwrap_or(DEFAULT_TEXT_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOK: &str = r#"<w:sectPr><w:headerReference w:type="first" r:id="rId7"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:left="1134" w:right="1134"/><w:pgNumType w:start="1"/><w:titlePg/></w:sectPr>"#;

    #[test]
    fn later_sections_continue_page_numbers() {
        let sect = r#"<w:sectPr><w:pgNumType w:fmt="decimal" w:start="1"/><w:titlePg/></w:sectPr>"#;
        assert_eq!(
            continue_numbering(sect),
            r#"<w:sectPr><w:pgNumType w:fmt="decimal"/><w:titlePg/></w:sectPr>"#
        );
        assert_eq!(continue_numbering("<w:sectPr/>"), "<w:sectPr/>");
    }

    #[test]
    fn chapters_start_sections_with_a_first_page() {
        let mut s = Sections::new(BOOK.into(), true);
        assert!(s.final_sect_pr().is_none());
        let first = s.chapter().unwrap();
        assert!(first.contains(r#"w:start="1""#) && first.contains("<w:titlePg/>"));
        let second = s.chapter().unwrap();
        assert!(!second.contains("w:start") && second.contains("<w:titlePg/>"));
        assert!(!s.final_sect_pr().unwrap().contains("w:start"));
        assert!(Sections::new(BOOK.into(), false).chapter().is_none());
    }

    #[test]
    fn orientation_turns_the_page_and_back() {
        let mut s = Sections::new(BOOK.into(), false);
        assert_eq!(s.text_width(), 9638);
        assert!(s.orientation(false).is_none());
        // The portrait section ends; the landscape one has no first page.
        let to_landscape = s.orientation(true).unwrap();
        assert!(to_landscape.contains(r#"<w:pgSz w:w="11906" w:h="16838"/>"#));
        assert!(s.landscape());
        assert_eq!(s.text_width(), 16838 - 2268);
        let back = s.orientation(false).unwrap();
        assert!(
            back.contains(r#"<w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>"#),
            "{back}"
        );
        assert!(!back.contains("<w:titlePg/>") && !back.contains("w:start"));
        // The last section is portrait again, without a first page.
        let last = s.final_sect_pr().unwrap();
        assert!(
            last.contains(r#"<w:pgSz w:w="11906" w:h="16838"/>"#),
            "{last}"
        );
        assert!(!last.contains("<w:titlePg/>"));
    }

    #[test]
    fn a_landscape_template_turns_to_portrait() {
        let mut s = Sections::new(
            r#"<w:sectPr><w:pgSz w:w="16838" w:h="11906" w:orient="landscape" w:code="9"/></w:sectPr>"#.into(),
            false,
        );
        assert!(s.landscape());
        assert!(s.orientation(true).is_none());
        s.orientation(false).unwrap();
        assert_eq!(
            s.final_sect_pr().unwrap(),
            r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838" w:code="9"/></w:sectPr>"#
        );
    }

    #[test]
    fn text_width_is_the_page_minus_margins() {
        assert_eq!(
            text_width(
                r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:left="1134" w:right="1134" w:gutter="100"/></w:sectPr>"#
            ),
            9538
        );
        assert_eq!(text_width("<w:sectPr/>"), DEFAULT_TEXT_WIDTH);
        assert_eq!(
            text_width(r#"<w:sectPr><w:pgSz w:w="100"/><w:pgMar w:left="200"/></w:sectPr>"#),
            DEFAULT_TEXT_WIDTH
        );
    }
}
