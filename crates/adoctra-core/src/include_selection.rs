//! Include selection runs on physical lines before conditional preprocessing.
//! Keep the pinned reader's precedence and ordered tag filters, without expanding
//! potentially enormous line ranges or rewriting the immutable source snapshot.
use crate::ruby_strip::RubyStrip;
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
    ) -> crate::ResourceResult<(crate::resource::IncludeSnapshot, Option<Selected>)> {
        if matches!(self, Self::All)
            && (resource.bytes.starts_with(&[0xff, 0xfe])
                || resource.bytes.starts_with(&[0xfe, 0xff]))
        {
            return resource
                .decode_include(binary.then_some("ASCII-8BIT"), None)
                .map(|decoded| (decoded.into(), None));
        }
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
        let raw_lines: Vec<_> = resource
            .bytes
            .split_inclusive(|&byte| byte == b'\n')
            .take(limit)
            .collect();
        for (index, &raw) in raw_lines.iter().enumerate() {
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
                    // UTF-8 regexp candidates validate the entire line. Binary
                    // markers are scanned from the original omitted bytes below.
                    if !binary {
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
        let selected = self.clone().select(&lines, &omissions, SourceId::default());
        let first = selected
            .keep
            .iter()
            .position(|&keep| keep)
            .and_then(|index| raw_lines.get(index));
        let endian = first.and_then(|line| {
            if line.starts_with(&[0xff, 0xfe]) {
                Some(true)
            } else if line.starts_with(&[0xfe, 0xff]) {
                Some(false)
            } else {
                None
            }
        });
        if let Some(little) = endian {
            return self
                .recover_utf16_array(&resource, &raw_lines, &lines, &omissions, selected, little);
        }
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
        Ok((
            crate::resource::IncludeSnapshot {
                decoded: crate::IncludeResource {
                    resource: crate::Resource {
                        name: resource.name,
                        text,
                    },
                    complete: false,
                },
                omissions,
                frames: None,
            },
            None,
        ))
    }

    /// Select an unconverted (OpenURI) resource like the pinned reader: lines
    /// split in the external encoding, tag markers scanned in it, and only
    /// the selected lines converted to UTF-8 by `prepare_source_array` (whose
    /// UTF-16 or UTF-8 BOM check applies to the first selected line). A whole
    /// read is converted at once by `prepare_source_string`. Unselected lines
    /// are kept decoded where possible, otherwise as explicit omissions.
    pub(crate) fn recover_unconverted(
        &self,
        resource: crate::BinaryResource,
        encoding: Option<&str>,
    ) -> crate::ResourceResult<(crate::resource::IncludeSnapshot, Option<Selected>)> {
        const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];
        let codec = crate::source_encoding::raw_codec(encoding);
        if let Some(binary) = codec.deferred {
            return self.recover_bytes(resource, binary);
        }
        let external = encoding.expect("a non-default external encoding");
        let invalid = || {
            crate::ResourceError::new(
                crate::ResourceErrorKind::InvalidEncoding,
                &resource.name,
                format!("invalid source encoding: {external}"),
            )
        };
        let decode = |bytes: &[u8], encoding: &str| {
            crate::BinaryResource {
                name: resource.name.clone(),
                bytes: bytes.to_vec(),
                content_type: None,
            }
            .decode(Some(encoding))
            .ok()
            .map(|decoded| decoded.text)
        };
        // A UTF-16 BOM wins over the external encoding; a UTF-8 BOM is
        // removed before converting from it. Only this BOM is removed: the
        // frames tell the reader not to strip a following U+FEFF.
        let normalize = |raw: &[u8]| {
            if let Some(rest) = raw.strip_prefix(&[0xff, 0xfe]) {
                decode(rest, "UTF-16LE")
            } else if let Some(rest) = raw.strip_prefix(&[0xfe, 0xff]) {
                decode(rest, "UTF-16BE")
            } else if let Some(rest) = raw.strip_prefix(UTF8_BOM) {
                decode(rest, external)
            } else {
                decode(raw, external)
            }
        };
        if matches!(self, Self::All) {
            let text = normalize(&resource.bytes).ok_or_else(invalid)?;
            let mut ranges = Vec::new();
            let mut start = 0;
            for (index, _) in text.match_indices('\n') {
                ranges.push((start, index + 1));
                start = index + 1;
            }
            if start < text.len() {
                ranges.push((start, text.len()));
            }
            let frames = crate::resource::SourceLineFrames {
                ranges,
                trailing_lf: text.ends_with('\n'),
                normalized: true,
            };
            return Ok((
                crate::resource::IncludeSnapshot {
                    decoded: crate::IncludeResource {
                        resource: crate::Resource {
                            name: resource.name,
                            text,
                        },
                        complete: true,
                    },
                    omissions: Vec::new(),
                    frames: Some(frames),
                },
                None,
            ));
        }
        // OpenURI buffers up to 10240 bytes in a StringIO, whose `each_line`
        // finds a multi-byte separator at any byte offset; a larger response
        // is a Tempfile, whose IO matches whole code units only.
        let separator = codec.separator;
        let aligned = resource.bytes.len() > 10240;
        let mut raw_lines = Vec::new();
        let (mut start, mut index) = (0, 0);
        let limit = self.line_limit().unwrap_or(usize::MAX);
        while index + separator.len() <= resource.bytes.len() && raw_lines.len() < limit {
            if resource.bytes[index..].starts_with(separator) {
                index += separator.len();
                raw_lines.push(&resource.bytes[start..index]);
                start = index;
            } else {
                index += if aligned { separator.len() } else { 1 };
            }
        }
        if start < resource.bytes.len() && raw_lines.len() < limit {
            raw_lines.push(&resource.bytes[start..]);
        }
        let tags = matches!(self, Self::Tags(_));
        if tags && !codec.ascii_compatible && !raw_lines.is_empty() {
            // `l.include? '::'` raises Encoding::CompatibilityError.
            return Err(invalid());
        }
        // Each line is converted on its own, as `prepare_source_array` does.
        let decoded: Vec<_> = raw_lines.iter().map(|raw| decode(raw, external)).collect();
        let layout = |texts: &dyn Fn(usize) -> Option<String>| {
            let mut text = String::new();
            let mut omissions = Vec::new();
            let mut ranges = Vec::new();
            let mut offsets = Vec::new();
            let mut trailing_lf = false;
            for (index, raw) in raw_lines.iter().enumerate() {
                offsets.push(text.len());
                if let Some(line) = texts(index) {
                    let start = text.len();
                    text.push_str(&line);
                    ranges.push((start, text.len()));
                    trailing_lf = raw.ends_with(separator);
                } else {
                    omissions.push(crate::OmittedSourceLine {
                        offset: text.len(),
                        line: index + 1,
                        bytes: raw.to_vec(),
                    });
                }
            }
            let frames = crate::resource::SourceLineFrames {
                ranges,
                trailing_lf,
                normalized: true,
            };
            (text, omissions, frames, offsets)
        };
        let (text, omissions, frames, raw_offsets) = layout(&|index| decoded[index].clone());
        for line in &omissions {
            // The marker regexp raises on a malformed multibyte line.
            if tags
                && !codec.single_byte
                && line.bytes.windows(2).any(|s| s == b"::")
                && line.bytes.windows(2).any(|s| s == b"[]")
            {
                return Err(invalid());
            }
        }
        let views = crate::resource::framed_snapshot_lines(&text, &omissions, Some(&frames));
        let selected = self
            .clone()
            .select_names(&views, &omissions, SourceId::default(), true);
        let first = selected.keep.iter().position(|&keep| keep);
        let utf16 = first.and_then(|index| match raw_lines[index].get(..2) {
            Some([0xff, 0xfe]) => Some("UTF-16LE"),
            Some([0xfe, 0xff]) => Some("UTF-16BE"),
            _ => None,
        });
        let mut converted = Vec::with_capacity(raw_lines.len());
        for (index, raw) in raw_lines.iter().enumerate() {
            converted.push(if !selected.keep[index] {
                decoded[index].clone()
            } else if utf16.is_some() && Some(index) == first {
                Some(normalize(raw).ok_or_else(invalid)?)
            } else if let Some(encoding) = utf16 {
                Some(decode(raw, encoding).ok_or_else(invalid)?)
            } else if Some(index) == first {
                Some(normalize(raw).ok_or_else(invalid)?)
            } else {
                Some(decoded[index].clone().ok_or_else(invalid)?)
            });
        }
        let (text, omissions, frames, offsets) = layout(&|index| converted[index].clone());
        // Remap the raw selection's diagnostics into the registered layout;
        // converted lines must never run through the tag filter again.
        let mut views = views;
        for (index, line) in views.iter_mut().enumerate() {
            let shift = line.original.expect("physical line") - raw_offsets[index];
            line.original = Some(offsets[index] + shift);
        }
        let selected = self
            .clone()
            .select_names(&views, &omissions, SourceId::default(), true);
        Ok((
            crate::resource::IncludeSnapshot {
                decoded: crate::IncludeResource {
                    resource: crate::Resource {
                        name: resource.name.clone(),
                        text,
                    },
                    complete: false,
                },
                omissions,
                frames: Some(frames),
            },
            Some(selected),
        ))
    }

    /// The reader's array normalization uses the BOM of its first kept raw
    /// line. Tags have already been filtered; decoded marker-like text must
    /// never run through that filter a second time.
    fn recover_utf16_array(
        &self,
        resource: &crate::BinaryResource,
        raw_lines: &[&[u8]],
        raw_views: &[crate::include_indent::Line],
        raw_omissions: &[crate::OmittedSourceLine],
        selected: Selected,
        little: bool,
    ) -> crate::ResourceResult<(crate::resource::IncludeSnapshot, Option<Selected>)> {
        let mut text = String::new();
        let mut omissions = Vec::new();
        let mut ranges = Vec::new();
        let mut offsets = Vec::new();
        let mut trailing_lf = false;
        let mut raw_omitted = raw_omissions.iter().peekable();
        for (index, &raw) in raw_lines.iter().enumerate() {
            let start = text.len();
            offsets.push(start);
            let omitted = raw_omitted
                .peek()
                .is_some_and(|line| line.line == index + 1);
            if omitted {
                raw_omitted.next();
            }
            if selected.keep[index] {
                let decoded = crate::BinaryResource {
                    name: resource.name.clone(),
                    bytes: raw.to_vec(),
                    content_type: None,
                }
                .decode(Some(if little { "UTF-16LE" } else { "UTF-16BE" }))?;
                text.push_str(&decoded.text);
            } else if omitted {
                omissions.push(crate::OmittedSourceLine {
                    offset: start,
                    line: index + 1,
                    bytes: raw.to_vec(),
                });
                continue;
            } else {
                text.push_str(std::str::from_utf8(raw).expect("validated raw line"));
            }
            ranges.push((start, text.len()));
            trailing_lf = raw.ends_with(b"\n");
        }
        // Remap raw tag diagnostics into the registered layout. Marker lines
        // stay raw (or omitted); only kept body lines were transcoded.
        let mut views = raw_views.to_vec();
        let strip_bom = resource.bytes.starts_with(&[0xef, 0xbb, 0xbf])
            && raw_omissions.first().is_none_or(|line| line.line != 1);
        for (index, line) in views.iter_mut().enumerate() {
            let bom = usize::from(index == 0 && strip_bom) * 3;
            line.original = Some(offsets[index] + bom);
        }
        let diagnostic_omissions: Vec<_> = raw_omissions
            .iter()
            .map(|line| crate::OmittedSourceLine {
                offset: offsets[line.line - 1],
                line: line.line,
                bytes: line.bytes.clone(),
            })
            .collect();
        let selected = self
            .clone()
            .select(&views, &diagnostic_omissions, SourceId::default());
        // Reuse a whole Unicode snapshot only when no bytes were skipped and
        // physical IO boundaries agree with its actual Unicode LF characters.
        let complete = selected.keep.iter().all(|&keep| keep)
            && raw_lines.iter().map(|line| line.len()).sum::<usize>() == resource.bytes.len()
            && trailing_lf == text.ends_with('\n')
            && ranges
                .iter()
                .all(|&(_, end)| end == text.len() || text[..end].ends_with('\n'));
        let frames = (!complete).then_some(crate::resource::SourceLineFrames {
            ranges,
            trailing_lf,
            normalized: false,
        });
        Ok((
            crate::resource::IncludeSnapshot {
                decoded: crate::IncludeResource {
                    resource: crate::Resource {
                        name: resource.name.clone(),
                        text,
                    },
                    complete,
                },
                omissions,
                frames,
            },
            Some(selected),
        ))
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

    pub fn select(
        self,
        lines: &[crate::include_indent::Line],
        omissions: &[crate::OmittedSourceLine],
        source_id: SourceId,
    ) -> Selected {
        self.select_names(lines, omissions, source_id, false)
    }

    /// `foreign`: lines are in another Ruby encoding than UTF-8, whose
    /// non-ASCII tag names never equal a UTF-8 attribute name.
    fn select_names(
        self,
        lines: &[crate::include_indent::Line],
        omissions: &[crate::OmittedSourceLine],
        source_id: SourceId,
        foreign: bool,
    ) -> Selected {
        let mut omitted = omissions.iter().peekable();
        let lines = lines.iter().enumerate().map(|(index, line)| {
            let raw = if omitted.peek().is_some_and(|raw| raw.line == index + 1) {
                omitted.next()
            } else {
                None
            };
            (
                line.original.expect("physical selection line"),
                line.text.as_ref(),
                raw,
            )
        });
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
                let mut stack: Vec<(TagName, bool, Span, usize, Option<crate::OmittedSourceSpan>)> =
                    Vec::new();
                let mut selected = base;
                for (index, (off, line, raw)) in lines.enumerate() {
                    let span = Span::in_source(source_id, off, off + line.len());
                    let directive = if let Some(raw) = raw {
                        binary_tag_directive(&raw.bytes).map(|(end, name, start, end_offset)| {
                            (
                                end,
                                TagName::new(name, true),
                                Some(crate::OmittedSourceSpan {
                                    line: raw.line,
                                    start,
                                    end: end_offset,
                                }),
                            )
                        })
                    } else {
                        tag_directive(line)
                            .map(|(end, name)| (end, TagName::new(name.as_bytes(), foreign), None))
                    };
                    if let Some((end, name, omitted_span)) = directive {
                        result.keep.push(false);
                        if end {
                            if stack.last().is_some_and(|(key, _, _, _, _)| key == &name) {
                                stack.pop();
                                selected = stack
                                    .last()
                                    .map_or(base, |(_, selected, _, _, _)| *selected);
                            } else if name.text().is_some_and(|name| tags.contains_key(name)) {
                                let (message, report) = if let Some(stack_index) =
                                    stack.iter().rposition(|(key, _, _, _, _)| key == &name)
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
                                diagnostic.omitted_span = omitted_span;
                                diagnostic.report.message = Some(report);
                                result.diagnostics.push(diagnostic);
                            }
                        } else if let Some(value) = name.text().and_then(|name| tags.get(name)) {
                            selected = *value;
                            if selected {
                                found.insert(name.text().expect("matched text tag").to_owned());
                            }
                            stack.push((name, selected, span, index + 1, omitted_span));
                        } else if let Some(wildcard) = wildcard {
                            selected = (stack.is_empty() || selected) && wildcard;
                            stack.push((name, selected, span, index + 1, omitted_span));
                        }
                    } else {
                        result.keep.push(selected);
                    }
                }
                for (name, _, span, line, omitted_span) in stack {
                    let mut diagnostic = warning(span, format!("unclosed tag '{name}'"));
                    diagnostic.omitted_span = omitted_span;
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
        omitted_span: None,
        report: Default::default(),
        span,
        severity: Severity::Warning,
        message,
    }
}

/// Ruby does not equate a non-ASCII BINARY name with a UTF-8 attribute name.
/// Keep raw names distinct even if their bytes happen to be valid UTF-8.
#[derive(Clone, PartialEq, Eq)]
struct TagName {
    bytes: Vec<u8>,
    binary: bool,
}

impl TagName {
    fn new(bytes: &[u8], binary: bool) -> Self {
        Self {
            bytes: bytes.to_vec(),
            binary: binary && !bytes.is_ascii(),
        }
    }

    fn text(&self) -> Option<&str> {
        (!self.binary).then(|| std::str::from_utf8(&self.bytes).expect("text tag name"))
    }
}

impl std::fmt::Display for TagName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Ok(text) = std::str::from_utf8(&self.bytes) {
            return f.write_str(text);
        }
        // Ruby may raise when a logger serializes an invalid BINARY name.
        // Preserve error tolerance with an explicitly labelled lossless form.
        f.write_str("<binary:")?;
        for byte in &self.bytes {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(">")
    }
}

/// The BINARY regexp uses ASCII word/space classes and matches bytes, not
/// decoded Unicode. Return the exact marker range within the original line.
fn binary_tag_directive(raw: &[u8]) -> Option<(bool, &[u8], usize, usize)> {
    let line = raw.strip_suffix(b"\n").unwrap_or(raw);
    for (offset, pair) in line.windows(2).enumerate() {
        if pair != b"::" || offset < 3 {
            continue;
        }
        let start = offset - 3;
        let end = match &line[start..offset] {
            b"tag" => false,
            b"end" => true,
            _ => continue,
        };
        if start > 0 && (line[start - 1].is_ascii_alphanumeric() || line[start - 1] == b'_') {
            continue;
        }
        let name_start = offset + 2;
        for close in name_start..line.len().saturating_sub(1) {
            if matches!(line[close], b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c) {
                break;
            }
            if close > name_start
                && &line[close..close + 2] == b"[]"
                && line
                    .get(close + 2)
                    .is_none_or(|byte| matches!(byte, b' ' | b'\r'))
            {
                return Some((end, &line[name_start..close], start, close + 2));
            }
        }
    }
    None
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
    let value = value.ruby_lstrip();
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
                .is_none_or(|c| matches!(c, b' ' | b'\r'))
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

    fn unconverted(
        bytes: &[u8],
        encoding: Option<&str>,
        attrs: &[(&str, &str)],
    ) -> crate::ResourceResult<(crate::resource::IncludeSnapshot, Option<Selected>)> {
        let attrs = attrs
            .iter()
            .map(|&(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
        Selection::parse(&attrs).recover_unconverted(
            crate::BinaryResource {
                name: "http://host/part.adoc".into(),
                bytes: bytes.to_vec(),
                content_type: None,
            },
            encoding,
        )
    }

    #[test]
    fn unconverted_lines_split_in_the_external_encoding_and_convert_only_selected_lines() {
        // 0x98 has no Windows-1251 mapping: unselected, it is an omission.
        let (snapshot, selected) = unconverted(
            b"*one*\n\x98\n\xcf\xf0\xe8\xe2\xe5\xf2\n",
            Some("Windows-1251"),
            &[("lines", "1;3")],
        )
        .unwrap();
        assert_eq!(snapshot.decoded.resource.text, "*one*\nПривет\n");
        assert!(!snapshot.decoded.complete);
        assert_eq!(snapshot.omissions.len(), 1);
        assert_eq!(snapshot.omissions[0].line, 2);
        assert_eq!(snapshot.omissions[0].offset, 6);
        assert_eq!(snapshot.omissions[0].bytes, b"\x98\n");
        let frames = snapshot.frames.unwrap();
        assert_eq!(frames.ranges, [(0, 6), (6, 19)]);
        assert!(frames.trailing_lf && frames.normalized);
        assert_eq!(selected.unwrap().keep, [true, false, true]);
        // Selecting it fails like Ruby's conversion of that line.
        let error = unconverted(b"*one*\n\x98\n", Some("Windows-1251"), &[("lines", "2")])
            .err()
            .unwrap();
        assert_eq!(error.kind, crate::ResourceErrorKind::InvalidEncoding);
        // IBM037 splits at byte 0x0A: its encoded LF (0x25) stays in line 1.
        let (snapshot, selected) =
            unconverted(b"\x81\x25\x82\x0a\x83", Some("IBM037"), &[("lines", "1;2")]).unwrap();
        assert_eq!(selected.unwrap().keep, [true, true]);
        assert_eq!(snapshot.decoded.resource.text, "a\nb\u{8e}c");
        assert_eq!(snapshot.frames.unwrap().ranges, [(0, 5), (5, 6)]);
        // Like Ruby's `break`, a finite selection reads no further lines.
        let (snapshot, _) =
            unconverted(b"\x81\x0a\x98\x0a", Some("IBM037"), &[("lines", "1")]).unwrap();
        assert_eq!(snapshot.decoded.resource.text, "a\u{8e}");
    }

    #[test]
    fn unconverted_utf16_splits_like_stringio_up_to_10240_bytes_and_like_io_beyond() {
        // `00 0A` at bytes 1 and 5: the StringIO's line 2 is `41 41 00 0A`.
        let text: String = [0x100, 0xa41, 0x4100, 0xa42, 0xa]
            .into_iter()
            .map(|c| char::from_u32(c).unwrap())
            .collect();
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let (snapshot, selected) =
            unconverted(&bytes, Some("UTF-16BE"), &[("lines", "2")]).unwrap();
        assert_eq!(selected.unwrap().keep, [false, true]);
        // Line 1 has an odd length: kept only as an omission.
        assert_eq!(snapshot.decoded.resource.text, "\u{4141}\n");
        assert_eq!(snapshot.omissions.len(), 1);
        assert_eq!(snapshot.omissions[0].bytes, [0x01, 0x00, 0x0a]);
        assert_eq!(snapshot.frames.unwrap().ranges, [(0, 4)]);
        assert!(unconverted(&bytes, Some("UTF-16BE"), &[("lines", "1")]).is_err());
        // A Tempfile body matches whole code units only.
        let mut large = bytes.clone();
        large.extend("x\n".repeat(2600).encode_utf16().flat_map(u16::to_be_bytes));
        let (snapshot, selected) =
            unconverted(&large, Some("UTF-16BE"), &[("lines", "1")]).unwrap();
        assert_eq!(selected.unwrap().keep, [true]);
        assert_eq!(snapshot.decoded.resource.text, text);
    }

    #[test]
    fn unconverted_tag_scans_follow_the_external_encoding() {
        let tagged = "// tag::t[]\n*in*\n// end::t[]\n";
        let utf16: Vec<u8> = tagged.encode_utf16().flat_map(u16::to_le_bytes).collect();
        // `include? '::'` raises for an ASCII-incompatible encoding.
        let error = unconverted(&utf16, Some("UTF-16LE"), &[("tag", "t")])
            .err()
            .unwrap();
        assert_eq!(error.kind, crate::ResourceErrorKind::InvalidEncoding);
        // A malformed multibyte marker line breaks the regexp; a single-byte
        // unmapped one does not.
        assert!(unconverted(b"// tag::t[] \x81\n", Some("Shift_JIS"), &[("tag", "t")]).is_err());
        let (snapshot, selected) = unconverted(
            b"// tag::t[] \x98\n*in*\n// end::t[]\n",
            Some("Windows-1251"),
            &[("tag", "t")],
        )
        .unwrap();
        let selected = selected.unwrap();
        assert_eq!(selected.keep, [false, true, false]);
        assert!(selected.missing.is_empty());
        assert_eq!(snapshot.omissions[0].line, 1);
        // A non-ASCII name in another encoding never equals the UTF-8 name.
        let cyrillic = b"// tag::\xf2\xe5\xe3[]\n*in*\n// end::\xf2\xe5\xe3[]\n";
        let (_, selected) = unconverted(cyrillic, Some("Windows-1251"), &[("tag", "тег")]).unwrap();
        let selected = selected.unwrap();
        assert_eq!(selected.keep, [false, false, false]);
        assert_eq!(selected.missing, ["тег"]);
    }

    #[test]
    fn unconverted_bom_applies_to_the_first_selected_line_and_is_removed_once() {
        let data = b"skip\n\xef\xbb\xbf\xcf\xf0\n";
        let (snapshot, _) = unconverted(data, Some("Windows-1251"), &[("lines", "2")]).unwrap();
        assert_eq!(snapshot.decoded.resource.text, "skip\nПр\n");
        // Not first: the BOM bytes are Windows-1251 text.
        let (snapshot, _) = unconverted(data, Some("Windows-1251"), &[("lines", "1;2")]).unwrap();
        assert_eq!(snapshot.decoded.resource.text, "skip\nп»їПр\n");
        // A UTF-16 BOM on the first selected line wins over the encoding.
        let mut data = b"skip\n\xff\xfe".to_vec();
        data.extend("Пр".encode_utf16().flat_map(u16::to_le_bytes));
        let (snapshot, _) = unconverted(&data, Some("Windows-1251"), &[("lines", "2")]).unwrap();
        assert_eq!(snapshot.decoded.resource.text, "skip\nПр");
        // A whole read: one BOM removed, a second U+FEFF stays content.
        let mut data = b"\xfe\xff\xfe\xff".to_vec();
        data.extend("a\nb".encode_utf16().flat_map(u16::to_be_bytes));
        let (snapshot, selected) = unconverted(&data, Some("Windows-1251"), &[]).unwrap();
        assert!(selected.is_none());
        assert!(snapshot.decoded.complete);
        assert_eq!(snapshot.decoded.resource.text, "\u{feff}a\nb");
        let frames = snapshot.frames.unwrap();
        assert_eq!(frames.ranges, [(0, 5), (5, 6)]);
        assert!(!frames.trailing_lf && frames.normalized);
        // The dummy UTF-16 converter consumes its own BOM per line.
        let (snapshot, _) = unconverted(&data, Some("UTF-16"), &[("lines", "1")]).unwrap();
        assert_eq!(snapshot.decoded.resource.text, "\u{feff}a\n");
    }

    #[test]
    fn unconverted_utf8_and_binary_keep_the_deferred_file_path() {
        let (snapshot, selected) =
            unconverted(b"one\n\xff\nthree\n", None, &[("lines", "1;3")]).unwrap();
        assert!(selected.is_none());
        assert!(snapshot.frames.is_none());
        assert_eq!(snapshot.decoded.resource.text, "one\nthree\n");
        assert_eq!(snapshot.omissions[0].bytes, b"\xff\n");
        let (snapshot, _) =
            unconverted(b"one\n\xff\nthree\n", Some("bogus"), &[("lines", "1;3")]).unwrap();
        assert_eq!(snapshot.decoded.resource.text, "one\nthree\n");
        assert!(unconverted(b"\xff\n", Some("BINARY"), &[("lines", "1")]).is_err());
    }
}
