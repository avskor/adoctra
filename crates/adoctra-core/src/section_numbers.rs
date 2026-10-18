//! Section kinds and numbering, like Ruby's `Parser.initialize_section`
//! (`sectname`, special sections and whether a section is numbered, decided
//! with the attributes while reading its title),
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
    /// The section's level: a special section at level 0 and a book's
    /// abstract become level 1.
    pub level: u8,
    /// The level of the title line, which decides where the section nests.
    pub raw_level: u8,
    /// A special section (appendix, preface, glossary, …) or one nested in
    /// it (Ruby inherits `special` from the parent section).
    pub special: bool,
    /// A chapter's `chapter-number` or an appendix's `appendix-number`
    /// counter value, taken when reading closes it.
    pub numeral: Option<String>,
    /// An appendix caption (`Appendix A: `, or `A. ` without
    /// `appendix-caption`), fixed when reading closes it.
    pub caption: Option<String>,
    /// `Section#sectnum` components: the numerals from the outermost section
    /// the number continues through (an unnumbered one contributes an empty
    /// numeral, like Ruby's `nil`). Assigned when the hierarchy is built.
    pub path: Vec<String>,
    /// Where reading closed each section of `path`: Ruby assigns numerals
    /// only then, so a label produced while reading inside it sees none.
    pub closes: Vec<usize>,
    /// Where reading closed this section (`usize::MAX` at the end).
    pub closed_at: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Numbered {
    No,
    Yes,
    /// A special section numbered as a chapter (`sectnums=all` in a book).
    Chapter,
}

impl Section {
    /// Ruby's `initialize_section` for a title line at `raw_level` with block
    /// `style`, nested in `parent` (`None` for the document). `synopsis`
    /// marks a manual page's SYNOPSIS title, special without a style.
    pub fn read(
        style: Option<&str>,
        raw_level: u8,
        book: bool,
        synopsis: bool,
        attributes: &Attributes,
        parent: Option<&Section>,
    ) -> Self {
        let sectnums = attributes.get("sectnums");
        let mut level = raw_level;
        let (sectname, special) = match style {
            Some("abstract") if book => {
                level = 1;
                ("chapter".to_owned(), false)
            }
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
            // A manual page's unstyled SYNOPSIS section.
            None if synopsis => ("synopsis".to_owned(), true),
            None => ("section".to_owned(), false),
        };
        if special && style.is_some() {
            level = level.max(1);
        }
        let inherited = parent.is_some_and(|parent| parent.special);
        let numbered = if special {
            match sectnums {
                _ if sectname == "appendix" => Numbered::Yes,
                Some("all") if book && level == 1 => Numbered::Chapter,
                Some("all") => Numbered::Yes,
                _ => Numbered::No,
            }
        } else if sectnums.is_some() && level > 0 {
            // A section nested in a special one is numbered with its parent.
            match parent {
                Some(parent) if inherited && !parent.is_numbered() => Numbered::No,
                _ => Numbered::Yes,
            }
        } else if book && level == 0 && attributes.get("partnums").is_some() {
            Numbered::Yes
        } else {
            Numbered::No
        };
        Self {
            sectname,
            numbered,
            book,
            level,
            raw_level,
            special: special || inherited,
            numeral: None,
            caption: None,
            path: Vec::new(),
            closes: Vec::new(),
            closed_at: usize::MAX,
        }
    }

    /// The section as a label produced while reading at `position` sees it:
    /// numerals of sections not closed yet are empty.
    pub fn at(&self, position: usize) -> Self {
        let mut section = self.clone();
        for (numeral, closed) in section.path.iter_mut().zip(&self.closes) {
            if *closed > position {
                numeral.clear();
            }
        }
        section
    }

    /// Whether closing this section advances the `appendix-number` counter.
    pub fn counts_appendix(&self) -> bool {
        self.is_numbered() && self.sectname == "appendix"
    }

    /// Ruby's `next_section` lets only appendices, prefaces and abstracts
    /// among level-1 special sections contain sections.
    pub fn allows_nesting(&self) -> bool {
        !(self.level == 1
            && self.special
            && !matches!(self.sectname.as_str(), "appendix" | "preface" | "abstract"))
    }

