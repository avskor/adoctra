//! Header metadata, following Asciidoctor 2.0.23 Parser.parse_header_metadata
//! and process_authors. Kept separate from block parsing so the tokenizer uses
//! exactly the same line boundaries and attribute transitions.
use crate::comments::skip_comment;
use crate::context::{consume_attribute_entry, AttributeEntry, ParseContext};
use crate::parser::parse_heading;
use crate::reader::Reader;
use crate::{Author, DocumentHeader, Revision, Span};

pub(crate) struct Header {
    pub title_line: Option<usize>,
    pub title_underline: Option<usize>,
    pub author_line: Option<usize>,
    pub revision_line: Option<usize>,
    pub body_start: usize,
    pub metadata: Option<DocumentHeader>,
}

pub(crate) fn collect_header(lines: &mut Reader<'_>, context: &mut ParseContext) -> Header {
    let mut i = 0;
    while lines.has(i, context) {
        if skip_comment(lines, &mut i, context) {
            continue;
        }
        if consume_attribute_entry(lines, &mut i, context) {
            continue;
        } else if !lines[i].1.trim().is_empty() {
            break;
        }
        i += 1;
    }
    let mut result = Header {
        title_line: None,
        title_underline: None,
        author_line: None,
        revision_line: None,
        body_start: i,
        metadata: None,
    };
    let mut implicit_author = None;
    let mut implicit_authors = None;
    let mut implicit_initials = None;
    let title = if lines.has(i, context) {
        if let Some((level, _)) = parse_heading(&lines[i].1) {
            Some((level, false))
        } else if lines.has(i + 1, context) {
            setext_level(&lines[i].1, &lines[i + 1].1).map(|level| (level, true))
        } else {
            None
        }
    } else {
        None
    };
    if let Some((_, setext)) = title.filter(|(level, _)| context.section_level(*level) == 0) {
        result.title_line = Some(i);
        let mut title_span = line_span((lines[i].0, &lines[i].1));
        if setext {
            i += 1;
            result.title_underline = Some(i);
            title_span.end = lines[i].0 + lines[i].1.len();
            context.apply(AttributeEntry::Set("compat-mode".to_owned(), String::new()));
        }
        i += 1;
        consume_attributes(lines, &mut i, context);
        let mut authors = Vec::new();
        let mut revision = None;
        if lines.has(i, context) && !lines[i].1.trim().is_empty() {
            result.author_line = Some(i);
            authors = parse_authors(&lines[i].1, lines[i].0, false);
            install_authors(&authors, context, false);
            implicit_author = context.attributes.get("author").map(str::to_owned);
            implicit_authors = context.attributes.get("authors").map(str::to_owned);
            implicit_initials = context.attributes.get("authorinitials").map(str::to_owned);
            i += 1;
            consume_attributes(lines, &mut i, context);
            if lines.has(i, context) && !lines[i].1.trim().is_empty() {
                revision = parse_revision((lines[i].0, &lines[i].1));
                if let Some(rev) = &revision {
                    result.revision_line = Some(i);
                    for (key, value) in [
                        ("revnumber", &rev.number),
                        ("revdate", &rev.date),
                        ("revremark", &rev.remark),
                    ] {
                        if let Some(value) = value {
                            set_implicit(context, key, value);
                        }
                    }
                    i += 1;
                }
            }
            consume_attributes(lines, &mut i, context);
        }
        let end = lines[i - 1].0 + lines[i - 1].1.len();
        let start = lines
            .loaded
            .iter()
            .find(|(_, line)| !line.trim().is_empty())
            .map_or(title_span.start, |(off, _)| *off);
        result.metadata = Some(DocumentHeader {
            span: Span::new(start, end),
            title_span,
            authors,
            revision,
        });
        result.body_start = i;
    }
    // Explicit name attributes replace derived name fields; emails are separate
    // attributes in this form. Indexed author_N overrides remain follow-up work.
    let explicit = context
        .attributes
        .get("author")
        .filter(|value| Some(*value) != implicit_author.as_deref())
        .map(|value| (value.to_owned(), false))
        .or_else(|| {
            context
                .attributes
                .get("authors")
                .filter(|value| Some(*value) != implicit_authors.as_deref())
                .map(|value| (value.to_owned(), true))
        });
    if let Some((value, multiple)) = explicit {
        let initials = context.attributes.get("authorinitials").map(str::to_owned);
        let preserve_initials = !multiple && initials != implicit_initials;
        let authors = if multiple {
            parse_authors(&value, 0, true)
        } else {
            vec![parse_author(&value, 0, true)]
        };
        install_authors(&authors, context, true);
        if preserve_initials {
            context.apply(match initials {
                Some(value) => AttributeEntry::Set("authorinitials".to_owned(), value),
                None => AttributeEntry::Unset("authorinitials".to_owned()),
            });
        }
    }
    result
}

