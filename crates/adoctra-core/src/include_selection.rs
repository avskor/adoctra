//! Include selection runs on physical lines before conditional preprocessing.
//! Keep the pinned reader's precedence and ordered tag filters, without expanding
//! potentially enormous line ranges or rewriting the immutable source snapshot.
use crate::{Diagnostic, Severity, SourceId, Span};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
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
    /// Validate only selected UTF-8/BINARY lines. Retained UTF-8 text is never
    /// repaired; skipped undecodable lines are explicit immutable omissions.
    pub(crate) fn recover_bytes(
        &self,
        resource: crate::BinaryResource,
        binary: bool,
    ) -> crate::ResourceResult<crate::resource::IncludeSnapshot> {
        let invalid = || {
            crate::ResourceError::new(
                crate::ResourceErrorKind::InvalidEncoding,
                &resource.name,
                "invalid source encoding in consumed include content",
            )
        };
        let mut text = String::new();
        let mut omissions = Vec::new();
        let limit = self.line_limit().unwrap_or(usize::MAX);
        for (index, raw) in resource
            .bytes
            .split_inclusive(|&byte| byte == b'\n')
            .take(limit)
            .enumerate()
        {
            // Ruby recognizes a BOM on the first *selected* line. Keep these
            // UTF-8 candidates until selection tells us which line is first.
            let ascii = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw);
            let decoded = std::str::from_utf8(raw)
                .ok()
                .filter(|_| !binary || ascii.is_ascii());
            if let Some(line) = decoded {
                text.push_str(line);
            } else {
                if matches!(self, Self::Tags(_))
                    && raw.windows(2).any(|s| s == b"::")
                    && raw.windows(2).any(|s| s == b"[]")
                {
                    // Ruby's UTF-8 regexp validates the whole candidate line,
                    // even when it is outside the selected tag. BINARY can scan
                    // arbitrary bytes; malformed tag-marker lines still require
                    // a byte-based marker/source-position contract (open gap).
                    let view: String = raw
                        .iter()
                        .map(|&byte| {
                            if byte.is_ascii() {
                                char::from(byte)
                            } else {
                                '\u{fffd}'
                            }
                        })
                        .collect();
                    if !binary || tag_directive(&view).is_some() {
                        return Err(invalid());
                    }
                }
                omissions.push(crate::OmittedSourceLine {
                    offset: text.len(),
                    line: index + 1,
                    bytes: raw.to_vec(),
                });
            }
        }
        let lines = crate::resource::snapshot_lines(&text, &omissions);
        let selected = self.clone().select(
            lines.iter().map(|line| {
                (
                    line.original.expect("physical selection slot"),
                    line.text.as_ref(),
                )
            }),
            SourceId::default(),
        );
        if omissions.iter().any(|line| selected.keep[line.line - 1]) {
            return Err(invalid());
        }
        if binary {
            let mut kept = lines.iter().zip(&selected.keep).filter(|(_, keep)| **keep);
            kept.next(); // Its leading BOM is consumed by reader normalization.
            if kept.any(|(line, _)| !line.text.is_ascii()) {
                return Err(invalid());
            }
        }
        Ok(crate::resource::IncludeSnapshot {
            decoded: crate::IncludeResource {
                resource: crate::Resource {
                    name: resource.name,
                    text,
                },
                complete: false,
            },
            omissions,
        })
    }

    /// Ruby stops reading only after consuming its entire sorted line queue.
    /// Invalid first numbers block that queue, and open ranges read to EOF.
    pub fn line_limit(&self) -> Option<usize> {
        match self {
            Self::Lines {
                ranges,
                tail: false,
            } if ranges.first()?.0 > 0 => usize::try_from(ranges.last()?.1).ok(),
            _ => None,
        }
    }

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

    /// Whether a selection keeps the include visible to interdocument
    /// references when it selects anything: Ruby marks selected lines and
    /// tags `partial-option`, except a bare `**` (optionally with `*`).
    pub fn full(&self) -> bool {
        match self {
            Self::All => true,
            Self::Lines { .. } => false,
            Self::Tags(tags) => {
                let get = |name: &str| tags.iter().find(|(key, _)| key == name).map(|t| t.1);
                get("**") == Some(true)
                    && get("*") != Some(false)
                    && tags.iter().all(|(key, _)| key == "**" || key == "*")
            }
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
                let required: Vec<_> = tags
                    .iter()
                    .filter(|(_, selected)| *selected)
                    .map(|(name, _)| name.clone())
                    .collect();
                let tags: BTreeMap<_, _> = tags.into_iter().collect();
                let mut found = BTreeSet::new();
                let mut stack: Vec<(String, bool, Span, usize)> = Vec::new();
                let mut selected = base;
                for (index, (off, line)) in lines.enumerate() {
                    let span = Span::in_source(source_id, off, off + line.len());
                    if let Some((end, name)) = tag_directive(line) {
                        result.keep.push(false);
                        if end {
                            if stack.last().is_some_and(|(key, _, _, _)| key == name) {
                                stack.pop();
                                selected =
                                    stack.last().map_or(base, |(_, selected, _, _)| *selected);
                            } else if tags.contains_key(name) {
                                let (message, report) = if let Some(stack_index) =
                                    stack.iter().rposition(|(key, _, _, _)| key == name)
                                {
                                    let expected = stack.last().unwrap().0.clone();
                                    stack.remove(stack_index);
                                    (format!(
                                        "mismatched end tag: expected '{expected}', found '{name}'"
                                    ), format!("mismatched end tag (expected '{expected}' but found '{name}') at line {}", index + 1))
                                } else {
                                    (
                                        format!("unexpected end tag '{name}'"),
                                        format!(
                                            "unexpected end tag '{name}' at line {}",
                                            index + 1
                                        ),
                                    )
                                };
                                let mut diagnostic = warning(span, message);
                                diagnostic.report.message = Some(report);
                                result.diagnostics.push(diagnostic);
                            }
                        } else if let Some(value) = tags.get(name) {
                            selected = *value;
                            if selected {
                                found.insert(name.to_owned());
                            }
                            stack.push((name.into(), selected, span, index + 1));
                        } else if let Some(wildcard) = wildcard {
                            selected = (stack.is_empty() || selected) && wildcard;
                            stack.push((name.into(), selected, span, index + 1));
                        }
                    } else {
                        result.keep.push(selected);
                    }
                }
                for (name, _, span, line) in stack {
                    let mut diagnostic = warning(span, format!("unclosed tag '{name}'"));
                    diagnostic.report.message = Some(format!(
                        "detected unclosed tag '{name}' starting at line {line}"
                    ));
                    result.diagnostics.push(diagnostic);
                }
                result
                    .missing
                    .extend(required.into_iter().filter(|name| !found.contains(name)));
            }
        }
        result
    }
}

fn warning(span: Span, message: String) -> Diagnostic {
    Diagnostic {
        report: Default::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_bound_follows_the_readers_sorted_line_queue() {
        for (lines, expected) in [
            ("1", Some(1)),
            ("3;1;2;2", Some(3)),
            ("2..4;8..7", Some(4)),
            ("2,4", Some(4)),
            ("1;", Some(1)),
            ("2suffix", Some(2)),
            ("0;1", None),
            ("-1;2", None),
            ("no-number", None),
            ("1..", None),
            ("1..-1", None),
            ("1;4..", None),
            ("2..1", None),
            ("", None),
            ("999999999999999999999999999999", None),
        ] {
            let attrs = BTreeMap::from([
                ("lines".into(), lines.into()),
                ("tag".into(), "body".into()),
            ]);
            assert_eq!(Selection::parse(&attrs).line_limit(), expected, "{lines}");
        }
        assert_eq!(
            Selection::parse(&BTreeMap::from([("tag".into(), "body".into())])).line_limit(),
            None
        );
    }
}