    /// Whether closing this section advances the `chapter-number` counter.
    pub fn counts_chapter(&self) -> bool {
        !self.counts_appendix()
            && (self.numbered == Numbered::Chapter
                || (self.numbered == Numbered::Yes && self.sectname == "chapter"))
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
        if self.caption.is_some() {
            return self.caption.clone();
        }
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
    /// Ruby's `assign_numeral`: chapters and appendices take their counter
    /// value, parts a Roman ordinal, other sections the next ordinal among
    /// their numbered siblings. `None` for an unnumbered section.
    pub fn assign(&mut self, section: &Section) -> Option<String> {
        if !section.is_numbered() {
            return None;
        }
        if section.counts_chapter() || section.counts_appendix() {
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
            let section = Section::read(style, level, book, false, attrs, None);
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
    fn special_sections_adjust_levels_and_pass_specialness_to_children() {
        let on = attributes(&[("sectnums", "")]);
        let appendix = Section::read(Some("appendix"), 0, false, false, &Attributes::new(), None);
        assert_eq!((appendix.level, appendix.raw_level), (1, 0));
        assert!(appendix.special && appendix.counts_appendix());
        assert!(!appendix.counts_chapter());
        let abstract_ = Section::read(Some("abstract"), 3, true, false, &on, None);
        assert_eq!(
            (abstract_.level, abstract_.sectname.as_str()),
            (1, "chapter")
        );
        assert!(!abstract_.special && abstract_.counts_chapter());
        // A child of a special section is special; it is numbered only when
        // its parent is.
        let child = Section::read(None, 2, false, false, &on, Some(&appendix));
        assert!(child.special && child.is_numbered());
        let preface = Section::read(Some("preface"), 1, true, false, &on, None);
        let child = Section::read(None, 2, true, false, &on, Some(&preface));
        assert!(child.special && !child.is_numbered());
        // Only appendices, prefaces and abstracts contain sections.
        assert!(preface.allows_nesting() && appendix.allows_nesting());
        assert!(!Section::read(Some("glossary"), 1, false, false, &on, None).allows_nesting());
        assert!(Section::read(Some("glossary"), 2, false, false, &on, None).allows_nesting());
    }

    #[test]
    fn labels_while_reading_see_only_closed_sections() {
        let mut section = Section::read(
            None,
            2,
            false,
            false,
            &attributes(&[("sectnums", "")]),
            None,
        );
        section.path = vec!["1".into(), "2".into()];
        section.closes = vec![50, 20];
        assert_eq!(section.at(30).sectnum(".", ""), ".2");
        assert_eq!(section.at(10).sectnum(".", ""), ".");
        assert_eq!(section.at(50).sectnum(".", ""), "1.2");
    }

    #[test]
    fn captions_follow_levels_and_signifiers() {
        let mut section = Section::read(
            None,
            2,
            false,
            false,
            &attributes(&[("sectnums", "")]),
            None,
        );
        section.path = vec!["1".into(), "2".into()];
        assert_eq!(
            section.caption(&Attributes::new()).as_deref(),
            Some("1.2. ")
        );
        let one = attributes(&[("sectnumlevels", "1")]);
        assert_eq!(section.caption(&one), None);
        assert_eq!(section.sectnum(".", ","), "1.2,");
        let mut chapter =
            Section::read(None, 1, true, false, &attributes(&[("sectnums", "")]), None);
        chapter.path = vec!["3".into()];
        let signifier = attributes(&[("chapter-signifier", "Chapter")]);
        assert_eq!(chapter.caption(&signifier).as_deref(), Some("Chapter 3. "));
        let mut part = Section::read(None, 0, true, false, &attributes(&[("partnums", "")]), None);
        part.path = vec!["II".into()];
        assert_eq!(part.caption(&Attributes::new()).as_deref(), Some("II: "));
        // An appendix caption is fixed at closing, beyond sectnumlevels.
        let mut appendix =
            Section::read(Some("appendix"), 1, false, false, &Attributes::new(), None);
        appendix.caption = Some("Приложение A: ".into());
        let none = attributes(&[("sectnumlevels", "0")]);
        assert_eq!(appendix.caption(&none).as_deref(), Some("Приложение A: "));
    }
}
