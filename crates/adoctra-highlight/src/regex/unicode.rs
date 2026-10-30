//! Character tables: Onigmo's ASCII class escapes, the Unicode properties
//! and POSIX brackets of the reference Ruby (generated into
//! `unicode_tables.rs`) and simple case folding.

#[rustfmt::skip]
#[allow(clippy::unreadable_literal)]
mod tables {
    include!("unicode_tables.rs");
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Table {
    /// `\w`: `[a-zA-Z0-9_]`.
    Word,
    /// `\d`: `[0-9]`.
    Digit,
    /// `\s`: `[ \t\n\v\f\r]`.
    Space,
    /// `\h`: `[0-9a-fA-F]`.
    Hex,
    Ranges(&'static [(u32, u32)]),
}

impl Table {
    /// `\w`, `\d`, `\s` or `\h`, which case folding never extends.
    pub(crate) fn is_ascii_escape(self) -> bool {
        !matches!(self, Table::Ranges(_))
    }

    pub(crate) fn contains(self, c: char) -> bool {
        match self {
            Table::Word => c.is_ascii_alphanumeric() || c == '_',
            Table::Digit => c.is_ascii_digit(),
            Table::Space => matches!(c, ' ' | '\t'..='\r'),
            Table::Hex => c.is_ascii_hexdigit(),
            Table::Ranges(ranges) => in_ranges(ranges, c as u32),
        }
    }
}

fn in_ranges(ranges: &[(u32, u32)], cp: u32) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                std::cmp::Ordering::Less
            } else if lo > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// The ASCII class escapes `\w`, `\d`, `\s` and `\h`.
pub(crate) fn builtin(name: &str) -> Table {
    match name {
        "w" => Table::Word,
        "d" => Table::Digit,
        "s" => Table::Space,
        _ => Table::Hex,
    }
}

fn named(key: &str) -> Option<Table> {
    tables::NAMED
        .binary_search_by(|(name, _)| name.cmp(&key))
        .ok()
        .map(|index| Table::Ranges(tables::NAMED[index].1))
}

/// A `\p{…}` property (Onigmo ignores case, spaces, `-` and `_`).
pub(crate) fn property(name: &str) -> Option<Table> {
    let wanted = loose(name);
    tables::NAMED
        .iter()
        .filter_map(|(key, ranges)| key.strip_prefix("p:").map(|name| (name, ranges)))
        .find(|(key, _)| loose(key) == wanted)
        .map(|(_, ranges)| Table::Ranges(ranges))
}

fn loose(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// A `[:name:]` POSIX bracket.
pub(crate) fn posix(name: &str) -> Option<Table> {
    named(&format!("posix:{name}"))
}

/// The Unicode Word property, which `\b` uses.
pub(crate) fn is_word(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    in_ranges(tables::PROP_WORD, c as u32)
}

/// The simple case fold of a scalar.
pub(crate) fn fold(c: char) -> char {
    match tables::FOLD.binary_search_by_key(&(c as u32), |&(from, _)| from) {
        Ok(index) => char::from_u32(tables::FOLD[index].1).unwrap_or(c),
        Err(_) => c,
    }
}

/// The other scalars with the same simple case fold as `c`.
pub(crate) fn variants(c: char) -> impl Iterator<Item = char> {
    let folded = fold(c);
    let others = tables::FOLD
        .iter()
        .filter(move |&&(_, to)| to == folded as u32)
        .filter_map(|&(from, _)| char::from_u32(from));
    std::iter::once(folded)
        .chain(others)
        .filter(move |&v| v != c)
}