/// Asciidoctor's setext_section_title?: character lengths, not byte lengths.
/// Only header detection uses this for now; body setext sections are separate work.
fn setext_level(title: &str, underline: &str) -> Option<u8> {
    let marker = *underline.as_bytes().first()?;
    let level = match marker {
        b'=' => 0,
        b'-' => 1,
        b'~' => 2,
        b'^' => 3,
        b'+' => 4,
        _ => return None,
    };
    (!title.starts_with('.')
        && title.chars().any(setext_alphanumeric)
        && underline.bytes().all(|byte| byte == marker)
        && title.chars().count().abs_diff(underline.len()) < 2)
        .then_some(level)
}

// Ruby Alnum is Alphabetic or Decimal_Number. Rust is_alphanumeric also
// accepts other numbers (e.g. superscripts and fractions). Decimal ranges from
// the reference Ruby's Unicode Nd property; keep core dependency-free.
fn setext_alphanumeric(ch: char) -> bool {
    ch.is_alphabetic()
        || matches!(
            ch as u32,
            0x30..=0x39 | 0x660..=0x669 | 0x6f0..=0x6f9
            | 0x7c0..=0x7c9 | 0x966..=0x96f | 0x9e6..=0x9ef
            | 0xa66..=0xa6f | 0xae6..=0xaef | 0xb66..=0xb6f
            | 0xbe6..=0xbef | 0xc66..=0xc6f | 0xce6..=0xcef
            | 0xd66..=0xd6f | 0xde6..=0xdef | 0xe50..=0xe59
            | 0xed0..=0xed9 | 0xf20..=0xf29 | 0x1040..=0x1049
            | 0x1090..=0x1099 | 0x17e0..=0x17e9 | 0x1810..=0x1819
            | 0x1946..=0x194f | 0x19d0..=0x19d9 | 0x1a80..=0x1a89
            | 0x1a90..=0x1a99 | 0x1b50..=0x1b59 | 0x1bb0..=0x1bb9
            | 0x1c40..=0x1c49 | 0x1c50..=0x1c59 | 0xa620..=0xa629
            | 0xa8d0..=0xa8d9 | 0xa900..=0xa909 | 0xa9d0..=0xa9d9
            | 0xa9f0..=0xa9f9 | 0xaa50..=0xaa59 | 0xabf0..=0xabf9
            | 0xff10..=0xff19 | 0x104a0..=0x104a9 | 0x10d30..=0x10d39
            | 0x11066..=0x1106f | 0x110f0..=0x110f9 | 0x11136..=0x1113f
            | 0x111d0..=0x111d9 | 0x112f0..=0x112f9 | 0x11450..=0x11459
            | 0x114d0..=0x114d9 | 0x11650..=0x11659 | 0x116c0..=0x116c9
            | 0x11730..=0x11739 | 0x118e0..=0x118e9 | 0x11950..=0x11959
            | 0x11c50..=0x11c59 | 0x11d50..=0x11d59 | 0x11da0..=0x11da9
            | 0x11f50..=0x11f59 | 0x16a60..=0x16a69 | 0x16ac0..=0x16ac9
            | 0x16b50..=0x16b59 | 0x1d7ce..=0x1d7ff | 0x1e140..=0x1e149
            | 0x1e2f0..=0x1e2f9 | 0x1e4f0..=0x1e4f9 | 0x1e950..=0x1e959
            | 0x1fbf0..=0x1fbf9
        )
}

fn consume_attributes(lines: &mut Reader<'_>, i: &mut usize, context: &mut ParseContext) {
    while lines.has(*i, context) {
        if skip_comment(lines, i, context) {
            continue;
        }
        if !consume_attribute_entry(lines, i, context) {
            break;
        }
    }
}

fn line_span((off, line): (usize, &str)) -> Span {
    Span::new(off, off + line.len())
}

fn set_implicit(context: &mut ParseContext, key: &str, value: &str) {
    if context.attributes.get(key).is_none() {
        context.apply(AttributeEntry::Set(key.to_owned(), value.to_owned()));
    }
}

