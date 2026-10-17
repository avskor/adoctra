//! Macro substitutions for pass attributes and expanded inline text: links,
//! emails, inline anchors and (for inline text) cross-references. Matched over
//! the shared lexical stream; generated constructs remain backend-neutral
//! boundaries.
use crate::inline_pass::Passes;
use crate::substitution_context::Evaluation;
use crate::substitution_text::Text;
use crate::{AttributeFormat, AttributeLink, AttributePart as Part, Inline, Span};
use std::collections::BTreeMap;
use std::ops::Range;

const SCHEMES: [&str; 5] = ["https://", "http://", "file://", "ftp://", "irc://"];

struct Report<'a> {
    links: Vec<Span>,
    anchors: Vec<Span>,
    xrefs: Vec<Span>,
    /// Physical label range of each cross-reference, keyed by its match range.
    labels: BTreeMap<(usize, usize), Span>,
    passes: &'a mut Passes,
}

impl<'a> Report<'a> {
    fn new(passes: &'a mut Passes) -> Self {
        Self {
            links: Vec::new(),
            anchors: Vec::new(),
            xrefs: Vec::new(),
            labels: BTreeMap::new(),
            passes,
        }
    }
}

/// Physical ranges of constructs consumed by the macros stage, for highlighting.
#[derive(Default)]
pub(crate) struct Matches {
    pub links: Vec<Span>,
    pub anchors: Vec<Span>,
    pub xrefs: Vec<Span>,
}

pub(crate) fn apply(
    parts: Vec<Part>,
    attributes: &mut Evaluation<'_>,
    compat: bool,
    span: Span,
    depth: usize,
    passes: &mut Passes,
) -> Vec<Part> {
    let mut report = Report::new(passes);
    apply_inner(parts, attributes, compat, depth, span, &mut report)
}

/// Link macros run over the lexical result of quotes, attributes and replacements.
/// Flatten source quote boundaries temporarily so a macro can consume a formatted
/// label. Surviving boundaries are rebuilt into their original spanned AST nodes.
pub(crate) fn apply_inlines(
    nodes: &mut Vec<Inline>,
    attributes: &mut Evaluation<'_>,
    fold_newlines: bool,
    passes: &mut Passes,
) -> Matches {
    let mut report = Report::new(passes);
    process_inlines(nodes, attributes, true, fold_newlines, &mut report);
    Matches {
        links: report.links,
        anchors: report.anchors,
        xrefs: report.xrefs,
    }
}

pub(crate) fn format(node: &Inline) -> Option<AttributeFormat> {
    Some(match node {
        Inline::Strong { .. } => AttributeFormat::Strong,
        Inline::Emphasis { .. } => AttributeFormat::Emphasis,
        Inline::Monospace { .. } => AttributeFormat::Monospace,
        Inline::Mark { .. } => AttributeFormat::Mark,
        Inline::Superscript { .. } => AttributeFormat::Superscript,
        Inline::Subscript { .. } => AttributeFormat::Subscript,
        _ => return None,
    })
}

fn children(node: &mut Inline) -> &mut Vec<Inline> {
    match node {
        Inline::Strong { children, .. }
        | Inline::Emphasis { children, .. }
        | Inline::Monospace { children, .. }
        | Inline::Mark { children, .. }
        | Inline::Superscript { children, .. }
        | Inline::Subscript { children, .. } => children,
        _ => unreachable!(),
    }
}

fn boundary(part: Part, span: Span) -> Inline {
    Inline::Substituted {
        value: String::new(),
        parts: vec![part],
        span,
    }
}

fn flatten(nodes: Vec<Inline>, flat: &mut Vec<Inline>, quotes: &mut Vec<Inline>) {
    for mut node in nodes {
        if let Some(kind) = format(&node) {
            let span = node.span();
            let contents = children(&mut node);
            let open_end = contents.first().map_or(span.start + 1, |n| n.span().start);
            let close_start = contents.last().map_or(span.end - 1, |n| n.span().end);
            flat.push(boundary(
                Part::Open(kind),
                Span {
                    end: open_end,
                    ..span
                },
            ));
            flatten(std::mem::take(children(&mut node)), flat, quotes);
            flat.push(boundary(
                Part::Close(kind),
                Span {
                    start: close_start,
                    ..span
                },
            ));
            quotes.push(node);
        } else {
            flat.push(node);
        }
    }
}

