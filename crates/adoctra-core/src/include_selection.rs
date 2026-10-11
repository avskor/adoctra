//! Include selection runs on physical lines before conditional preprocessing.
//! Keep the pinned reader's precedence and ordered tag filters, without expanding
//! potentially enormous line ranges or rewriting the immutable source snapshot.
use crate::{Diagnostic, Severity, SourceId, Span};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) enum Selection {
    All,
    Lines {
        ranges: Vec<(i128, i128)>,
        tail: bool,
    },
    Tags(Vec<(String, bool)>),
}

pub(crate) struct Selected {
    pub keep: Vec<bool>,
    pub diagnostics: Vec<Diagnostic>,
    pub missing: Vec<String>,
}

impl Selection {
    pub fn parse(attrs: &BTreeMap<String, String>) -> Self {
        if let Some(value) = attrs.get("lines") {
            let mut ranges = Vec::new();
            let mut tail = false;
            for part in split_value(value) {
                if let Some((from, to)) = part.split_once("..") {
                    let from = integer_prefix(from);
                    let end = integer_prefix(to);
                    if to.is_empty() || end < 0 {
                        ranges.push((from, from));
                        tail = true;
                    } else if from <= end {
                        ranges.push((from, end));
                    }
                } else {
                    let number = integer_prefix(part);
                    ranges.push((number, number));
                }
            }
            if ranges.is_empty() {
                return Self::All;
            }
            ranges.sort_unstable();
            let mut merged: Vec<(i128, i128)> = Vec::new();
            for (start, end) in ranges {
                if let Some(last) = merged
                    .last_mut()
                    .filter(|last| start <= last.1.saturating_add(1))
                {
                    last.1 = last.1.max(end);
                } else {
                    merged.push((start, end));
                }
            }
            return Self::Lines {
                ranges: merged,
                tail,
            };
        }
        let definitions = if let Some(value) = attrs.get("tag") {
            vec![value.as_str()]
        } else if let Some(value) = attrs.get("tags") {
            split_value(value)
        } else {
            return Self::All;
        };
        let mut tags: Vec<(String, bool)> = Vec::new();
        for definition in definitions {
            if definition.is_empty() || definition == "!" {
                continue;
            }
            let (name, selected) = definition
                .strip_prefix('!')
                .map_or((definition, true), |s| (s, false));
            if let Some(entry) = tags.iter_mut().find(|(key, _)| key == name) {
                entry.1 = selected;
            } else {
                tags.push((name.into(), selected));
            }
        }
        if tags.is_empty() {
            Self::All
        } else {
            Self::Tags(tags)
        }
    }

