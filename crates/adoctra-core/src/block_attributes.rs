//! Block attribute lines, as the pinned Ruby parser consumes them: the
//! `BlockAttributeListRx` gate, an early a-stage over the whole line
//! (`sub_input`), AttributeList fields merged `into:` the accumulated metadata
//! by Ruby's integer keys, normal substitutions of single-quoted values
//! (`sub_result`) and the first-positional style shorthand
//! (`style#id.role%option`). Everything runs while reading; effects are not
//! replayed as declarations.
use crate::attribute_list::{self, Field};
use crate::context::ParseContext;
use crate::diagnostic::{Diagnostic, Severity};
use crate::inline_attributes::Reference;
use crate::parser::PendingMeta;
use crate::substitution_text::Text;
use crate::{Inline, Span};
use std::collections::BTreeMap;

/// A recognised attribute line: the text between the brackets and its offset
/// in the processed source.
#[derive(Debug, PartialEq)]
pub(crate) struct AttrLine {
    pub inner: String,
    pub base: usize,
}

/// A positional value as Ruby stores it, with the processed-source range of
/// the field that produced it. `inlines` is the rendered value: substituted
/// for a single-quoted field, otherwise the raw source and the attribute
/// references expanded by the early a-stage.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AttrValue {
    pub text: String,
    pub span: Span,
    pub inlines: Vec<Inline>,
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
    })
}

/// Ruby's `parse_attributes(…, sub_input: true, sub_result: true, into:)`
/// followed by the style shorthand of the merged first positional. Returns the
/// physical spans of the references expanded by the early a-stage.
pub(crate) fn merge(
    pending: &mut PendingMeta,
    line: AttrLine,
    line_span: Span,
    context: &mut ParseContext,
    includes: &crate::xref_target::Includes,
) -> Vec<Span> {
    merge_fields(pending, line, Some(line_span), &[], context, includes)
}

/// A block macro's attribute list (`image::t[…]`): Ruby's
/// `parse_attributes(…, posattrs, sub_input: true, into:)` without
/// `sub_result` or the style shorthand. Positional fields also take the names
/// in `posattrs`, in field order with the named fields.
pub(crate) fn macro_list(
    line: AttrLine,
    posattrs: &[&str],
    context: &mut ParseContext,
    includes: &crate::xref_target::Includes,
) -> (PendingMeta, Vec<Span>) {
    let mut pending = PendingMeta::default();
    let references = merge_fields(&mut pending, line, None, posattrs, context, includes);
    (pending, references)
}

/// The shared field loop: an attribute line (`line_span`) requests
/// `sub_result` and the style shorthand; a macro list names its positionals.
fn merge_fields(
    pending: &mut PendingMeta,
    line: AttrLine,
    line_span: Option<Span>,
    posattrs: &[&str],
    context: &mut ParseContext,
    includes: &crate::xref_target::Includes,
) -> Vec<Span> {
    let mut source = Text::default();
    // sub_attributes does not protect special characters before expansion.
    source.append_source(
        &line.inner,
        true,
        0,
        Span::new(line.base, line.base + line.inner.len()),
    );
    let (expanded, refs) = context.block_attribute_line(source);
    let references = refs.iter().map(|r| r.span).collect();
    let current_style = pending.list.positional.get(&1).cloned().flatten();
    for field in attribute_list::parse(&expanded.text) {
        let name = field.name(&expanded.text).map(str::to_owned);
        let text = field.value.as_ref().map(|_| field_text(&expanded, &field));
        match (name.as_deref(), text) {
            // Titles and reftext share their keys with `.Title` lines and block
            // anchors (last wins) and never request `sub_result`.
            (Some("title"), Some(text)) => {
                let (text, span, exact) = lexical(text);
                pending.title = Some(crate::parser::PendingTitle {
                    text,
                    base: span.start,
                    origin: (!exact).then_some(span),
                });
                continue;
            }
            (Some("reftext"), Some(text)) => {
                let (text, refs) = renumber(text.text, &refs);
                pending.reftext = Some(crate::reftext::Read::expanded(text, refs));
                continue;
            }
            _ => {}
        }
        let substitute = line_span.is_some()
            && field.single_quoted
            && !matches!(
                name.as_deref(),
                Some("title" | "reftext" | "opts" | "options")
            );
        let value = field.value.as_ref().map(|_| {
            field_value(
                field_text(&expanded, &field),
                &refs,
                substitute,
                context,
                includes,
            )
        });
        let Some(name) = name else {
            if let (Some(name), Some(value)) = (posattrs.get(field.index), &value) {
                pending
                    .list
                    .named
                    .insert((*name).to_owned(), value.text.clone());
            }
            pending.list.positional.insert(field.index + 1, value);
            continue;
        };
        let text = value.map(|v| v.text).unwrap_or_default();
        match name.as_str() {
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
                pending.list.named.insert(name, text);
            }
        }
    }
    let Some(line_span) = line_span else {
        sync(pending);
        return references;
    };
    if let Some(raw) = pending.list.value(1).cloned() {
        let style = parse_style(
            pending,
            &raw.text,
            line_span,
            &mut context.inline_diagnostics,
        );
        let style = style
            .map(|text| AttrValue {
                inlines: vec![Inline::Raw {
                    value: text.clone(),
                    span: raw.span,
                }],
                text,
                span: raw.span,
            })
            .or(current_style);
        pending.list.positional.insert(1, style);
    }
    sync(pending);
    references
}

/// One field of the expanded line with its physical range. `exact` holds when
/// the field is unchanged source text, so its own offsets remain valid.
struct FieldText {
    text: Text,
    span: Span,
    exact: bool,
}