pub(crate) fn rebuild(nodes: Vec<Inline>, quotes: Vec<Inline>) -> Vec<Inline> {
    let mut openings: BTreeMap<_, _> = quotes.into_iter().map(|n| (n.span().start, n)).collect();
    let mut output = Vec::new();
    let mut stack: Vec<(Inline, Inline, Vec<Inline>)> = Vec::new();
    for node in nodes {
        if let Inline::Substituted { parts, span, .. } = &node {
            if let [Part::Open(kind)] = parts.as_slice() {
                if openings
                    .get(&span.start)
                    .is_some_and(|n| format(n) == Some(*kind))
                {
                    let template = openings.remove(&span.start).unwrap();
                    stack.push((template, node, std::mem::take(&mut output)));
                    continue;
                }
            }
            if let [Part::Close(kind)] = parts.as_slice() {
                if stack
                    .last()
                    .is_some_and(|(n, _, _)| format(n) == Some(*kind) && n.span().end == span.end)
                {
                    let (mut template, _, mut parent) = stack.pop().unwrap();
                    *children(&mut template) = output;
                    parent.push(template);
                    output = parent;
                    continue;
                }
            }
        }
        output.push(node);
    }
    // A macro may consume only one boundary. Preserve the remaining semantic
    // boundary literally instead of inventing a differently nested source tree.
    while let Some((_, open, mut parent)) = stack.pop() {
        parent.push(open);
        parent.extend(output);
        output = parent;
    }
    output
}

fn process_inlines(
    nodes: &mut Vec<Inline>,
    attributes: &mut Evaluation<'_>,
    edges: bool,
    fold_newlines: bool,
    report: &mut Report<'_>,
) {
    let mut left_edge = edges;
    let mut flat = Vec::new();
    let mut quotes = Vec::new();
    flatten(std::mem::take(nodes), &mut flat, &mut quotes);
    let mut pending = Vec::new();
    let mut output = Vec::new();
    for mut node in flat {
        if matches!(
            node,
            Inline::Text { .. }
                | Inline::Raw { .. }
                | Inline::Substituted { .. }
                | Inline::AttributeRef { .. }
        ) {
            pending.push(node);
            continue;
        }
        flush_inlines(
            &mut pending,
            &mut output,
            attributes,
            (left_edge, false),
            fold_newlines,
            report,
        );
        if let Inline::Link {
            children,
            bare: false,
            ..
        } = &mut node
        {
            process_inlines(children, attributes, false, true, report);
        }
        output.push(node);
        left_edge = false;
    }
    flush_inlines(
        &mut pending,
        &mut output,
        attributes,
        (left_edge, edges),
        fold_newlines,
        report,
    );
    *nodes = rebuild(output, quotes);
}

fn flush_inlines(
    nodes: &mut Vec<Inline>,
    output: &mut Vec<Inline>,
    attributes: &mut Evaluation<'_>,
    edges: (bool, bool),
    fold_newlines: bool,
    report: &mut Report<'_>,
) {
    if nodes.is_empty() {
        return;
    }
    let mut text = Text::default();
    for (owner, node) in nodes.iter().enumerate() {
        match node {
            Inline::AttributeRef {
                name,
                value: None,
                span,
                ..
            } => {
                text.append(&format!("{{{name}}}"), false, owner, *span, false);
            }
            Inline::Text { value, span } | Inline::Raw { value, span } => {
                text.append_source(value, matches!(node, Inline::Raw { .. }), owner, *span);
            }
            Inline::AttributeRef {
                value: Some(value),
                parts,
                span,
                ..
            }
            | Inline::Substituted { value, parts, span } => {
                if parts.is_empty() {
                    text.append(value, false, owner, *span, false);
                } else {
                    for part in parts {
                        text.append_part(part, owner, *span);
                    }
                }
            }
            _ => unreachable!(),
        }
    }
    text = crate::replacements::apply_text(text, edges.0, edges.1);
    for stage in 0..3 {
        text = apply_stage(
            text,
            stage,
            attributes,
            attributes.get("compat-mode").is_some(),
            0,
            edges.0,
            report,
        );
    }
    text = apply_anchors(text, report);
    text = apply_xrefs(text, attributes, report);
    text = crate::post_replacements::apply(text, attributes.hardbreaks(), edges.1);
    let written = crate::substitution_text::write_back(std::mem::take(nodes), text, fold_newlines);
    // Source-owned anchors and cross-references keep their spanned AST nodes;
    // attribute-owned ones remain parts of their reference.
    output.extend(written.into_iter().map(|node| match node {
        Inline::Substituted { parts, span, .. }
            if matches!(parts.as_slice(), [Part::Anchor(_)]) =>
        {
            let Some(Part::Anchor(id)) = parts.into_iter().next() else {
                unreachable!()
            };
            Inline::Anchor { id, span }
        }
        Inline::Substituted { parts, span, .. } if single_xref(&parts) => {
            let label = report
                .labels
                .get(&(span.start, span.end))
                .copied()
                .unwrap_or(span);
            xref_node(parts, span, label, fold_newlines)
        }
        node => node,
    }));
}

