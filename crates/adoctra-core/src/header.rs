//! Header metadata, following Asciidoctor 2.0.23 Parser.parse_header_metadata
//! and process_authors. Kept separate from block parsing so the tokenizer uses
//! exactly the same line boundaries and attribute transitions.
use crate::attributes::reference_text;
use crate::author_values::{derive_authors, split_authors};
use crate::comments::skip_comment;
use crate::context::{consume_attribute_entry, AttributeEntry, ParseContext};
use crate::parser::parse_heading;
use crate::reader::Reader;
use crate::{Attributes, Author, DocumentHeader, Revision, Span};
use std::sync::Arc;

pub(crate) struct HeaderTitle {
    pub text: String,
    pub span: Span,
    /// A replacement is attributed to its whole entry, not invented offsets
    /// into the transformed value. Host values use an empty insertion anchor.
    pub replacement: bool,
}

pub(crate) struct Header {
    pub title_line: Option<usize>,
    pub title_underline: Option<usize>,
    pub author_line: Option<usize>,
    pub revision_line: Option<usize>,
    pub body_start: usize,
    pub metadata: Option<DocumentHeader>,
    pub effective_title: Option<HeaderTitle>,
    pub pending: crate::parser::PendingMeta,
    pub metadata_for_title: bool,
    /// The id of a trailing ` [[id]]` on the document title.
    pub title_anchor: Option<String>,
}