fn field_text(expanded: &Text, field: &Field) -> FieldText {
    let range = field.value.clone().unwrap();
    let mut text = Text::default();
    let mut copied = range.start;
    let mut unescaped = false;
    if let Some(quote) = field.quote {
        let bytes = expanded.text.as_bytes();
        let mut i = range.start;
        while i + 1 < range.end {
            if bytes[i] == b'\\' && bytes[i + 1] == quote as u8 {
                expanded.copy(copied..i, &mut text);
                copied = i + 1;
                unescaped = true;
                i += 2;
            } else {
                i += 1;
            }
        }
    }
    if let Some(gap) = &field.gap {
        expanded.copy(copied..gap.start, &mut text);
        expanded.replacement(gap.clone(), &" ".repeat(gap.len()), None, &mut text);
        copied = gap.end;
    }
    expanded.copy(copied..range.end, &mut text);
    let span = if range.is_empty() {
        // An empty quoted value sits right after its opening quote.
        let at = expanded.match_span(range.start - 1..range.start).end;
        Span::new(at, at)
    } else {
        covering(expanded, range.clone())
    };
    let exact = !unescaped
        && text.fragments.iter().all(|(_, f)| f.owner == 0)
        && span.end - span.start == text.text.len();
    FieldText { text, span, exact }
}

/// The physical range from a field's first to its last character. A match
/// starting inside an expanded value covers only that reference, so the ends
/// are mapped separately.
fn covering(expanded: &Text, range: std::ops::Range<usize>) -> Span {
    let text = &expanded.text[range.clone()];
    let first = text.chars().next().map_or(0, char::len_utf8);
    let last = text.chars().next_back().map_or(0, char::len_utf8);
    let start = expanded.match_span(range.start..range.start + first);
    let end = expanded.match_span(range.end - last..range.end);
    if start.source_id != end.source_id || end.end < start.start {
        return start;
    }
    Span {
        end: end.end,
        ..start
    }
}

/// A value that is rescanned later (a title, or a single-quoted value) keeps
/// its source text when exact; otherwise its lexical spelling from the early
/// a-stage, whose protected characters stay escaped.
fn lexical(field: FieldText) -> (String, Span, bool) {
    let text = if field.exact {
        field.text.text
    } else {
        crate::attributes::reference_text(&field.text.into_parts())
    };
    (text, field.span, field.exact)
}

/// A single-quoted value (except the named `title`/`reftext`/`options`)
/// repeats the normal substitutions; other values keep the raw source and
/// expanded references.
fn field_value(
    field: FieldText,
    refs: &[Reference],
    substitute: bool,
    context: &mut ParseContext,
    includes: &crate::xref_target::Includes,
) -> AttrValue {
    if !substitute {
        let (text, refs) = renumber(field.text, refs);
        return AttrValue {
            text: text.text.clone(),
            span: field.span,
            inlines: merge_raw(crate::inline_attributes::nodes(text, refs)),
        };
    }
    let (value, span, exact) = lexical(field);
    let inlines = context.attribute_value(&value, span, exact, includes);
    AttrValue {
        text: visible_text(&inlines),
        span,
        inlines,
    }
}

/// Keep only the references that contributed to this field, renumbering their
/// owners. A reference split across fields appears in each.
fn renumber(mut text: Text, refs: &[Reference]) -> (Text, Vec<Reference>) {
    let mut owners = Vec::new();
    for (_, fragment) in &mut text.fragments {
        if fragment.owner > 0 {
            let index = match owners.iter().position(|o| *o == fragment.owner) {
                Some(index) => index,
                None => {
                    owners.push(fragment.owner);
                    owners.len() - 1
                }
            };
            fragment.owner = index + 1;
        }
    }
    let refs = owners
        .iter()
        .map(|owner| {
            let r = &refs[owner - 1];
            Reference {
                span: r.span,
                name: r.name.clone(),
                missing: r.missing,
                drops_line: r.drops_line,
            }
        })
        .collect();
    (text, refs)
}

/// The a-stage copies source in pieces; contiguous raw source is one node.
fn merge_raw(nodes: Vec<Inline>) -> Vec<Inline> {
    let mut merged: Vec<Inline> = Vec::new();
    for node in nodes {
        if let (
            Some(Inline::Raw { value, span }),
            Inline::Raw {
                value: next,
                span: next_span,
            },
        ) = (merged.last_mut(), &node)
        {
            if span.end == next_span.start && span.source_id == next_span.source_id {
                value.push_str(next);
                span.end = next_span.end;
                continue;
            }
        }
        merged.push(node);
    }
    merged
}

/// Ruby stores substituted strings for other fields; adoctra keeps their
/// visible text.
fn visible_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text { value, .. } | Inline::Raw { value, .. } => out.push_str(value),
            Inline::AttributeRef { name, value, .. } => match value {
                Some(value) => out.push_str(value),
                None => out.push_str(&format!("{{{name}}}")),
            },
            Inline::Substituted { parts, .. } => {
                out.push_str(&crate::attributes::parts_text(parts))
            }
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Monospace { children, .. }
            | Inline::Mark { children, .. }
            | Inline::Superscript { children, .. }
            | Inline::Subscript { children, .. }
            | Inline::Link { children, .. }
            | Inline::CrossReference { children, .. } => out.push_str(&visible_text(children)),
            _ => {}
        }
    }
    out
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
        let mut context = ParseContext::new(&crate::ParseOptions::default());
        for line in lines {
            let parsed = parse_block_attr_list(line, 0).expect("attribute line");
            merge(
                &mut pending,
                parsed,
                Span::new(0, line.len()),
                &mut context,
                &Default::default(),
            );
        }
        (pending, context.inline_diagnostics)
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