/// Exactly one generated cross-reference and nothing around it.
fn single_xref(parts: &[Part]) -> bool {
    matches!(
        (parts.first(), parts.last()),
        (Some(Part::OpenXref { .. }), Some(Part::CloseXref))
    ) && parts
        .iter()
        .filter(|p| matches!(p, Part::OpenXref { .. } | Part::CloseXref))
        .count()
        == 2
}

/// A source-owned reference becomes its AST node. The consumed label keeps
/// its text and semantic boundaries; a textless one stays empty for conversion.
fn xref_node(mut parts: Vec<Part>, span: Span, label: Span, fold_newlines: bool) -> Inline {
    parts.pop();
    let Part::OpenXref { target, role } = parts.remove(0) else {
        unreachable!()
    };
    let children = if parts.iter().any(Part::is_boundary) {
        vec![Inline::Substituted {
            value: crate::attributes::parts_text(&parts),
            parts,
            span: label,
        }]
    } else {
        parts
            .into_iter()
            .map(|part| match part {
                Part::Raw(value) => Inline::Raw { value, span: label },
                part => Inline::Text {
                    value: if fold_newlines {
                        part.text().replace(['\r', '\n'], " ")
                    } else {
                        part.text().into()
                    },
                    span: label,
                },
            })
            .collect()
    };
    Inline::CrossReference {
        target: crate::attributes::decode_special(&target),
        role: role.as_deref().map(crate::attributes::decode_special),
        children,
        span,
    }
}

fn apply_inner(
    parts: Vec<Part>,
    attributes: &mut Evaluation<'_>,
    compat: bool,
    depth: usize,
    span: Span,
    report: &mut Report<'_>,
) -> Vec<Part> {
    let mut text = Text::from_parts_at(&parts, span);
    for stage in 0..3 {
        text = apply_stage(text, stage, attributes, compat, depth, true, report);
    }
    apply_anchors(text, report).into_parts()
}

/// Ruby's `InlineAnchorRx` after links and emails, over the lexical result of
/// the earlier stages: an expanded reference can complete or break an anchor.
/// An escaped anchor loses its backslash. The reftext only feeds the catalog
/// built while reading, so the consumed range becomes a bare anchor.
fn apply_anchors(text: Text, report: &mut Report<'_>) -> Text {
    let src = &text.text;
    if !src.contains("[[") && !src.contains("anchor:") {
        return text;
    }
    let mut output = Text::default();
    let mut copied = 0;
    let mut at = 0;
    while at < src.len() {
        let escaped = src.as_bytes()[at] == b'\\';
        let start = at + usize::from(escaped);
        let found = if src[start..].starts_with("[[") {
            crate::anchor_catalog::shorthand_at(src, start)
        } else if src[start..].starts_with("anchor:") {
            crate::anchor_catalog::macro_at(src, start)
        } else {
            None
        };
        let Some(found) = found else {
            at += src[at..].chars().next().unwrap().len_utf8();
            continue;
        };
        text.copy(copied..at, &mut output);
        if escaped {
            text.copy(start..found.end, &mut output);
        } else {
            report.anchors.push(text.match_span(at..found.end));
            let id = src[found.id].to_owned();
            text.replace_parts(at..found.end, &[Part::Anchor(id)], &mut output);
        }
        copied = found.end;
        at = found.end;
    }
    text.copy(copied..src.len(), &mut output);
    output
}