    pub fn select<'a>(
        self,
        lines: impl Iterator<Item = (usize, &'a str)>,
        source_id: SourceId,
    ) -> Selected {
        let mut result = Selected {
            keep: Vec::new(),
            diagnostics: Vec::new(),
            missing: Vec::new(),
        };
        match self {
            Self::All => result.keep.extend(lines.map(|_| true)),
            Self::Lines { ranges, tail } => {
                let mut current = 0;
                // Ruby's sorted queue never advances past an invalid first number.
                let blocked = ranges[0].0 < 1;
                for (index, _) in lines.enumerate() {
                    let number = index as i128 + 1;
                    while current < ranges.len() && number > ranges[current].1 {
                        current += 1;
                    }
                    result.keep.push(
                        !blocked
                            && if current == ranges.len() {
                                tail
                            } else {
                                number >= ranges[current].0
                            },
                    );
                }
            }
            Self::Tags(mut tags) => {
                let take = |tags: &mut Vec<(String, bool)>, key: &str| {
                    tags.iter()
                        .position(|(name, _)| name == key)
                        .map(|index| tags.remove(index).1)
                };
                let (base, wildcard) = if let Some(base) = take(&mut tags, "**") {
                    let wildcard = take(&mut tags, "*").or_else(|| {
                        (!base && tags.first().is_some_and(|(_, v)| !v)).then_some(true)
                    });
                    (base, wildcard)
                } else if let Some(index) = tags.iter().position(|(name, _)| name == "*") {
                    let wildcard = tags.remove(index).1;
                    (index == 0 && !wildcard, Some(wildcard))
                } else {
                    (!tags.iter().any(|(_, v)| *v), None)
                };
                let tags: BTreeMap<_, _> = tags.into_iter().collect();
                let mut found = BTreeSet::new();
                let mut stack: Vec<(String, bool, Span)> = Vec::new();
                let mut selected = base;
                for (off, line) in lines {
                    let span = Span::in_source(source_id, off, off + line.len());
                    if let Some((end, name)) = tag_directive(line) {
                        result.keep.push(false);
                        if end {
                            if stack.last().is_some_and(|(key, _, _)| key == name) {
                                stack.pop();
                                selected = stack.last().map_or(base, |(_, selected, _)| *selected);
                            } else if tags.contains_key(name) {
                                let message = if let Some(index) =
                                    stack.iter().rposition(|(key, _, _)| key == name)
                                {
                                    let expected = stack.last().unwrap().0.clone();
                                    stack.remove(index);
                                    format!(
                                        "mismatched end tag: expected '{expected}', found '{name}'"
                                    )
                                } else {
                                    format!("unexpected end tag '{name}'")
                                };
                                result.diagnostics.push(warning(span, message));
                            }
                        } else if let Some(value) = tags.get(name) {
                            selected = *value;
                            if selected {
                                found.insert(name.to_owned());
                            }
                            stack.push((name.into(), selected, span));
                        } else if let Some(wildcard) = wildcard {
                            selected = (stack.is_empty() || selected) && wildcard;
                            stack.push((name.into(), selected, span));
                        }
                    } else {
                        result.keep.push(selected);
                    }
                }
                for (name, _, span) in stack {
                    result
                        .diagnostics
                        .push(warning(span, format!("unclosed tag '{name}'")));
                }
                result
                    .missing
                    .extend(tags.into_iter().filter_map(|(name, selected)| {
                        (selected && !found.contains(&name)).then_some(name)
                    }));
            }
        }
        result
    }
}

fn warning(span: Span, message: String) -> Diagnostic {
    Diagnostic {
        span,
        severity: Severity::Warning,
        message,
    }
}

// As in the pinned reader, comma wins over semicolon; trailing empty fields
// disappear, but interior empty fields still matter for numeric selection.
fn split_value(value: &str) -> Vec<&str> {
    let separator = if value.contains(',') { ',' } else { ';' };
    let value = value.trim_end_matches(separator);
    if value.is_empty() {
        Vec::new()
    } else {
        value.split(separator).collect()
    }
}

pub(crate) fn integer_prefix(value: &str) -> i128 {
    let value = value.trim_start();
    let negative = value.starts_with('-');
    let value = value.strip_prefix(['-', '+']).unwrap_or(value);
    let mut number = 0i128;
    let mut digit_seen = false;
    for (index, byte) in value.bytes().enumerate() {
        if byte.is_ascii_digit() {
            number = number
                .saturating_mul(10)
                .saturating_add((byte - b'0') as i128);
            digit_seen = true;
        } else if byte == b'_'
            && digit_seen
            && value
                .as_bytes()
                .get(index + 1)
                .is_some_and(u8::is_ascii_digit)
        {
            digit_seen = false;
        } else {
            break;
        }
    }
    if negative {
        -number
    } else {
        number
    }
}

/// Equivalent to the pinned tag marker grammar: a word boundary on the left,
/// a nonempty non-space name, and EOF or a literal space/CR after `[]`.
fn tag_directive(line: &str) -> Option<(bool, &str)> {
    for (offset, _) in line.match_indices("::") {
        let prefix = &line[..offset];
        let (end, start) = if prefix.ends_with("tag") {
            (false, offset - 3)
        } else if prefix.ends_with("end") {
            (true, offset - 3)
        } else {
            continue;
        };
        if line[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let name = &line[offset + 2..];
        for (close, _) in name.match_indices("[]") {
            if close == 0 || name[..close].chars().any(char::is_whitespace) {
                break;
            }
            if name
                .as_bytes()
                .get(close + 2)
                .map_or(true, |c| matches!(c, b' ' | b'\r'))
            {
                return Some((end, &name[..close]));
            }
        }
    }
    None
}
