//! Parse configuration and attribute state shared by recursive block parsing.

use crate::attributes::{parts_text, push_part, push_value};
use crate::{AttributePart, Attributes, SourceId};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    /// SourceMap id for this input. The convenience APIs use source zero.
    pub source_id: SourceId,
    /// Hard attribute overrides: document assignments cannot change these keys.
    /// This initial API supports set overrides; soft defaults and locked unsets
    /// will be added alongside the remaining Asciidoctor option semantics.
    pub attributes: Attributes,
    /// Limits for explicit resource parsing; ordinary string parsing performs no loads.
    pub includes: IncludeLimits,
}

impl ParseOptions {
    /// File-derived attributes are hard overrides, including over caller
    /// attributes, matching the pinned reference's file loading entry point.
    pub(crate) fn for_resource(
        &self,
        sources: &crate::SourceMap,
        provider: &dyn crate::ResourceProvider,
    ) -> Self {
        let mut options = self.clone();
        if let Some(path) = sources
            .get(self.source_id)
            .and_then(|source| provider.document_path(source.name()))
        {
            if let Some((directory, basename)) = path.rsplit_once('/') {
                let dot = basename.rfind('.');
                let suffix = dot.map_or("", |i| &basename[i..]);
                // Helpers.extname treats a dotfile as an extension, whereas
                // File.basename refuses to strip the entire filename.
                let name = dot.filter(|&i| i != 0).map_or(basename, |i| &basename[..i]);
                // Keep the separator for both POSIX and Windows drive roots.
                let directory = if directory.is_empty()
                    || (directory.len() == 2
                        && directory.as_bytes()[0].is_ascii_alphabetic()
                        && directory.ends_with(':'))
                {
                    &path[..directory.len() + 1]
                } else {
                    directory
                };
                options.attributes.set("docdir", directory);
                options.attributes.set("docname", name);
                options.attributes.set("docfilesuffix", suffix);
                options.attributes.set("docfile", path);
            }
        }
        options
    }
}

pub(crate) struct ParseContext {
    pub attributes: Arc<Attributes>,
    /// Quote mode is cached at document creation/header completion in Ruby;
    /// entries in the header and later body mutations do not update it eagerly.
    pub quote_compat: bool,
    /// Reader range of the last accepted doctitle entry; None for host values.
    pub doctitle_span: Option<crate::Span>,
    locked: BTreeSet<String>,
    pub comments: Vec<crate::Span>,
    pub comment_diagnostics: Vec<crate::Diagnostic>,
}

impl ParseContext {
    pub fn new(options: &ParseOptions) -> Self {
        let mut attributes = Attributes::new();
        attributes.set("example-caption", "Example");
        let mut locked = BTreeSet::new();
        for (key, value) in options.attributes.iter() {
            attributes.set_parts(
                key,
                options
                    .attributes
                    .raw_parts(key)
                    .map_or_else(|| vec![AttributePart::Raw(value.to_owned())], <[_]>::to_vec),
            );
            locked.insert(key.to_string());
        }
        Self {
            quote_compat: options.attributes.get("compat-mode").is_some(),
            attributes: Arc::new(attributes),
            doctitle_span: None,
            locked,
            comments: Vec::new(),
            comment_diagnostics: Vec::new(),
        }
    }

    pub fn apply(&mut self, entry: AttributeEntry) {
        let (key, value) = match entry {
            AttributeEntry::Set(key, value) => (key, Some(value)),
            AttributeEntry::Unset(key) => (key, None),
        };
        if self.locked.contains(&key) {
            return;
        }
        let value = value.map(|v| {
            let parts = attribute_entry_parts(&v, &self.attributes, self.quote_compat);
            let value = parts_text(&parts);
            if key == "leveloffset" {
                let current = self
                    .attributes
                    .get("leveloffset")
                    .map_or(0, crate::include_selection::integer_prefix);
                if let Some(relative) = value.strip_prefix('+') {
                    return vec![AttributePart::Text(
                        current
                            .saturating_add(crate::include_selection::integer_prefix(relative))
                            .to_string(),
                    )];
                }
                if let Some(relative) = value.strip_prefix('-') {
                    return vec![AttributePart::Text(
                        current
                            .saturating_sub(crate::include_selection::integer_prefix(relative))
                            .to_string(),
                    )];
                }
            }
            parts
        });
        let attributes = Arc::make_mut(&mut self.attributes);
        match value {
            Some(value) => attributes.set_parts(key, value),
            None => {
                attributes.remove(&key);
            }
        }
    }