/// Ruby's `InlineXrefMacroRx`, the last macro: `<<id[,label]>>` over the
/// protected spelling and `xref:id[label]`, matched after inline anchors over
/// the lexical result of the earlier stages, so `<<{id}>>` takes the expanded
/// id. An escaped reference loses its backslash. A textless reference keeps an
/// empty label, filled from the registry during conversion.
fn apply_xrefs(text: Text, attributes: &mut Evaluation<'_>, report: &mut Report<'_>) -> Text {
    let src = &text.text;
    if !src.contains("&lt;&lt;") && !src.contains("xref:") {
        return text;
    }
    let compat = attributes.get("compat-mode").is_some();
    let mut output = Text::default();
    let mut copied = 0;
    let mut at = 0;
    while at < src.len() {
        let escaped = src.as_bytes()[at] == b'\\';
        let start = at + usize::from(escaped);
        let Some(m) = xref_at(src, start) else {
            at += src[at..].chars().next().unwrap().len_utf8();
            continue;
        };
        text.copy(copied..at, &mut output);
        if escaped {
            text.copy(start..m.end, &mut output);
        } else {
            let span = text.match_span(at..m.end);
            report.xrefs.push(span);
            let parts = xref_parts(&text, &m, attributes, compat, span, report);
            text.replace_parts(at..m.end, &parts, &mut output);
        }
        copied = m.end;
        at = m.end;
    }
    text.copy(copied..src.len(), &mut output);
    output
}

struct XrefMatch {
    end: usize,
    refid: Range<usize>,
    label: Option<Range<usize>>,
    macro_form: bool,
}

/// The refid starts with a word character or one of `#/.:{`, then extends
/// lazily (across lines) to the first closer.
fn xref_at(src: &str, start: usize) -> Option<XrefMatch> {
    let rest = &src[start..];
    let (refid, macro_form) = if rest.starts_with("&lt;&lt;") {
        (start + 8, false)
    } else if rest.starts_with("xref:") {
        (start + 5, true)
    } else {
        return None;
    };
    let first = src[refid..].chars().next()?;
    if !(word(first) || matches!(first, '#' | '/' | '.' | ':' | '{')) {
        return None;
    }
    let lazy = refid + first.len_utf8();
    if !macro_form {
        let close = lazy + src[lazy..].find("&gt;&gt;")?;
        let (id_end, label) = match src[refid..close].find(',') {
            Some(comma) => {
                let after = refid + comma + 1;
                let lead = src[after..close].len()
                    - src[after..close]
                        .trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r', '\0'])
                        .len();
                (
                    refid + comma,
                    (after + lead < close).then_some(after + lead..close),
                )
            }
            None => (close, None),
        };
        return Some(XrefMatch {
            end: close + 8,
            refid: refid..id_end,
            label,
            macro_form,
        });
    }
    // A later bracket can only close where an earlier one could, so the
    // first bracket decides whether the macro matches at all.
    let bracket = lazy + src[lazy..].find('[')?;
    if src[bracket + 1..].starts_with(']') {
        return Some(XrefMatch {
            end: bracket + 2,
            refid: refid..bracket,
            label: None,
            macro_form,
        });
    }
    let lazy = bracket + 1 + src[bracket + 1..].chars().next()?.len_utf8();
    let close = src[lazy..]
        .match_indices(']')
        .map(|(i, _)| lazy + i)
        .find(|&i| src.as_bytes()[i - 1] != b'\\')?;
    Some(XrefMatch {
        end: close + 1,
        refid: refid..bracket,
        label: Some(bracket + 1..close),
        macro_form,
    })
}

/// The reference's semantic parts. Outside compat mode a leading `#` names a
/// fragment of this document; the macro form unescapes `\]` and, with an
/// `=`, reads its label and role from an AttributeList like Ruby's
/// `extract_attributes_from_text`. Interdocument paths and natural (title)
/// references keep the refid unchanged.
fn xref_parts(
    text: &Text,
    m: &XrefMatch,
    attributes: &mut Evaluation<'_>,
    compat: bool,
    span: Span,
    report: &mut Report<'_>,
) -> Vec<Part> {
    let mut refid = m.refid.clone();
    if !compat
        && text.text[refid.clone()].starts_with('#')
        && !text.text[refid.clone()].ends_with('&')
    {
        refid.start += 1;
    }
    let target = crate::attributes::reference_text(&slice(text, refid, None, false));
    let mut label = m
        .label
        .clone()
        .map_or_else(Vec::new, |r| slice(text, r, None, m.macro_form));
    let mut role = None;
    if m.macro_form && !compat && crate::attributes::reference_text(&label).contains('=') {
        let (first, mut named, _) = parse_label(&label, attributes, compat, 0, span, report);
        label = first;
        role = named.remove("role");
    }
    if let Some(range) = m.label.clone() {
        report
            .labels
            .insert((span.start, span.end), text.cover_span(range));
    }
    let mut parts = vec![Part::OpenXref { target, role }];
    parts.extend(label);
    parts.push(Part::CloseXref);
    parts
}

