//! Block attribute lines, as the pinned Ruby parser consumes them: the
//! `BlockAttributeListRx` gate, AttributeList fields merged `into:` the
//! accumulated metadata by Ruby's integer keys, and the first-positional style
//! shorthand (`style#id.role%option`).
use crate::attribute_list::{self, Field};
use crate::diagnostic::{Diagnostic, Severity};
use crate::parser::PendingMeta;
use crate::Span;
use std::collections::BTreeMap;

/// A recognised attribute line: the text between the brackets, its offset in
/// the processed source and its AttributeList fields.
#[derive(Debug, PartialEq)]
pub(crate) struct AttrLine {
    pub inner: String,
    pub base: usize,
    pub fields: Vec<Field>,
}

/// A positional value as Ruby stores it, with the processed-source range of
/// the field that produced it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AttrValue {
    pub text: String,
    pub span: Span,
}

/// Ruby's integer-keyed positional attributes and plain named attributes.
/// `None` is an explicit nil (an empty unquoted field), which still replaces
/// a value from an earlier line.
#[derive(Debug, Default)]
pub(crate) struct BlockList {
    pub positional: BTreeMap<usize, Option<AttrValue>>,
    named: BTreeMap<String, String>,
}

impl BlockList {
    pub fn value(&self, key: usize) -> Option<&AttrValue> {
        self.positional.get(&key).and_then(Option::as_ref)
    }
}

/// `^\[(|[\p{Word}.#%{,"'].*)\]$`, excluding `[[` lines (anchors only).
pub(crate) fn attr_list_inner(line: &str) -> Option<&str> {
    let line = line.trim_end();
    if line.starts_with("[[") {
        return None;
    }
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    match inner.chars().next() {
        None => Some(inner),
        Some(c) if crate::unicode_word::is_word(c) || ".#%{,\"'".contains(c) => Some(inner),
        Some(_) => None,
    }
}

/// Recognise a whole-line block attribute list. `line_start` is the processed
/// offset of `line`.
pub(crate) fn parse_block_attr_list(line: &str, line_start: usize) -> Option<AttrLine> {
    let inner = attr_list_inner(line)?;
    Some(AttrLine {
        inner: inner.to_owned(),
        base: line_start + 1,
        fields: attribute_list::parse(inner),
    })
}

/// Ruby's `parse_attributes(…, into: attributes)` followed by the style
/// shorthand of the merged first positional.
pub(crate) fn merge(
    pending: &mut PendingMeta,
    line: AttrLine,
    line_span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let current_style = pending.list.positional.get(&1).cloned().flatten();
    for field in &line.fields {
        let text = field.text(&line.inner);
        let Some(name) = field.name(&line.inner) else {
            let range = field.value.clone().unwrap_or(0..0);
            let value = text.map(|text| AttrValue {
                text,
                span: Span::new(line.base + range.start, line.base + range.end),
            });
            pending.list.positional.insert(field.index + 1, value);
            continue;
        };
        let text = text.unwrap_or_default();
        match name {
            "options" | "opts" => {
                if text.contains(',') {
                    for option in text.replace(' ', "").split(',') {
                        add_option(pending, option);
                    }
                } else {
                    add_option(pending, &text);
                }
            }
            "id" => pending.id = Some(text),
            // The named role replaces earlier roles; shorthand roles append.
            "role" => pending.roles = text.split_whitespace().map(str::to_owned).collect(),
            _ => {
                pending.list.named.insert(name.to_owned(), text);
            }
        }
    }
    if let Some(raw) = pending.list.value(1).cloned() {
        let style = parse_style(pending, &raw.text, line_span, diagnostics);
        let style = style
            .map(|text| AttrValue {
                text,
                span: raw.span,
            })
            .or(current_style);
        pending.list.positional.insert(1, style);
    }
    sync(pending);
}

fn add_option(pending: &mut PendingMeta, option: &str) {
    if !option.is_empty() && !pending.attrs.options.iter().any(|o| o == option) {
        pending.attrs.options.push(option.to_owned());
    }
}

