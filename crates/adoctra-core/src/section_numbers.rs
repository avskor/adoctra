//! Section numbering, like Ruby's `Parser.initialize_section` (whether a
//! section is numbered, decided with the attributes while reading its title),
//! `AbstractBlock#assign_numeral` (numerals among numbered siblings, applied
//! when the hierarchy is built) and `Section#sectnum`. The displayed prefix is
//! produced during conversion with the attributes at the section (see
//! body_subs.rs), as the HTML converter's `convert_section` does.

use crate::Attributes;

/// What reading decided about a section title.
#[derive(Clone, Debug)]
pub(crate) struct Section {
    /// Ruby's `sectname`: `section`, `chapter` or `part` in a book, or the
    /// style of a special section.
    pub sectname: String,
    pub numbered: Numbered,
    pub book: bool,
    pub level: u8,
    /// A chapter's `chapter-number` counter value, taken when reading closes it.
    pub numeral: Option<String>,
    /// `Section#sectnum` components: the numerals from the outermost section
    /// the number continues through (an unnumbered one contributes an empty
    /// numeral, like Ruby's `nil`). Assigned when the hierarchy is built.
    pub path: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Numbered {
    No,
    Yes,
    /// A special section numbered as a chapter (`sectnums=all` in a book).
    Chapter,
}

impl Section {
    /// Ruby's `initialize_section` for a title at `level` with block `style`.
    /// Special sections (appendix, preface, …) are otherwise unimplemented:
    /// their level is kept and an appendix is not lettered.
    pub fn read(style: Option<&str>, level: u8, book: bool, attributes: &Attributes) -> Self {
        let sectnums = attributes.get("sectnums");
        let (sectname, special) = match style {
            Some("abstract") if book => ("chapter".to_owned(), false),
            Some(style) if is_level_style(style) => ("section".to_owned(), false),
            Some(style) => (style.to_owned(), true),
            None if book => (
                match level {
                    0 => "part",
                    1 => "chapter",
                    _ => "section",
                }
                .to_owned(),
                false,
            ),
            None => ("section".to_owned(), false),
        };
        let numbered = if special {
            match sectnums {
                Some("all") if book && level.max(1) == 1 => Numbered::Chapter,
                Some("all") => Numbered::Yes,
                _ => Numbered::No,
            }
        } else if (sectnums.is_some() && level > 0)
            || (book && level == 0 && attributes.get("partnums").is_some())
        {
            Numbered::Yes
        } else {
            Numbered::No
        };
        Self {
            sectname,
            numbered,
            book,
            level,
            numeral: None,
            path: Vec::new(),
        }
    }

    /// Whether closing this section advances the `chapter-number` counter.
    pub fn counts_chapter(&self) -> bool {
        self.numbered == Numbered::Chapter
            || (self.numbered == Numbered::Yes && self.sectname == "chapter")
    }

    pub fn is_numbered(&self) -> bool {
        self.numbered != Numbered::No
    }

    /// Ruby's `sectnum(delimiter, append)`.
    pub fn sectnum(&self, delimiter: &str, append: &str) -> String {
        format!("{}{append}", self.path.join(delimiter))
    }

    /// Chapters and appendices are emphasized in cross-reference labels.
    pub fn emphasized(&self) -> bool {
        matches!(self.sectname.as_str(), "chapter" | "appendix")
    }