fn space(ch: char) -> bool {
    matches!(ch, ' ' | '\t'..='\r')
}
fn word(ch: char) -> bool {
    crate::unicode_word::is_alphanumeric(ch) || crate::unicode_word::is_mark_or_connector(ch)
}

struct LinkMatch {
    start: usize,
    end: usize,
    target: Range<usize>,
    label: Option<Range<usize>>,
    mailto: bool,
    escaped: Option<usize>,
    ignored: bool,
}

/// Index closers once so incomplete macro runs cannot repeatedly scan a suffix.
struct Index {
    closes: Vec<usize>,
    target_ends: Vec<usize>,
    url_ends: Vec<usize>,
    angle_ends: Vec<usize>,
}
impl Index {
    fn new(src: &str) -> Self {
        Self {
            closes: src
                .match_indices(']')
                .filter_map(|(i, _)| (i == 0 || src.as_bytes()[i - 1] != b'\\').then_some(i))
                .collect(),
            target_ends: src
                .char_indices()
                .filter_map(|(i, c)| (space(c) || c == '[').then_some(i))
                .collect(),
            url_ends: src
                .char_indices()
                .filter_map(|(i, c)| (space(c) || matches!(c, '[' | ']' | '<')).then_some(i))
                .collect(),
            angle_ends: src.match_indices("&gt;").map(|(i, _)| i).collect(),
        }
    }
    fn next(positions: &[usize], at: usize, fallback: usize) -> usize {
        positions
            .get(positions.partition_point(|&i| i < at))
            .copied()
            .unwrap_or(fallback)
    }
    fn label(&self, src: &str, bracket: usize) -> Option<Range<usize>> {
        if src.as_bytes().get(bracket) != Some(&b'[') {
            return None;
        }
        let close = Self::next(&self.closes, bracket + 1, src.len());
        (close < src.len()).then_some(bracket + 1..close)
    }
}

fn apply_stage(
    text: Text,
    stage: usize,
    attributes: &mut Evaluation<'_>,
    compat: bool,
    depth: usize,
    left_edge: bool,
    report: &mut Report<'_>,
) -> Text {
    let src = &text.text;
    let index = Index::new(src);
    let mut output = Text::default();
    let mut copied = 0;
    let mut at = 0;
    while at < src.len() {
        let found = match stage {
            0 => url(src, at, copied, &index),
            1 => explicit(src, at, &index),
            _ => email(src, at),
        };
        let Some(mut m) = found else {
            at += src[at..].chars().next().unwrap().len_utf8();
            continue;
        };
        // A child's lexical predecessor is a generated tag's `>`, not the
        // start of the full substitution stream. Ruby suppresses bare emails
        // here, while escaped emails still lose their escape.
        if stage == 2 && at == 0 && !left_edge && m.escaped.is_none() {
            m.ignored = true;
        }
        if m.ignored {
            at = m.end;
            continue;
        }
        text.copy(copied..m.start, &mut output);
        if let Some(escape) = m.escaped {
            text.copy(m.start..escape, &mut output);
            text.copy(escape + 1..m.end, &mut output);
        } else {
            report.links.push(text.match_span(m.start..m.end));
            let target = &src[m.target.clone()];
            let (link, label) = link_parts(&text, &m, target, attributes, compat, depth, report);
            let mut parts = vec![Part::OpenLink(link)];
            parts.extend(label);
            parts.push(Part::CloseLink);
            text.replace_parts(m.start..m.end, &parts, &mut output);
        }
        copied = m.end;
        at = m.end;
    }
    text.copy(copied..src.len(), &mut output);
    output
}