pub(crate) fn collect_header(lines: &mut Reader<'_>, context: &mut ParseContext) -> Header {
    let mut i = 0;
    let mut pending = crate::parser::PendingMeta::default();
    while lines.has(i, context) {
        if crate::parser::consume_block_metadata(lines, &mut i, context, &mut pending) {
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
        metadata_for_title: false,
        title_anchor: None,
        pending,
        effective_title: context
            .attributes
            .get("doctitle")
            .filter(|s| !s.is_empty())
            .map(|s| HeaderTitle {
                text: s.to_owned(),
                span: context.doctitle_span.unwrap_or_else(|| Span::new(0, 0)),
                replacement: true,
            }),
    };
    let mut implicit_author = None;
    let mut implicit_authors = None;
    let mut implicit_initials = None;
    let title = if !matches!(
        result.pending.attrs.style.as_deref(),
        Some("comment" | "discrete" | "float")
    ) && lines.has(i, context)
    {
        // Ruby peeks two processed lines even for ATX. Directives immediately
        // following it therefore see the context before the implicit doctitle.
        let has_next = lines.has(i + 1, context);
        if let Some((level, _)) = parse_heading(&lines[i].1) {
            Some((level, false))
        } else if has_next {
            setext_level(&lines[i].1, &lines[i + 1].1).map(|level| (level, true))
        } else {
            None
        }
    } else {
        None
    };
    if let Some((_, setext)) = title
        .filter(|(level, _)| context.section_level(*level) == 0 && result.pending.title.is_none())
    {
        result.metadata_for_title = true;
        result.title_line = Some(i);
        let mut title_span = line_span((lines[i].0, &lines[i].1));
        let declared = if setext {
            lines[i].1.as_ref()
        } else {
            parse_heading(&lines[i].1).unwrap().1
        };
        // A trailing ` [[id]]` always leaves the document title; its id applies
        // unless metadata supplied one (its reftext is ignored, as in Ruby).
        let anchor = crate::parser::section_anchor(declared);
        let declared = anchor.as_ref().map_or(declared, |anchor| anchor.title);
        result.title_anchor = anchor.map(|anchor| anchor.id.to_owned());
        let text_start = lines[i].0 + (declared.as_ptr() as usize - lines[i].1.as_ptr() as usize);
        let declared_title = declared.to_owned();
        if result.effective_title.is_none() {
            result.effective_title = Some(HeaderTitle {
                text: declared_title.clone(),
                span: Span::new(text_start, text_start + declared_title.len()),
                replacement: false,
            });
            let value = context.implicit_doctitle(
                &declared_title,
                Span::new(text_start, text_start + declared_title.len()),
            );
            // The implicit title also replaces a locked empty host value.
            Arc::make_mut(&mut context.attributes).set("doctitle", value);
        }
        let initial_doctitle = context.attributes.get("doctitle").unwrap().to_owned();
        let initial_span = context.doctitle_span;
        if setext {
            i += 1;
            result.title_underline = Some(i);
            title_span.end = lines[i].0 + lines[i].1.len();
            context.apply(AttributeEntry::Set("compat-mode".to_owned(), String::new()));
        }
        // Ruby installs document metadata after the implicit doctitle a-stage.
        if !result.pending.roles.is_empty() {
            Arc::make_mut(&mut context.attributes).set("role", result.pending.roles.join(" "));
        }
        if let Some(reftext) = &result.pending.reftext {
            Arc::make_mut(&mut context.attributes).set_parts(
                "reftext",
                vec![crate::AttributePart::Raw(reftext.value.clone())],
            );
        }
        i += 1;
        consume_attributes(lines, &mut i, context);
        let mut authors = Vec::new();
        let mut revision = None;
        if lines.has(i, context) && !lines[i].1.trim().is_empty() {
            result.author_line = Some(i);
            authors = parse_authors(&lines[i].1, lines[i].0);
            let values = author_attributes(&authors);
            Arc::make_mut(&mut context.attributes).set("authorcount", authors.len().to_string());
            for (key, value) in values.iter().filter(|(key, _)| *key != "authorcount") {
                set_implicit(context, key, value);
            }
            implicit_author = context
                .attributes
                .value_parts("author")
                .map(|p| reference_text(&p));
            implicit_authors = context
                .attributes
                .value_parts("authors")
                .map(|p| reference_text(&p));
            implicit_initials = context
                .attributes
                .value_parts("authorinitials")
                .map(|p| reference_text(&p));
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
            title: declared_title,
            authors,
            revision,
        });
        result.body_start = i;
        match context.attributes.get("doctitle") {
            None | Some("") => {
                Arc::make_mut(&mut context.attributes).set("doctitle", initial_doctitle);
                context.doctitle_span = initial_span;
            }
            Some(value) if value != initial_doctitle => {
                result.effective_title = Some(HeaderTitle {
                    text: value.to_owned(),
                    span: context.doctitle_span.unwrap_or_else(|| Span::new(0, 0)),
                    replacement: true,
                });
            }
            _ => {}
        }
    }
    // Explicit name attributes have priority over the contiguous author_N list.
    // Compare indexed values to source declarations, not the effective attributes
    // captured after installing defaults (which may already contain overrides).
    let explicit = context
        .attributes
        .value_parts("author")
        .filter(|value| Some(reference_text(value)) != implicit_author)
        .map(|_| ("author", false))
        .or_else(|| {
            context
                .attributes
                .value_parts("authors")
                .filter(|value| Some(reference_text(value)) != implicit_authors)
                .map(|_| ("authors", true))
        });
    let mut derived = if let Some((key, multiple)) = explicit {
        let initials = context
            .attributes
            .value_parts("authorinitials")
            .map(|p| reference_text(&p));
        let preserve_initials = !multiple && initials != implicit_initials;
        let value = context.attributes.value_parts(key).unwrap();
        let mut values = derive_authors(split_authors(&value, multiple));
        if preserve_initials {
            values.remove("authorinitials");
        }
        Some(values)
    } else if let Some(header) = &result.metadata {
        let implicit = author_attributes(&header.authors);
        let mut authors = Vec::new();
        let mut changed = false;
        for index in 1.. {
            let key = format!("author_{index}");
            if context.attributes.get(&key).is_none() {
                break;
            }
            let parts = context.attributes.value_parts(&key).unwrap();
            if Some(reference_text(&parts).as_str()) == implicit.get(&key) {
                // Rebuilding unchanged names preserves compound name components.
                let author = &header.authors[index - 1];
                let name = [
                    Some(author.firstname.as_str()),
                    author.middlename.as_deref(),
                    author.lastname.as_deref(),
                ]
                .into_iter()
                .flatten()
                .map(|s| s.replace(' ', "_"))
                .collect::<Vec<_>>()
                .join(" ");
                authors.push(vec![crate::AttributePart::Text(name)]);
            } else {
                changed = true;
                authors.push(parts);
            }
        }
        changed.then(|| derive_authors(authors))
    } else {
        None
    };
    let has_declarations = result.author_line.is_some();
    // Empty overrides do not erase an implicit author line. Without one, Ruby
    // records a zero count, including documents with no title or author fields.
    if derived.as_ref().and_then(|a| a.get("authorcount")) == Some("0") {
        derived = None;
    }
    if let Some(values) = derived {
        // These are derived metadata, not document attribute entries: Ruby merges
        // them even over locked host fields, without a second substitution pass.
        let attributes = Arc::make_mut(&mut context.attributes);
        for (key, _) in values.iter() {
            attributes.set_parts(key, values.value_parts(key).unwrap());
        }
        if result.metadata.is_some() && attributes.get("email").is_none() {
            if let Some(email) = attributes.value_parts("email_1") {
                attributes.set_parts("email", email);
            }
        }
    } else if !has_declarations {
        Arc::make_mut(&mut context.attributes).set("authorcount", "0");
    }
    save_toc(Arc::make_mut(&mut context.attributes));
    save_icons(Arc::make_mut(&mut context.attributes));
    context.quote_compat = context.attributes.get("compat-mode").is_some();
    result
}