/// Ruby's `parse_style_attribute`. Returns the value to store as the first
/// positional; `None` lets the caller restore the previous style.
fn parse_style(
    pending: &mut PendingMeta,
    raw: &str,
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    // Spaces disable the shorthand, as does the absence of any sigil.
    if raw.contains(' ') || !raw.contains(['.', '#', '%']) {
        pending.attrs.style = Some(raw.to_owned());
        return Some(raw.to_owned());
    }
    let mut style = None;
    let mut id = None;
    let mut roles = Vec::new();
    let mut options = Vec::new();
    // Ruby's yield_buffered_attribute for the value preceding each sigil.
    let mut buffered = |sigil: Option<char>, value: &str| {
        let mut warn = |message: String| {
            diagnostics.push(Diagnostic {
                span,
                severity: Severity::Warning,
                message,
            })
        };
        match sigil {
            None if !value.is_empty() => style = Some(value.to_owned()),
            None => {}
            Some(kind) if value.is_empty() => {
                let name = match kind {
                    '#' => "id",
                    '.' => "role",
                    _ => "option",
                };
                warn(format!("invalid empty {name} detected in style attribute"));
            }
            Some('#') => {
                if id.is_some() {
                    warn("multiple ids detected in style attribute".to_owned());
                }
                id = Some(value.to_owned());
            }
            Some('.') => roles.push(value.to_owned()),
            Some(_) => options.push(value.to_owned()),
        }
    };
    let mut sigil = None;
    let mut start = 0;
    for (i, c) in raw.char_indices() {
        if matches!(c, '.' | '#' | '%') {
            buffered(sigil, &raw[start..i]);
            sigil = Some(c);
            start = i + 1;
        }
    }
    buffered(sigil, &raw[start..]);
    if let Some(style) = &style {
        pending.attrs.style = Some(style.clone());
    }
    if id.is_some() {
        pending.id = id;
    }
    pending.roles.extend(roles);
    for option in options {
        add_option(pending, &option);
    }
    style
}