fn explicit(src: &str, at: usize, index: &Index) -> Option<LinkMatch> {
    let escaped = src.as_bytes()[at] == b'\\';
    let start = at + usize::from(escaped);
    let prefix = if src[start..].starts_with("link:") {
        "link:"
    } else if src[start..].starts_with("mailto:") {
        "mailto:"
    } else {
        return None;
    };
    let target = start + prefix.len();
    let bracket = Index::next(&index.target_ends, target, src.len());
    if src[target..bracket].starts_with(':') {
        return None;
    }
    let label = index.label(src, bracket)?;
    Some(LinkMatch {
        start: at,
        end: label.end + 1,
        target: target..bracket,
        label: Some(label),
        mailto: prefix == "mailto:",
        escaped: escaped.then_some(at),
        ignored: false,
    })
}

fn url(src: &str, at: usize, copied: usize, index: &Index) -> Option<LinkMatch> {
    let mut start = at;
    let mut prefix_escape = None;
    let explicit = src[at..].starts_with("link:");
    let angle = src[at..].starts_with("&lt;") || src[at..].starts_with("\\&lt;");
    if explicit {
        start += 5;
    } else if angle {
        if src.as_bytes()[start] == b'\\' {
            prefix_escape = Some(start);
            start += 1;
        }
        start += 4;
    } else if at > 0 {
        let (prev_at, prev) = src[..at].char_indices().next_back()?;
        if prev_at < copied
            || !(prev.is_whitespace()
                || matches!(prev, '>' | '(' | ')' | '[' | ']' | ';' | '"' | '\''))
        {
            return None;
        }
    }
    let escape = (src.as_bytes().get(start) == Some(&b'\\')).then_some(start);
    let target = start + usize::from(escape.is_some());
    let scheme = SCHEMES
        .iter()
        .find(|scheme| src[target..].starts_with(**scheme))?;
    let run_end = Index::next(&index.url_ends, target, src.len());
    // The URL form disallows brackets in its target, while explicit link:
    // has its own later grammar. Bracketed URL macros take precedence over <>.
    let label = (run_end > target + scheme.len())
        .then(|| index.label(src, run_end))
        .flatten();
    if let Some(label) = label {
        return Some(LinkMatch {
            start: if explicit { at } else { start },
            end: label.end + 1,
            target: target..run_end,
            label: Some(label),
            mailto: false,
            escaped: escape,
            ignored: false,
        });
    }
    if angle {
        let end = Index::next(&index.angle_ends, target + scheme.len(), src.len());
        if end > target + scheme.len() && end < src.len() && !src[target..end].contains(space) {
            return Some(LinkMatch {
                start: at,
                end: end + 4,
                target: target..end,
                label: None,
                mailto: false,
                escaped: prefix_escape.or(escape),
                ignored: false,
            });
        }
        // The reference consumes an unmatched angle-prefixed URL unchanged;
        // its trailing semicolon cannot donate a boundary to another match.
        return Some(LinkMatch {
            start: at,
            end: run_end,
            target: target..run_end,
            label: None,
            mailto: false,
            escaped: prefix_escape.or(escape),
            ignored: prefix_escape.is_none() && escape.is_none(),
        });
    }
    let mut end = run_end;
    while end > target + scheme.len() && src[..end].ends_with([',', '.', '?', '!', ')']) {
        end -= 1;
    }
    if escape.is_some() && end > target + scheme.len() {
        return Some(LinkMatch {
            start: at,
            end,
            target: target..end,
            label: None,
            mailto: false,
            escaped: escape,
            ignored: false,
        });
    }
    if end > target + scheme.len() && src[..end].ends_with([';', ':']) {
        end -= 1;
        if src[..end].ends_with(')') {
            end -= 1;
        }
    }
    if end <= target + scheme.len() {
        return None;
    }
    let ignored = explicit || (escape.is_none() && src[..at].ends_with(['"', '\'']));
    Some(LinkMatch {
        start: at,
        end,
        target: target..end,
        label: None,
        mailto: false,
        escaped: escape,
        ignored,
    })
}