    pub fn section_level(&self, level: u8) -> i128 {
        self.attributes
            .get("leveloffset")
            .map_or(0, crate::include_selection::integer_prefix)
            .saturating_add(i128::from(level))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum AttributeEntry {
    Set(String, String),
    Unset(String),
}

/// Consume an entry at a parser-approved block/header boundary. Preprocessing
/// sees the old context until the entire value has been collected, as in the
/// pinned Parser.process_attribute_entry. Record each physical reader line so
/// highlighting does not have to guess continuation or container boundaries.
pub(crate) fn consume_attribute_entry(
    lines: &mut crate::reader::Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
) -> bool {
    let Some(mut entry) = parse_attribute_entry(&lines[*i].1) else {
        return false;
    };
    let first = lines.attribute_entries.len();
    let mut value = lines[*i].1[1..]
        .split_once(':')
        .unwrap()
        .1
        .trim_start_matches([' ', '\t'])
        .to_owned();
    let continuation = [" \\", " +"].into_iter().find(|con| value.ends_with(con));
    loop {
        let (off, line) = lines.line(*i);
        lines
            .attribute_entries
            .push((crate::Span::new(off, off + line.len()), None));
        *i += 1;
        let Some(con) = continuation else { break };
        value.truncate(value.len() - con.len());
        value.truncate(value.trim_end_matches(ruby_space).len());
        if !lines.has(*i, context) {
            break;
        }
        if lines[*i].1.is_empty() {
            // Ruby advances past the blank line that terminated continuation.
            *i += 1;
            break;
        }
        let next = lines[*i].1.trim_start_matches(ruby_space);
        value.push(if value.ends_with(" +") { '\n' } else { ' ' });
        value.push_str(next);
        if !next.ends_with(con) {
            let (off, line) = lines.line(*i);
            lines
                .attribute_entries
                .push((crate::Span::new(off, off + line.len()), None));
            *i += 1;
            break;
        }
    }
    if let AttributeEntry::Set(_, ref mut current) = entry {
        *current = value;
    }
    lines.attribute_entries[first].1 = Some(entry.clone());
    if matches!(&entry, AttributeEntry::Set(key, _) | AttributeEntry::Unset(key) if key == "doctitle")
        && !context.locked.contains("doctitle")
    {
        context.doctitle_span = Some(crate::Span::new(
            lines.attribute_entries[first].0.start,
            lines.attribute_entries.last().unwrap().0.end,
        ));
    }
    context.apply(entry);
    true
}

fn ruby_space(c: char) -> bool {
    matches!(c, '\0' | '\t'..='\r' | ' ')
}

pub(crate) fn parse_attribute_entry(line: &str) -> Option<AttributeEntry> {
    let rest = line.trim_end().strip_prefix(':')?;
    let (key, value) = rest.split_once(':')?;
    if !value.is_empty() && !value.starts_with([' ', '\t']) {
        return None;
    }
    let (key, unset) = if let Some(key) = key.strip_suffix('!') {
        (key, true)
    } else if let Some(key) = key.strip_prefix('!') {
        (key, true)
    } else {
        (key, false)
    };
    let first = key.chars().next()?;
    if !first.is_alphanumeric() && first != '_' {
        return None;
    }
    // Ruby Parser.sanitize_attribute_name removes punctuation and spaces.
    let key: String = key
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
        .collect();
    let key = key.to_lowercase();
    Some(if unset {
        AttributeEntry::Unset(key)
    } else {
        AttributeEntry::Set(key, value.trim().to_owned())
    })
}

/// Header attribute values substitute attribute references, not inline markup.
/// Substituted strings are not rescanned, which also bounds self references.
pub(crate) fn substitute_attribute_value(value: &str, attributes: &Attributes) -> String {
    parts_text(&substitute_attribute_parts(value, attributes, false))
}

/// A whole-value pass macro can select literal protection and attribute
/// substitution, quotes and replacements independently. Macros remain follow-up work.
fn attribute_entry_parts(value: &str, attributes: &Attributes, compat: bool) -> Vec<AttributePart> {
    if let Some((subs, body)) = value
        .strip_prefix("pass:")
        .and_then(|v| v.split_once('['))
        .and_then(|(subs, body)| body.strip_suffix(']').map(|body| (subs, body)))
    {
        let stages: Vec<_> = if subs.is_empty() {
            Vec::new()
        } else {
            subs.split(',').collect()
        };
        if stages.iter().all(|s| {
            matches!(
                *s,
                "a" | "attributes"
                    | "c"
                    | "specialcharacters"
                    | "specialchars"
                    | "none"
                    | "r"
                    | "replacements"
                    | "q"
                    | "quotes"
            )
        }) {
            let mut parts = vec![AttributePart::Raw(body.to_owned())];
            let mut applied_specials = false;
            let mut applied_attributes = false;
            let mut applied_replacements = false;
            let mut applied_quotes = false;
            for stage in stages {
                if matches!(stage, "c" | "specialcharacters" | "specialchars") {
                    if applied_specials {
                        continue;
                    }
                    applied_specials = true;
                    // Protect the entire current value, including prior expansions.
                    let text = crate::attributes::reference_text(&parts);
                    parts = vec![AttributePart::Text(text)];
                } else if matches!(stage, "a" | "attributes") {
                    if applied_attributes {
                        continue;
                    }
                    applied_attributes = true;
                    let mut expanded = Vec::new();
                    for part in parts {
                        if matches!(part, AttributePart::Open(_) | AttributePart::Close(_)) {
                            expanded.push(part);
                            continue;
                        }
                        for new in
                            substitute_attribute_parts(part.text(), attributes, part.is_raw())
                        {
                            push_value(&mut expanded, new);
                        }
                    }
                    parts = expanded;
                } else if matches!(stage, "r" | "replacements") && !applied_replacements {
                    applied_replacements = true;
                    parts = crate::replacements::apply_parts(parts);
                } else if matches!(stage, "q" | "quotes") && !applied_quotes {
                    applied_quotes = true;
                    parts = crate::attribute_quotes::apply(parts, compat);
                }
            }
            return parts;
        }
    }
    substitute_attribute_parts(value, attributes, false)
}

pub(crate) fn substitute_attribute_parts(
    value: &str,
    attributes: &Attributes,
    raw: bool,
) -> Vec<AttributePart> {
    let mut result = Vec::new();
    let mut rest = value;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open + 1..].find('}').map(|i| open + 1 + i) else {
            break;
        };
        if rest[..open].ends_with('\\') {
            push_part(&mut result, &rest[..open - 1], raw);
            push_part(&mut result, &rest[open..=close], raw);
        } else {
            push_part(&mut result, &rest[..open], raw);
            let name = &rest[open + 1..close];
            if let Some(parts) = attributes.value_parts(name) {
                for part in parts {
                    push_value(&mut result, part);
                }
            } else {
                push_part(&mut result, &rest[open..=close], raw);
            }
        }
        rest = &rest[close + 1..];
    }
    push_part(&mut result, rest, raw);
    result
}

