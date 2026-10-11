//! Header metadata, following Asciidoctor 2.0.23 Parser.parse_header_metadata
//! and process_authors. Kept separate from block parsing so the tokenizer uses
//! exactly the same line boundaries and attribute transitions.
use crate::comments::skip_comment;
use crate::context::{parse_attribute_entry, AttributeEntry, ParseContext};
use crate::parser::parse_heading;
use crate::{Author, DocumentHeader, Revision, Span};

pub(crate) struct Header {
    pub title_line: Option<usize>,
    pub author_line: Option<usize>,
    pub revision_line: Option<usize>,
    pub body_start: usize,
    pub metadata: Option<DocumentHeader>,
}

pub(crate) fn collect_header(lines: &[(usize, &str)], context: &mut ParseContext) -> Header {
    let mut i = 0;
    while i < lines.len() {
        if skip_comment(lines, &mut i, context) {
            continue;
        }
        if let Some(entry) = parse_attribute_entry(lines[i].1) {
            context.apply(entry);
        } else if !lines[i].1.trim().is_empty() {
            break;
        }
        i += 1;
    }
    let mut result = Header {
        title_line: None,
        author_line: None,
        revision_line: None,
        body_start: i,
        metadata: None,
    };
    let mut implicit_author = None;
    let mut implicit_authors = None;
    let mut implicit_initials = None;
    if i < lines.len() && matches!(parse_heading(lines[i].1), Some((0, _))) {
        result.title_line = Some(i);
        let title_span = line_span(lines[i]);
        i += 1;
        consume_attributes(lines, &mut i, context);
        let mut authors = Vec::new();
        let mut revision = None;
        if i < lines.len() && !lines[i].1.trim().is_empty() {
            result.author_line = Some(i);
            authors = parse_authors(lines[i].1, lines[i].0, false);
            install_authors(&authors, context, false);
            implicit_author = context.attributes.get("author").map(str::to_owned);
            implicit_authors = context.attributes.get("authors").map(str::to_owned);
            implicit_initials = context.attributes.get("authorinitials").map(str::to_owned);
            i += 1;
            consume_attributes(lines, &mut i, context);
            if i < lines.len() && !lines[i].1.trim().is_empty() {
                revision = parse_revision(lines[i]);
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

fn consume_attributes(lines: &[(usize, &str)], i: &mut usize, context: &mut ParseContext) {
    while let Some((_, line)) = lines.get(*i) {
        if skip_comment(lines, i, context) {
            continue;
        }
        let Some(entry) = parse_attribute_entry(line) else {
            break;
        };
        context.apply(entry);
        *i += 1;
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