/// Refresh the public block attributes from Ruby's keyed representation.
fn sync(pending: &mut PendingMeta) {
    let mut named = pending.list.named.clone();
    let mut first = 2;
    if pending.attrs.style.as_deref() == Some("source") {
        // `[source, language, linenums]` rekeys the second positional, even
        // over a named `language`. An empty language carries none.
        if let Some(language) = pending.list.value(2).filter(|v| !v.text.is_empty()) {
            named.insert("language".to_owned(), language.text.clone());
        }
        first = 3;
    }
    let last = pending
        .list
        .positional
        .keys()
        .next_back()
        .copied()
        .unwrap_or(0);
    pending.attrs.positional = (first..=last)
        .map(|key| {
            pending
                .list
                .value(key)
                .map(|v| v.text.clone())
                .unwrap_or_default()
        })
        .collect();
    pending.attrs.named = named;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(lines: &[&str]) -> (PendingMeta, Vec<Diagnostic>) {
        let mut pending = PendingMeta::default();
        let mut diagnostics = Vec::new();
        for line in lines {
            let parsed = parse_block_attr_list(line, 0).expect("attribute line");
            merge(
                &mut pending,
                parsed,
                Span::new(0, line.len()),
                &mut diagnostics,
            );
        }
        (pending, diagnostics)
    }

    #[test]
    fn recognises_ruby_attribute_lines() {
        assert_eq!(attr_list_inner("[]"), Some(""));
        assert_eq!(attr_list_inner("[{s}, x]"), Some("{s}, x"));
        assert_eq!(attr_list_inner("[,B]"), Some(",B"));
        assert_eq!(attr_list_inner("['a']"), Some("'a'"));
        assert_eq!(attr_list_inner("[Ünï]"), Some("Ünï"));
        // Leading blanks, anchors, other first characters and partial lines.
        assert_eq!(attr_list_inner(" [quote]"), None);
        assert_eq!(attr_list_inner("[ quote]"), None);
        assert_eq!(attr_list_inner("[[x y]]"), None);
        assert_eq!(attr_list_inner("[*x]"), None);
        assert_eq!(attr_list_inner("text [#id]"), None);
        // Offsets point inside the brackets.
        let line = parse_block_attr_list("[quote, A]", 10).unwrap();
        assert_eq!(line.base, 11);
    }

    #[test]
    fn shorthand_and_named_forms() {
        let (p, d) = meta(&["[source#x.foo%open,ruby]"]);
        assert_eq!(p.attrs.style.as_deref(), Some("source"));
        assert_eq!(p.id.as_deref(), Some("x"));
        assert_eq!(p.roles, ["foo"]);
        assert_eq!(p.attrs.options, ["open"]);
        assert_eq!(p.attrs.named["language"], "ruby");
        assert!(p.attrs.positional.is_empty() && d.is_empty());
        // Shorthand is applied after the named fields of the same line.
        let (p, _) = meta(&["[#a, id=b]"]);
        assert_eq!(p.id.as_deref(), Some("a"));
        let (p, _) = meta(&["[.x, role=y]"]);
        assert_eq!(p.roles, ["y", "x"]);
        // A space disables shorthand; ids and roles are not validated.
        let (p, _) = meta(&["[#bad id]"]);
        assert_eq!((p.id, p.attrs.style.as_deref()), (None, Some("#bad id")));
        let (p, _) = meta(&["[#a&b.c:d]"]);
        assert_eq!(
            (p.id.as_deref(), p.roles.as_slice()),
            (Some("a&b"), &["c:d".to_owned()][..])
        );
        // Options: comma lists drop spaces; a single value keeps them.
        let (p, _) = meta(&["[%a%b, options=\"c, d\", opts=\"e f\"]"]);
        // Named fields are merged before the first-positional shorthand.
        assert_eq!(p.attrs.options, ["c", "d", "e f", "a", "b"]);
    }

    #[test]
    fn shorthand_warnings_follow_ruby() {
        let (p, d) = meta(&["[#a#b]"]);
        assert_eq!(p.id.as_deref(), Some("b"));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "multiple ids detected in style attribute");
        let (_, d) = meta(&["[#]"]);
        assert_eq!(d[0].message, "invalid empty id detected in style attribute");
        let (p, d) = meta(&["[quote..r%]"]);
        assert_eq!(p.roles, ["r"]);
        let messages: Vec<_> = d.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "invalid empty role detected in style attribute",
                "invalid empty option detected in style attribute"
            ]
        );
        assert!(d.iter().all(|d| d.severity == Severity::Warning));
    }

    #[test]
    fn lines_merge_by_ruby_keys() {
        // A later positional replaces the same key; earlier keys remain.
        let (p, _) = meta(&["[quote, A, W1]", "[quote, B]"]);
        assert_eq!(p.attrs.positional, ["B", "W1"]);
        // An explicit nil replaces a value but keeps the style.
        let (p, _) = meta(&["[quote,A]", "[,B]"]);
        assert_eq!(p.attrs.style.as_deref(), Some("quote"));
        assert_eq!(p.attrs.positional, ["B"]);
        // Named and skipped fields occupy positional slots.
        let (p, _) = meta(&["[quote, role=x, Author]"]);
        assert_eq!(p.attrs.positional, ["", "Author"]);
        // A shorthand-only line keeps the previous style.
        let (p, _) = meta(&["[quote]", "[#x]"]);
        assert_eq!(p.attrs.style.as_deref(), Some("quote"));
        assert_eq!(p.list.value(1).unwrap().text, "quote");
        // Roles: the named form replaces, shorthand appends.
        let (p, _) = meta(&["[role=y]", "[.x]"]);
        assert_eq!(p.roles, ["y", "x"]);
        let (p, _) = meta(&["[.x]", "[role=y]"]);
        assert_eq!(p.roles, ["y"]);
    }

    #[test]
    fn source_language_rekeys_the_second_field() {
        let (p, _) = meta(&["[source,ruby,language=py]"]);
        assert_eq!(p.attrs.named["language"], "ruby");
        let (p, _) = meta(&["[source,language=py,ruby]"]);
        assert_eq!(p.attrs.named["language"], "py");
        assert_eq!(p.attrs.positional, ["ruby"]);
        let (p, _) = meta(&["[source, ruby]", "[source]"]);
        assert_eq!(p.attrs.named["language"], "ruby");
    }

    #[test]
    fn values_keep_field_spans() {
        let (p, _) = meta(&["[quote, Ann, \"W\\\"x\"]"]);
        let author = p.list.value(2).unwrap();
        assert_eq!(
            (author.text.as_str(), author.span),
            ("Ann", Span::new(8, 11))
        );
        let work = p.list.value(3).unwrap();
        assert_eq!((work.text.as_str(), work.span), ("W\"x", Span::new(14, 18)));
    }
}