    /// The title prefix `convert_section` renders (with its trailing space),
    /// using the attributes at the section; `None` beyond `sectnumlevels`.
    pub fn caption(&self, attributes: &Attributes) -> Option<String> {
        if !self.is_numbered() {
            return None;
        }
        let levels = attributes
            .get("sectnumlevels")
            .map_or(3, crate::include_selection::integer_prefix);
        if i128::from(self.level) > levels {
            return None;
        }
        let signified = |name: &str, number: String| match attributes.get(name) {
            Some(signifier) => format!("{signifier} {number} "),
            None => format!("{number} "),
        };
        Some(match self.sectname.as_str() {
            "chapter" if self.level < 2 && self.book => {
                signified("chapter-signifier", self.sectnum(".", "."))
            }
            "part" if self.level < 2 && self.book => {
                signified("part-signifier", self.sectnum("", ":"))
            }
            _ => format!("{} ", self.sectnum(".", ".")),
        })
    }
}

/// Ruby's `SectionLevelStyleRx` (`sect` and one digit).
fn is_level_style(style: &str) -> bool {
    style
        .strip_prefix("sect")
        .is_some_and(|rest| rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit())
}

/// Numerals of numbered sections within one parent (Ruby's
/// `@next_section_ordinal`).
#[derive(Default)]
pub(crate) struct Ordinals {
    next: u32,
}

impl Ordinals {
    /// Ruby's `assign_numeral`: chapters take their `chapter-number` counter
    /// value, parts a Roman ordinal, other sections the next ordinal among
    /// their numbered siblings. `None` for an unnumbered section.
    pub fn assign(&mut self, section: &Section) -> Option<String> {
        if !section.is_numbered() {
            return None;
        }
        if section.counts_chapter() {
            return section.numeral.clone();
        }
        self.next += 1;
        Some(if section.sectname == "part" {
            roman(self.next)
        } else {
            self.next.to_string()
        })
    }
}

/// Ruby's `Helpers.int_to_roman`.
fn roman(mut value: u32) -> String {
    const NUMERALS: [(&str, u32); 13] = [
        ("M", 1000),
        ("CM", 900),
        ("D", 500),
        ("CD", 400),
        ("C", 100),
        ("XC", 90),
        ("L", 50),
        ("XL", 40),
        ("X", 10),
        ("IX", 9),
        ("V", 5),
        ("IV", 4),
        ("I", 1),
    ];
    let mut result = String::new();
    for (letters, amount) in NUMERALS {
        while value >= amount {
            result.push_str(letters);
            value -= amount;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attributes(pairs: &[(&str, &str)]) -> Attributes {
        let mut attributes = Attributes::new();
        for (key, value) in pairs {
            attributes.set(*key, *value);
        }
        attributes
    }

    #[test]
    fn roman_numerals_match_ruby() {
        let values: Vec<_> = [1, 4, 9, 14, 40, 90, 400, 1994].map(roman).into();
        assert_eq!(
            values,
            ["I", "IV", "IX", "XIV", "XL", "XC", "CD", "MCMXCIV"]
        );
    }

    #[test]
    fn reading_decides_names_and_numbering() {
        let on = attributes(&[("sectnums", "")]);
        let all = attributes(&[("sectnums", "all"), ("partnums", "")]);
        let read = |style, level, book, attrs: &Attributes| {
            let section = Section::read(style, level, book, attrs);
            (section.sectname, section.numbered)
        };
        assert_eq!(read(None, 1, false, &on), ("section".into(), Numbered::Yes));
        assert_eq!(read(None, 0, false, &on), ("section".into(), Numbered::No));
        assert_eq!(read(None, 1, true, &on), ("chapter".into(), Numbered::Yes));
        assert_eq!(read(None, 0, true, &on), ("part".into(), Numbered::No));
        assert_eq!(read(None, 0, true, &all), ("part".into(), Numbered::Yes));
        assert_eq!(
            read(Some("sect1"), 1, true, &on),
            ("section".into(), Numbered::Yes)
        );
        assert_eq!(
            read(Some("abstract"), 1, true, &on),
            ("chapter".into(), Numbered::Yes)
        );
        assert_eq!(
            read(Some("preface"), 1, false, &on),
            ("preface".into(), Numbered::No)
        );
        assert_eq!(
            read(Some("preface"), 1, true, &all),
            ("preface".into(), Numbered::Chapter)
        );
        assert_eq!(
            read(Some("glossary"), 2, true, &all),
            ("glossary".into(), Numbered::Yes)
        );
        assert_eq!(
            read(None, 1, false, &Attributes::new()),
            ("section".into(), Numbered::No)
        );
    }

    #[test]
    fn captions_follow_levels_and_signifiers() {
        let mut section = Section::read(None, 2, false, &attributes(&[("sectnums", "")]));
        section.path = vec!["1".into(), "2".into()];
        assert_eq!(
            section.caption(&Attributes::new()).as_deref(),
            Some("1.2. ")
        );
        let one = attributes(&[("sectnumlevels", "1")]);
        assert_eq!(section.caption(&one), None);
        assert_eq!(section.sectnum(".", ","), "1.2,");
        let mut chapter = Section::read(None, 1, true, &attributes(&[("sectnums", "")]));
        chapter.path = vec!["3".into()];
        let signifier = attributes(&[("chapter-signifier", "Chapter")]);
        assert_eq!(chapter.caption(&signifier).as_deref(), Some("Chapter 3. "));
        let mut part = Section::read(None, 0, true, &attributes(&[("partnums", "")]));
        part.path = vec!["II".into()];
        assert_eq!(part.caption(&Attributes::new()).as_deref(), Some("II: "));
    }
}