/// Ruby's `save_attributes` at header completion: an `icons` value other
/// than empty or `font` names the icon type (`image` keeps the default) and
/// leaves image icons enabled, unless `icontype` is already set.
fn save_icons(attributes: &mut crate::Attributes) {
    let Some(icons) = attributes.get("icons").map(str::to_owned) else {
        return;
    };
    if attributes.get("icontype").is_some() || icons.is_empty() || icons == "font" {
        return;
    }
    attributes.set("icons", "");
    if icons != "image" {
        attributes.set("icontype", icons);
    }
}

/// Ruby's `save_attributes` at header completion: a `toc` value (or `toc2`)
/// names the TOC position. Derived values bypass locks, as in Ruby.
fn save_toc(attributes: &mut crate::Attributes) {
    let toc = match attributes.remove("toc2") {
        Some(_) => "left".to_string(),
        None => match attributes.get("toc") {
            Some(toc) => toc.to_owned(),
            None => return,
        },
    };
    let position = match attributes.get("toc-placement").unwrap_or("macro") {
        "auto" => attributes
            .get("toc-position")
            .unwrap_or_default()
            .to_owned(),
        placement => placement.to_owned(),
    };
    if toc.is_empty() && position.is_empty() {
        return;
    }
    let position = match (position.is_empty(), toc.is_empty()) {
        (false, _) => position,
        (true, true) => "left".to_string(),
        (true, false) => toc,
    };
    attributes.set("toc", "");
    attributes.set("toc-placement", "auto");
    let side = match position.as_str() {
        "left" | "<" | "&lt;" => Some("left"),
        "right" | ">" | "&gt;" => Some("right"),
        "top" | "^" => Some("top"),
        "bottom" | "v" => Some("bottom"),
        "preamble" | "macro" => {
            attributes.set("toc-position", "content");
            attributes.set("toc-placement", position.as_str());
            None
        }
        _ => {
            attributes.remove("toc-position");
            None
        }
    };
    if let Some(side) = side {
        attributes.set("toc-position", side);
        if attributes.get("toc-class").is_none() {
            attributes.set("toc-class", "toc2");
        }
    }
}

/// Asciidoctor's setext_section_title?: character lengths, not byte lengths.
/// Shared by header detection and body sections.
pub(crate) fn setext_level(title: &str, underline: &str) -> Option<u8> {
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
        && title.chars().any(crate::unicode_word::is_alphanumeric)
        && underline.bytes().all(|byte| byte == marker)
        && title.chars().count().abs_diff(underline.len()) < 2)
        .then_some(level)
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
        let parts = crate::context::substitute_attribute_parts(value, &context.attributes, false);
        Arc::make_mut(&mut context.attributes).set_parts(key, parts);
    }
}

fn author_attributes(authors: &[Author]) -> Attributes {
    let mut values = Attributes::new();
    values.set("authorcount", authors.len().to_string());
    if !authors.is_empty() {
        values.set(
            "authors",
            authors
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
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
                    values.set(format!("{key}{suffix}"), value);
                }
            }
        }
    }
    values
}

fn parse_authors(line: &str, off: usize) -> Vec<Author> {
    let mut authors = Vec::new();
    let mut start = 0;
    for (pos, ch) in line.char_indices() {
        if ch == ';' && (pos + 1 == line.len() || line[pos + 1..].starts_with(' ')) {
            if pos > start {
                authors.push(parse_author(&line[start..pos], off + start));
            }
            start = (pos + 2).min(line.len());
        }
    }
    if start < line.len() {
        authors.push(parse_author(&line[start..], off + start));
    }
    authors
}

fn parse_author(entry: &str, off: usize) -> Author {
    let (name, email) = if entry.ends_with('>') {
        entry
            .rsplit_once(" <")
            .filter(|(_, email)| email.len() > 1 && !email[..email.len() - 1].contains('>'))
            .map_or((entry, None), |(name, email)| {
                (name, Some(email[..email.len() - 1].to_owned()))
            })
    } else {
        (entry, None)
    };
    let parts: Vec<_> = name.split(' ').filter(|s| !s.is_empty()).collect();
    let valid = !name.starts_with(' ')
        && !name.ends_with(' ')
        && !parts.is_empty()
        && parts.len() <= 3
        && parts.iter().all(|p| {
            p.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                && p.chars().all(|c| c.is_alphanumeric() || "_-'.".contains(c))
        });
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