fn email(src: &str, at: usize) -> Option<LinkMatch> {
    let escaped = src.as_bytes()[at] == b'\\';
    let start = at + usize::from(escaped);
    let first = src[start..].chars().next()?;
    if !word(first) {
        return None;
    }
    let mut i = start + first.len_utf8();
    while i < src.len() {
        if src[i..].starts_with("&amp;") {
            i += 5;
            continue;
        }
        let c = src[i..].chars().next()?;
        if word(c) || matches!(c, '-' | '.' | '%' | '+') {
            i += c.len_utf8();
        } else {
            break;
        }
    }
    let no_match = LinkMatch {
        start: at,
        end: i,
        target: start..i,
        label: None,
        mailto: true,
        escaped: None,
        ignored: true,
    };
    if src.as_bytes().get(i) != Some(&b'@') {
        return Some(no_match);
    }
    i += 1;
    if !src[i..]
        .chars()
        .next()
        .is_some_and(crate::unicode_word::is_alphanumeric)
    {
        return Some(no_match);
    }
    let domain = i;
    while i < src.len() {
        let c = src[i..].chars().next()?;
        if crate::unicode_word::is_alphanumeric(c) || matches!(c, '_' | '-' | '.') {
            i += c.len_utf8();
        } else {
            break;
        }
    }
    // Ruby greedily scans the domain, then backtracks to a 2–5 letter suffix
    // ending at a word boundary (which may precede a dash or another dot).
    let end = src[domain..i].rmatch_indices('.').find_map(|(dot, _)| {
        let start = domain + dot + 1;
        let len = src[start..]
            .bytes()
            .take(5)
            .take_while(u8::is_ascii_alphabetic)
            .count();
        (len >= 2 && !src[start + len..].chars().next().is_some_and(word)).then_some(start + len)
    });
    let Some(end) = end else {
        return Some(no_match);
    };
    let ignored = !escaped && src[..at].ends_with(['>', ':', '/']);
    Some(LinkMatch {
        start: at,
        end,
        target: start..end,
        label: None,
        mailto: true,
        escaped: escaped.then_some(at),
        ignored,
    })
}

fn slice(text: &Text, range: Range<usize>, unescape: Option<char>, brackets: bool) -> Vec<Part> {
    let mut out = Text::default();
    let mut copied = range.start;
    let mut chars = text.text[range.clone()].char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\\'
            && chars
                .peek()
                .is_some_and(|(_, c)| (brackets && *c == ']') || Some(*c) == unescape)
        {
            text.copy(copied..range.start + i, &mut out);
            copied = range.start + i + 1;
            chars.next();
        }
    }
    text.copy(copied..range.end, &mut out);
    out.into_parts()
}

fn link_parts(
    text: &Text,
    m: &LinkMatch,
    target: &str,
    attributes: &mut Evaluation<'_>,
    compat: bool,
    depth: usize,
    report: &mut Report<'_>,
) -> (AttributeLink, Vec<Part>) {
    let mut link = AttributeLink {
        target: if m.mailto {
            format!("mailto:{target}")
        } else {
            target.into()
        },
        ..Default::default()
    };
    let mut label = m
        .label
        .clone()
        .map_or_else(Vec::new, |r| slice(text, r, None, true));
    let mut lexical = crate::attributes::reference_text(&label);
    let mut attrs = BTreeMap::new();
    if !compat && ((!m.mailto && lexical.contains('=')) || (m.mailto && lexical.contains(','))) {
        let (first, parsed, positional) = parse_label(
            &label,
            attributes,
            compat,
            depth,
            text.match_span(m.start..m.end),
            report,
        );
        label = first;
        attrs = parsed;
        lexical = crate::attributes::reference_text(&label);
        if m.mailto {
            if let Some(subject) = positional.get(&2) {
                link.target.push_str("?subject=");
                link.target.push_str(&encode_uri(subject));
                if let Some(body) = positional.get(&3) {
                    link.target.push_str("&amp;body=");
                    link.target.push_str(&encode_uri(body));
                }
            }
        }
    }
    if lexical.ends_with('^') {
        let text = Text::from_parts(&label);
        label = slice(&text, 0..text.text.len() - 1, None, true);
        attrs
            .entry("window".into())
            .or_insert_with(|| "_blank".into());
        lexical.pop();
    }
    if lexical.is_empty() {
        let display = if !m.mailto && attributes.get("hide-uri-scheme").is_some() {
            strip_scheme(target)
        } else {
            target
        };
        let display = if display.is_empty() { target } else { display };
        // Keep semantic/literal fragments in the fallback target too.
        let offset = target.len() - display.len();
        label = slice(text, m.target.start + offset..m.target.end, None, true);
        if !m.mailto {
            attrs
                .entry("role".into())
                .and_modify(|role| *role = format!("bare {role}"))
                .or_insert_with(|| "bare".into());
        }
    }
    link.id = attrs.remove("id");
    link.role = attrs.remove("role");
    link.title = attrs.remove("title");
    link.window = attrs.remove("window");
    let nofollow = attrs.contains_key("nofollow-option");
    let noopener =
        link.window.as_deref() == Some("_blank") || attrs.contains_key("noopener-option");
    link.rel = if link.window.is_some() {
        noopener.then(|| {
            if nofollow {
                "nofollow noopener"
            } else {
                "noopener"
            }
            .into()
        })
    } else {
        nofollow.then(|| "nofollow".into())
    };
    (link, label)
}