fn install_authors(authors: &[Author], context: &mut ParseContext, replace: bool) {
    let mut values = vec![
        ("authorcount".to_owned(), authors.len().to_string()),
        (
            "authors".to_owned(),
            authors
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ),
    ];
    for (i, author) in authors.iter().enumerate() {
        let mut suffixes = vec![if i == 0 {
            String::new()
        } else {
            format!("_{}", i + 1)
        }];
        if i == 0 && authors.len() > 1 {
            suffixes.push("_1".to_owned());
        }
        for suffix in suffixes {
            for (key, value) in [
                ("author", Some(&author.name)),
                ("firstname", Some(&author.firstname)),
                ("middlename", author.middlename.as_ref()),
                ("lastname", author.lastname.as_ref()),
                ("authorinitials", Some(&author.initials)),
                ("email", author.email.as_ref()),
            ] {
                if let Some(value) = value {
                    values.push((format!("{key}{suffix}"), value.clone()));
                }
            }
        }
    }
    for (key, value) in values {
        if replace {
            context.apply(AttributeEntry::Set(key, value));
        } else {
            set_implicit(context, &key, &value);
        }
    }
}

fn parse_authors(line: &str, off: usize, names_only: bool) -> Vec<Author> {
    let mut authors = Vec::new();
    let mut start = 0;
    for (pos, ch) in line.char_indices() {
        if ch == ';' && (pos + 1 == line.len() || line[pos + 1..].starts_with(' ')) {
            if pos > start {
                authors.push(parse_author(&line[start..pos], off + start, names_only));
            }
            start = (pos + 2).min(line.len());
        }
    }
    if start < line.len() {
        authors.push(parse_author(&line[start..], off + start, names_only));
    }
    authors
}

fn parse_author(entry: &str, off: usize, names_only: bool) -> Author {
    let (name, email) = if !names_only && entry.ends_with('>') {
        entry
            .rsplit_once(" <")
            .filter(|(_, email)| email.len() > 1 && !email[..email.len() - 1].contains('>'))
            .map_or((entry, None), |(name, email)| {
                (name, Some(email[..email.len() - 1].to_owned()))
            })
    } else {
        (entry, None)
    };
    let parts: Vec<_> = if names_only {
        name.split_whitespace().collect()
    } else {
        name.split(' ').filter(|s| !s.is_empty()).collect()
    };
    let valid = names_only
        || (!name.starts_with(' ')
            && !name.ends_with(' ')
            && !parts.is_empty()
            && parts.len() <= 3
            && parts.iter().all(|p| {
                p.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                    && p.chars().all(|c| c.is_alphanumeric() || "_-'.".contains(c))
            }));
    let parts: Vec<String> = if valid && !parts.is_empty() {
        let mut result: Vec<_> = parts.iter().take(2).map(|p| p.replace('_', " ")).collect();
        if parts.len() > 2 {
            result.push(parts[2..].join(" ").replace('_', " "));
        }
        result
    } else {
        vec![entry
            .split(' ')
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_owned()]
    };
    Author {
        span: Span::new(off, off + entry.len()),
        name: parts.join(" "),
        firstname: parts[0].clone(),
        middlename: (parts.len() == 3).then(|| parts[1].clone()),
        lastname: (parts.len() > 1).then(|| parts[parts.len() - 1].clone()),
        initials: parts.iter().filter_map(|p| p.chars().next()).collect(),
        email: if valid { email } else { None },
    }
}

fn parse_revision(line: (usize, &str)) -> Option<Revision> {
    let text = line.1.trim_start_matches(' ');
    if text.starts_with(':') {
        return None;
    }
    let (number, rest) = if let Some((prefix, rest)) = text.split_once(',') {
        let start = prefix
            .find(|c: char| c.is_ascii_digit() || c == '{')
            .unwrap_or(prefix.len());
        (
            Some(prefix[start..].trim_end().to_owned()),
            rest.trim_start_matches(' '),
        )
    } else {
        (None, text)
    };
    if rest.starts_with(':') {
        return None;
    }
    let (date, remark) = rest.split_once(':').map_or((rest, None), |(date, remark)| {
        (
            date.trim_end().trim_end_matches(','),
            Some(remark.trim().to_owned()),
        )
    });
    let mut number = number;
    let date = if number.is_none() && date.trim().starts_with('v') {
        number = Some(date.trim()[1..].to_owned());
        None
    } else {
        (!date.trim().is_empty()).then(|| date.trim().to_owned())
    };
    Some(Revision {
        span: line_span(line),
        number,
        date,
        remark,
    })
}