/// Bounds for recursive resource expansion (the primary source is excluded).
#[derive(Debug, Clone, Copy)]
pub struct IncludeLimits {
    pub max_depth: usize,
    /// Loaded snapshots plus indentation/tab expansion growth, counted per include.
    pub max_bytes: usize,
}

impl Default for IncludeLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_bytes: 16 * 1024 * 1024,
        }
    }
}

pub(crate) fn intrinsic(name: &str) -> Option<&'static str> {
    Some(match name {
        "startsb" => "[",
        "endsb" => "]",
        "vbar" => "|",
        "caret" => "^",
        "asterisk" => "*",
        "tilde" => "~",
        "plus" => "&#43;",
        "backslash" => "\\",
        "backtick" => "`",
        "blank" | "empty" => "",
        "sp" => " ",
        "two-colons" => "::",
        "two-semicolons" => ";;",
        "nbsp" => "&#160;",
        "deg" => "&#176;",
        "zwsp" => "&#8203;",
        "quot" => "&#34;",
        "apos" => "&#39;",
        "lsquo" => "&#8216;",
        "rsquo" => "&#8217;",
        "ldquo" => "&#8220;",
        "rdquo" => "&#8221;",
        "wj" => "&#8288;",
        "brvbar" => "&#166;",
        "pp" => "&#43;&#43;",
        "cpp" => "C&#43;&#43;",
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        _ => return None,
    })
}