fn strip_scheme(target: &str) -> &str {
    let Some(colon) = target.find(':') else {
        return target;
    };
    let scheme = &target[..colon];
    if scheme.len() < 2
        || !scheme.as_bytes()[0].is_ascii_alphabetic()
        || !scheme
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'+' | b'-'))
    {
        return target;
    }
    let rest = &target[colon + 1..];
    rest.strip_prefix("//")
        .or_else(|| rest.strip_prefix('/'))
        .unwrap_or(rest)
}

fn encode_uri(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// AttributeList's positional label and named link properties. The first
/// positional field retains the original semantic fragments instead of parsing
/// the generated reference tags back into a tree.
fn parse_label(
    parts: &[Part],
    attributes: &mut Evaluation<'_>,
    compat: bool,
    depth: usize,
    span: Span,
    report: &mut Report<'_>,
) -> (Vec<Part>, BTreeMap<String, String>, BTreeMap<usize, String>) {
    let mut text = Text::from_parts(parts);
    // AttributeList folds physical newlines before parsing mixed text/attrs.
    let ranges: Vec<_> = text.text.match_indices('\n').map(|(i, _)| i).collect();
    if !ranges.is_empty() {
        let mut out = Text::default();
        let mut copied = 0;
        for i in ranges {
            text.copy(copied..i, &mut out);
            text.replacement(i..i + 1, " ", None, &mut out);
            copied = i + 1;
        }
        text.copy(copied..text.text.len(), &mut out);
        text = out;
    }
    let src = &text.text;
    let mut first = None;
    let mut positional = BTreeMap::new();
    let mut named = BTreeMap::new();
    let fields = crate::attribute_list::parse(src);
    let last = fields.len().saturating_sub(1);
    for (n, field) in fields.iter().enumerate() {
        let name = field.name(src);
        let Some(range) = field.value.clone() else {
            // Ruby stores nil here (and its mailto encoding then fails); keep
            // interior empty fields as empty strings and omit a trailing one.
            if n != last {
                positional.insert(field.index + 1, String::new());
            }
            continue;
        };
        // The macro label already removed one bracket escape. AttributeList
        // only removes escapes for its delimiter quote; nested pass keeps the rest.
        let mut value = match &field.gap {
            Some(gap) if src[gap.clone()].contains('\t') => {
                let mut normalized = Text::default();
                text.copy(range.start..gap.start, &mut normalized);
                text.replacement(gap.clone(), &" ".repeat(gap.len()), None, &mut normalized);
                text.copy(gap.end..range.end, &mut normalized);
                normalized.into_parts()
            }
            _ => slice(&text, range, field.quote, false),
        };
        if field.single_quoted && !matches!(name, Some("title" | "reftext" | "opts" | "options")) {
            // AttributeList repeats the complete normal pipeline on the
            // lexical value, sharing slots and the recursion budget with pass:.
            value = report.passes.normal_parameter(
                crate::attributes::reference_text(&value),
                attributes,
                compat,
                span,
                depth,
            );
        }
        let lexical = crate::attributes::reference_text(&value);
        if let Some(name) = name {
            if matches!(name, "opts" | "options") {
                let options = if lexical.contains(',') {
                    lexical.replace(' ', "")
                } else {
                    lexical
                };
                for option in options.split(',').filter(|s| !s.is_empty()) {
                    named.insert(format!("{option}-option"), String::new());
                }
            } else {
                named.insert(name.into(), lexical);
            }
        } else {
            if field.index == 0 {
                first = Some(value);
            }
            positional.insert(field.index + 1, lexical);
        }
    }
    let first = first.unwrap_or_default();
    if positional.get(&1).is_some_and(|s| s == src) {
        return (parts.to_vec(), BTreeMap::new(), BTreeMap::new());
    }
    (first, named, positional)
}
