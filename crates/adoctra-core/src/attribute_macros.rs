//! Macro substitutions for pass attributes and expanded inline text: the
//! experimental keyboard, button and menu macros, inline images and icons,
//! index terms, links, emails, inline anchors, cross-references and
//! footnotes. Matched over
//! the shared lexical stream; generated constructs remain backend-neutral
//! boundaries.
use crate::inline_pass::Passes;
use crate::substitution_context::Evaluation;
use crate::substitution_text::{Marked, Text};
use crate::{
    AttributeFormat, AttributeLink, AttributePart as Part, IndexTerm, Inline, InlineImage, Span,
    UiMacro,
};
use std::collections::BTreeMap;
use std::ops::Range;

const SCHEMES: [&str; 5] = ["https://", "http://", "file://", "ftp://", "irc://"];

struct Report<'a> {
    ui: Vec<Span>,
    images: Vec<Span>,
    index_terms: Vec<Span>,
    links: Vec<Span>,
    anchors: Vec<Span>,
    xrefs: Vec<Span>,
    /// Physical label range of each cross-reference, keyed by its match range.
    labels: BTreeMap<(usize, usize), Span>,
    /// Labels built in the macros stage, keyed by the reference's match
    /// range: the inserted parts and the label's AST nodes.
    filled: BTreeMap<(usize, usize), (Vec<Part>, Vec<Inline>)>,
    passes: &'a mut Passes,
}

impl<'a> Report<'a> {
    fn new(passes: &'a mut Passes) -> Self {
        Self {
            ui: Vec::new(),
            images: Vec::new(),
            index_terms: Vec::new(),
            links: Vec::new(),
            anchors: Vec::new(),
            xrefs: Vec::new(),
            labels: BTreeMap::new(),
            filled: BTreeMap::new(),
            passes,
        }
    }
}

/// Physical ranges of constructs consumed by the macros stage, for highlighting.
#[derive(Default)]
pub(crate) struct Matches {
    pub ui: Vec<Span>,
    pub images: Vec<Span>,
    pub index_terms: Vec<Span>,
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
        ui: report.ui,
        images: report.images,
        index_terms: report.index_terms,
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
                // An empty close is formatting a macro's markup split before it.
                if stack.last().is_some_and(|(n, _, _)| {
                    format(n) == Some(*kind) && (n.span().end == span.end || span.start == span.end)
                }) {
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
    text = apply_ui(text, attributes, report);
    text = apply_images(text, attributes, report);
    text = apply_index_terms(text, attributes, 0, report);
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
    text = apply_bibliography(text, attributes, report);
    text = apply_anchors(text, report);
    text = apply_xrefs(text, attributes, report);
    text = apply_footnotes(text, attributes, report);
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
        Inline::Substituted { parts, span, .. } if matches!(parts.as_slice(), [Part::Ui(_)]) => {
            let Some(Part::Ui(ui)) = parts.into_iter().next() else {
                unreachable!()
            };
            Inline::Ui { ui: *ui, span }
        }
        Inline::Substituted { parts, span, .. } if matches!(parts.as_slice(), [Part::Image(_)]) => {
            let Some(Part::Image(image)) = parts.into_iter().next() else {
                unreachable!()
            };
            Inline::Image {
                image: *image,
                span,
            }
        }
        Inline::Substituted { parts, span, .. }
            if matches!(parts.as_slice(), [Part::IndexTerm(_)]) =>
        {
            let Some(Part::IndexTerm(term)) = parts.into_iter().next() else {
                unreachable!()
            };
            Inline::IndexTerm { term: *term, span }
        }
        Inline::Substituted { parts, span, .. }
            if matches!(parts.as_slice(), [Part::Footnote(_)]) =>
        {
            let Some(Part::Footnote(footnote)) = parts.into_iter().next() else {
                unreachable!()
            };
            Inline::Footnote {
                footnote: *footnote,
                span,
            }
        }
        Inline::Substituted { parts, span, .. } if single_xref(&parts) => {
            let label = report
                .labels
                .get(&(span.start, span.end))
                .copied()
                .unwrap_or(span);
            let filled = report.filled.remove(&(span.start, span.end));
            xref_node(parts, span, label, filled, fold_newlines)
        }
        node => node,
    }));
}

/// Exactly one generated cross-reference and nothing around it.
fn single_xref(parts: &[Part]) -> bool {
    matches!(
        (parts.first(), parts.last()),
        (Some(Part::OpenXref(_)), Some(Part::CloseXref))
    ) && parts
        .iter()
        .filter(|p| matches!(p, Part::OpenXref(_) | Part::CloseXref))
        .count()
        == 2
}

/// A source-owned reference becomes its AST node. The consumed label keeps
/// its text and semantic boundaries; a textless one stays empty for conversion
/// unless the macros stage built its label.
fn xref_node(
    mut parts: Vec<Part>,
    span: Span,
    label: Span,
    filled: Option<(Vec<Part>, Vec<Inline>)>,
    fold_newlines: bool,
) -> Inline {
    parts.pop();
    let Part::OpenXref(mut xref) = parts.remove(0) else {
        unreachable!()
    };
    xref.map_fields(crate::attributes::decode_special);
    // A label built in the macros stage keeps its nodes unless a later
    // stage changed it.
    xref.label = filled.and_then(|(inserted, nodes)| (inserted == parts).then_some((span, nodes)));
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
        xref,
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
    text = apply_ui(text, attributes, report);
    text = apply_images(text, attributes, report);
    text = apply_index_terms(text, attributes, depth, report);
    for stage in 0..3 {
        text = apply_stage(text, stage, attributes, compat, depth, true, report);
    }
    let text = apply_bibliography(text, attributes, report);
    let text = apply_anchors(text, report);
    let text = apply_xrefs(text, attributes, report);
    apply_footnotes(text, attributes, report).into_parts()
}

/// Ruby's experimental UI macros, the first built-in macros of `sub_macros`
/// and only with `experimental`: `kbd:[…]`/`btn:[…]` (`InlineKbdBtnMacroRx`),
/// then `menu:name[…]` (`InlineMenuMacroRx`), then the `"File > Save"`
/// shorthand (`InlineMenuRx`), each over the previous pass's result (so the
/// shorthand also sees the markup generated before it). An escaped macro
/// loses its backslash.
fn apply_ui(mut text: Text, attributes: &Evaluation<'_>, report: &mut Report<'_>) -> Text {
    if attributes.get("experimental").is_none() {
        return text;
    }
    let font = attributes.get("icons") == Some("font");
    let src = &text.text;
    let macroish = src.contains('[') && src.contains(':');
    if macroish && src.contains(":[") && (src.contains("kbd:") || src.contains("btn:")) {
        text = scan_ui(text, kbd_btn_at, font, report);
    }
    if macroish && text.text.contains("menu:") {
        text = scan_ui(text, menu_at, font, report);
    }
    if text.text.contains('"') && text.text.contains("&gt;") {
        text = scan_ui(text, menu_shorthand_at, font, report);
    }
    text
}

enum UiMatch {
    Keyboard(Range<usize>),
    Button(Range<usize>),
    Menu(Range<usize>, Option<Range<usize>>),
    Shorthand(Range<usize>),
}

fn scan_ui(
    text: Text,
    matcher: fn(&str, usize) -> Option<(usize, UiMatch)>,
    font: bool,
    report: &mut Report<'_>,
) -> Text {
    let src = &text.text;
    let mut output = Text::default();
    let mut copied = 0;
    let mut at = 0;
    while at < src.len() {
        let escaped = src.as_bytes()[at] == b'\\';
        let start = at + usize::from(escaped);
        let Some((end, m)) = matcher(src, start) else {
            at += src[at..].chars().next().unwrap().len_utf8();
            continue;
        };
        text.copy(copied..at, &mut output);
        if escaped {
            text.copy(start..end, &mut output);
        } else {
            report.ui.push(text.match_span(at..end));
            text.replace_ui(at..end, Marked::Ui(ui_macro(&text, m, font)), &mut output);
        }
        copied = end;
        at = end;
    }
    text.copy(copied..src.len(), &mut output);
    output
}

/// Where macro content opened at `open` ends: Ruby's lazy `(CC_ALL*?[^\\])\]`
/// stops at the first `]` after at least one character that does not follow
/// a backslash. Like Ruby we take that `]`, unless it closes a macro nested in
/// the content (`btn:[link:x[y]]`): then the content runs to the bracket that
/// balances it, so the nested macro is matched whole inside the label instead
/// of across its markup.
fn content_end(src: &str, open: usize) -> Option<usize> {
    let closes = |close: usize| close > open && src.as_bytes()[close - 1] != b'\\';
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        let at = open + i;
        match c {
            '[' if nested_macro(&src[open..at]).is_some() => depth += 1,
            ']' if closes(at) && depth > 0 => depth -= 1,
            ']' if closes(at) => return Some(at),
            _ => {}
        }
    }
    // An unbalanced nested macro leaves Ruby's first closing bracket.
    src[open..]
        .match_indices(']')
        .map(|(i, _)| open + i)
        .find(|&close| closes(close))
}

/// Where the inline macro whose `[` follows `before` starts (`name:target[`
/// with a built-in macro name or a URL scheme), or `None` for a literal
/// bracket (`kbd:[[]`, `kbd:[Ctrl+[]`).
fn nested_macro(before: &str) -> Option<usize> {
    const NAMES: [&str; 16] = [
        "link",
        "mailto",
        "image",
        "icon",
        "footnote",
        "footnoteref",
        "xref",
        "anchor",
        "pass",
        "kbd",
        "btn",
        "menu",
        "stem",
        "latexmath",
        "asciimath",
        "indexterm",
    ];
    let token = before
        .rfind(|c: char| blank(c) || c == '[' || c == ']')
        .map_or(0, |i| i + 1);
    let colon = token + before[token..].find(':')?;
    let start = before[..colon]
        .rfind(|c: char| !c.is_ascii_alphanumeric())
        .map_or(0, |i| i + 1)
        .max(token);
    let macro_ = &before[start..];
    (NAMES.contains(&&before[start..colon])
        || SCHEMES
            .iter()
            .any(|scheme| macro_.starts_with(scheme) && macro_.len() > scheme.len()))
    .then_some(start)
}

/// The ranges of `src` covered by inline macros nested in content (from the
/// macro name to its balancing `]`) and by `<<…>>` cross-references, where
/// delimiters do not split labels.
fn nested_regions(src: &str) -> Vec<Range<usize>> {
    let mut regions = Vec::new();
    let mut starts = Vec::new();
    let mut from = 0;
    while let Some(open) = src[from..].find("&lt;&lt;").map(|i| from + i) {
        let Some(close) = src[open..].find("&gt;&gt;").map(|i| open + i) else {
            break;
        };
        from = close + "&gt;&gt;".len();
        regions.push(open..from);
    }
    for (at, c) in src.char_indices() {
        match c {
            '[' => {
                if let Some(start) = nested_macro(&src[..at]) {
                    starts.push(start);
                }
            }
            ']' if at > 0 && src.as_bytes()[at - 1] != b'\\' => {
                if let Some(start) = starts.pop() {
                    if starts.is_empty() {
                        regions.push(start..at + 1);
                    }
                }
            }
            _ => {}
        }
    }
    regions
}

/// `(kbd|btn):\[(CC_ALL*?[^\\])\]` at `start`.
fn kbd_btn_at(src: &str, start: usize) -> Option<(usize, UiMatch)> {
    let rest = &src[start..];
    let keyboard = if rest.starts_with("kbd:[") {
        true
    } else if rest.starts_with("btn:[") {
        false
    } else {
        return None;
    };
    let open = start + "kbd:[".len();
    let close = content_end(src, open)?;
    Some((
        close + 1,
        if keyboard {
            UiMatch::Keyboard(open..close)
        } else {
            UiMatch::Button(open..close)
        },
    ))
}

/// `menu:(CG_WORD|[CC_WORD&][^\n\[]*[^\s\[])\[ *(?:|(CC_ALL*?[^\\]))\]` at
/// `start`: the name runs to the first `[` on its line, starts with a word
/// character or `&` (a lone character must be a word character) and does not
/// end with a blank; spaces after the bracket are skipped.
fn menu_at(src: &str, start: usize) -> Option<(usize, UiMatch)> {
    if !src[start..].starts_with("menu:") {
        return None;
    }
    let name = start + "menu:".len();
    let bracket = name + src[name..].find(['\n', '['])?;
    if src.as_bytes()[bracket] != b'[' {
        return None;
    }
    let first = src[name..bracket].chars().next()?;
    let word = crate::unicode_word::is_word(first);
    if name + first.len_utf8() == bracket {
        if !word {
            return None;
        }
    } else if !(word || first == '&') || src[name..bracket].ends_with(blank) {
        return None;
    }
    let mut open = bracket + 1;
    while src[open..].starts_with(' ') {
        open += 1;
    }
    if src[open..].starts_with(']') {
        return Some((open + 1, UiMatch::Menu(name..bracket, None)));
    }
    let close = content_end(src, open)?;
    Some((close + 1, UiMatch::Menu(name..bracket, Some(open..close))))
}

/// `"([CC_WORD&][^"]*?[ \n]+&gt;[ \n]+[^"]*)"` at `start`: quoted text without
/// quotes, starting with a word character or `&`, with a `>` between blanks
/// after its first character.
fn menu_shorthand_at(src: &str, start: usize) -> Option<(usize, UiMatch)> {
    if !src[start..].starts_with('"') {
        return None;
    }
    let open = start + 1;
    let first = src[open..].chars().next()?;
    if !(crate::unicode_word::is_word(first) || first == '&') {
        return None;
    }
    let close = open + src[open..].find('"')?;
    let content = &src[open..close];
    let blank = |i: usize| matches!(content.as_bytes().get(i), Some(b' ' | b'\n'));
    content
        .match_indices("&gt;")
        .any(|(i, _)| i > first.len_utf8() && blank(i - 1) && blank(i + "&gt;".len()))
        .then_some((close + 1, UiMatch::Shorthand(open..close)))
}

/// Ruby's `String#strip` (or `rstrip`) of a range of `src`.
fn strip(src: &str, range: Range<usize>, left: bool) -> Range<usize> {
    let value = &src[range.clone()];
    let start = if left {
        range.start + value.len() - value.trim_start_matches(|c| blank(c) || c == '\0').len()
    } else {
        range.start
    };
    let end = range.start + value.trim_end_matches(|c| blank(c) || c == '\0').len();
    start..end.max(start)
}

/// Ruby's `String#split(delimiter)` of a range: trailing empty pieces are
/// dropped unless `keep_empty` (Ruby's `-1` limit). Delimiters inside a
/// nested macro do not split.
fn split(src: &str, range: Range<usize>, delimiter: &str, keep_empty: bool) -> Vec<Range<usize>> {
    let regions = nested_regions(src);
    let mut pieces = Vec::new();
    let mut from = range.start;
    for (i, _) in src[range.clone()].match_indices(delimiter) {
        let at = range.start + i;
        if at < from || regions.iter().any(|r| r.contains(&at)) {
            continue;
        }
        pieces.push(from..at);
        from = at + delimiter.len();
    }
    pieces.push(from..range.end);
    if !keep_empty {
        while pieces.last().is_some_and(|piece| piece.is_empty()) {
            pieces.pop();
        }
    }
    pieces
}

fn ui_macro(text: &Text, m: UiMatch, font: bool) -> UiMacro {
    let parts = |text: &Text, range: Range<usize>| {
        let mut piece = Text::default();
        text.copy(range, &mut piece);
        piece.into_parts()
    };
    let unescaped = |range| Text::from_parts(&slice(text, range, None, true));
    match m {
        UiMatch::Keyboard(range) => {
            let keys = unescaped(range);
            let src = &keys.text;
            let range = strip(src, 0..src.len(), true);
            let value = &src[range.clone()];
            let second = value.chars().next().map_or(0, char::len_utf8);
            let regions = nested_regions(src);
            let position = |c: char| {
                value[second..]
                    .match_indices(c)
                    .map(|(i, _)| range.start + second + i)
                    .find(|at| !regions.iter().any(|r| r.contains(at)))
            };
            let delimiter = match (position(','), position('+')) {
                _ if value.chars().nth(1).is_none() => None,
                (Some(comma), plus) => Some(plus.map_or(comma, |plus| plus.min(comma))),
                (None, plus) => plus,
            };
            let Some(at) = delimiter else {
                return UiMacro::Keyboard {
                    keys: vec![parts(&keys, range)],
                };
            };
            let delimiter = &src[at..at + 1];
            let trailing = value.ends_with(delimiter);
            let body = range.start..range.end - usize::from(trailing);
            let mut list: Vec<Vec<Part>> = split(src, body, delimiter, trailing)
                .into_iter()
                .map(|piece| parts(&keys, strip(src, piece, true)))
                .collect();
            if trailing {
                if let Some(last) = list.last_mut() {
                    crate::attributes::push_part(last, delimiter, false);
                }
            }
            UiMacro::Keyboard { keys: list }
        }
        UiMatch::Button(range) => UiMacro::Button {
            label: normalize_text(slice(text, range, None, true)),
        },
        UiMatch::Menu(name, items) => {
            let menu = parts(text, name);
            let (submenus, item) = match items {
                None => (Vec::new(), None),
                Some(items) => {
                    let items = unescaped(items);
                    let src = &items.text;
                    let regions = nested_regions(src);
                    let outside = |delimiter: &str| {
                        src.match_indices(delimiter)
                            .any(|(at, _)| !regions.iter().any(|r| r.contains(&at)))
                    };
                    let delimiter = if outside("&gt;") {
                        Some("&gt;")
                    } else if outside(",") {
                        Some(",")
                    } else {
                        None
                    };
                    match delimiter {
                        Some(delimiter) => {
                            let mut list: Vec<Vec<Part>> =
                                split(src, 0..src.len(), delimiter, false)
                                    .into_iter()
                                    .map(|piece| parts(&items, strip(src, piece, true)))
                                    .collect();
                            let item = list.pop();
                            (list, item)
                        }
                        None => (
                            Vec::new(),
                            Some(parts(&items, strip(src, 0..src.len(), false))),
                        ),
                    }
                }
            };
            UiMacro::Menu {
                menu,
                submenus,
                item,
                font,
            }
        }
        UiMatch::Shorthand(range) => {
            let src = &text.text;
            let mut list = split(src, range, "&gt;", false)
                .into_iter()
                .map(|piece| parts(text, strip(src, piece, true)));
            let menu = list.next().unwrap_or_default();
            let mut submenus: Vec<Vec<Part>> = list.collect();
            let item = submenus.pop();
            UiMacro::Menu {
                menu,
                submenus,
                item,
                font,
            }
        }
    }
}

/// Ruby's `InlineImageMacroRx`, the first built-in macro (before links) over
/// the lexical result of the earlier stages: `image:{dir}/a.png[]` takes the
/// expanded target. An escaped macro loses its backslash.
fn apply_images(text: Text, attributes: &mut Evaluation<'_>, report: &mut Report<'_>) -> Text {
    let src = &text.text;
    if !src.contains('[') || !(src.contains("image:") || src.contains("icon:")) {
        return text;
    }
    let mut output = Text::default();
    let mut copied = 0;
    let mut at = 0;
    while at < src.len() {
        let escaped = src.as_bytes()[at] == b'\\';
        let start = at + usize::from(escaped);
        let Some(m) = image_at(src, start) else {
            at += src[at..].chars().next().unwrap().len_utf8();
            continue;
        };
        text.copy(copied..at, &mut output);
        if escaped {
            text.copy(start..m.end, &mut output);
        } else {
            report.images.push(text.match_span(at..m.end));
            let span = text.match_span(at..m.end);
            let image = inline_image(&text, &m, attributes, span, &mut report.passes.diagnostics);
            text.replace_parts(at..m.end, &[Part::Image(Box::new(image))], &mut output);
        }
        copied = m.end;
        at = m.end;
    }
    text.copy(copied..src.len(), &mut output);
    output
}

struct ImageMatch {
    end: usize,
    icon: bool,
    target: Range<usize>,
    list: Range<usize>,
}

/// Ruby's `\s`.
fn blank(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// `i(?:mage|con):([^:\s\[](?:[^\n\[]*[^\s\[])?)\[(|.*?[^\\])\]` at `start`:
/// the target runs to the first `[` on its line and neither starts with a
/// colon or blank nor ends with a blank; the list ends at the first `]` not
/// preceded by a backslash (it may span lines).
fn image_at(src: &str, start: usize) -> Option<ImageMatch> {
    let rest = &src[start..];
    let (icon, prefix) = if rest.starts_with("image:") {
        (false, "image:".len())
    } else if rest.starts_with("icon:") {
        (true, "icon:".len())
    } else {
        return None;
    };
    let target = start + prefix;
    let first = src[target..].chars().next()?;
    if first == ':' || first == '[' || blank(first) {
        return None;
    }
    let bracket = target + src[target..].find(['\n', '['])?;
    if src.as_bytes()[bracket] != b'[' || src[..bracket].ends_with(blank) {
        return None;
    }
    let open = bracket + 1;
    let close = src[open..]
        .match_indices(']')
        .map(|(i, _)| open + i)
        .find(|&close| close == open || src.as_bytes()[close - 1] != b'\\')?;
    Some(ImageMatch {
        end: close + 1,
        icon,
        target: target..bracket,
        list: open..close,
    })
}

/// The `:image` inline node of Ruby's `sub_macros`: the list is parsed with
/// `posattrs` after `normalize_text` (strip, newlines as spaces, `\]`
/// unescaped) and without value substitutions; a missing alt is the target's
/// basename. Sources use the document attributes at this point.
fn inline_image(
    text: &Text,
    m: &ImageMatch,
    attributes: &Evaluation<'_>,
    span: crate::Span,
    diagnostics: &mut Vec<crate::Diagnostic>,
) -> InlineImage {
    let target = text.text[m.target.clone()].to_owned();
    let posattrs: &[&str] = if m.icon {
        &["size"]
    } else {
        &["alt", "width", "height"]
    };
    let (alt, named, options) = image_attributes(text, m.list.clone(), posattrs);
    let alt = alt.unwrap_or_else(|| {
        let basename = crate::image::default_alt(&target);
        crate::entity_references::reference_parts(&crate::attributes::decode_special(&basename))
    });
    let icons = m
        .icon
        .then(|| attributes.get("icons").map(str::to_owned))
        .flatten();
    // Ruby reads an inline SVG instead of resolving its source.
    let svg = if m.icon {
        None
    } else {
        attributes.svg(&target, &named, &options, span, diagnostics)
    };
    let src = if matches!(svg, Some(crate::Svg::Inline(_))) {
        crate::image::web_path(&target, attributes.get("imagesdir"))
    } else if !m.icon {
        attributes.image_uri(&target, attributes.get("imagesdir"), span, diagnostics)
    } else if icons.as_deref().is_some_and(|icons| icons != "font") {
        let icon = crate::image::icon_target(
            named.get("icon").map(String::as_str),
            &target,
            attributes.get("icontype"),
        );
        attributes.image_uri(&icon, attributes.get("iconsdir"), span, diagnostics)
    } else {
        String::new()
    };
    InlineImage {
        icon: m.icon,
        target,
        src,
        alt,
        icons,
        attributes: named,
        options,
        svg,
    }
}

/// `parse_attributes(…, posattrs, unescape_input: true)`: the alt keeps its
/// lexical parts, other values their lexical spelling; options are split.
fn image_attributes(
    text: &Text,
    list: Range<usize>,
    posattrs: &[&str],
) -> (Option<Vec<Part>>, BTreeMap<String, String>, Vec<String>) {
    let mut alt = None;
    let mut named = BTreeMap::new();
    let mut options = Vec::new();
    let unescaped = Text::from_parts(&slice(text, list, None, true));
    // `normalize_text`: strip, then line feeds become spaces.
    let src = &unescaped.text;
    let start = src.len() - src.trim_start_matches(blank).len();
    let end = src
        .trim_end_matches(|c| blank(c) || c == '\0')
        .len()
        .max(start);
    let mut text = Text::default();
    let mut copied = start;
    for (i, _) in src[start..end].match_indices('\n') {
        unescaped.copy(copied..start + i, &mut text);
        unescaped.replacement(start + i..start + i + 1, " ", None, &mut text);
        copied = start + i + 1;
    }
    unescaped.copy(copied..end, &mut text);
    let src = &text.text;
    for field in crate::attribute_list::parse(src) {
        let Some(range) = field.value.clone() else {
            continue;
        };
        let value = match &field.gap {
            Some(gap) => {
                let mut normalized = Text::default();
                text.copy(range.start..gap.start, &mut normalized);
                text.replacement(gap.clone(), &" ".repeat(gap.len()), None, &mut normalized);
                text.copy(gap.end..range.end, &mut normalized);
                normalized.into_parts()
            }
            None => slice(&text, range, field.quote, false),
        };
        let name = match field.name(src) {
            Some(name) => name,
            None => match posattrs.get(field.index) {
                Some(name) => name,
                None => continue,
            },
        };
        let lexical = crate::attributes::reference_text(&value);
        match name {
            "opts" | "options" => {
                let lexical = if lexical.contains(',') {
                    lexical.replace(' ', "")
                } else {
                    lexical
                };
                for option in lexical.split(',').filter(|s| !s.is_empty()) {
                    if !options.iter().any(|o| o == option) {
                        options.push(option.to_owned());
                    }
                }
            }
            "alt" => alt = Some(value),
            _ => {
                named.insert(name.to_owned(), lexical);
            }
        }
    }
    (alt, named, options)
}

/// Ruby's `InlineIndextermMacroRx` after images, over the lexical result of
/// the earlier stages. A flow term (`((Tigers))`, `indexterm2:[Tigers]`)
/// keeps its text between empty marks, so the later macros see Ruby's text
/// and may match inside it; a concealed term (`(((Big cats,Tigers)))`,
/// `indexterm:[…]`) becomes an atomic value with an empty spelling. An
/// escaped macro loses its backslash; an escaped concealed term
/// (`\(((Tigers)))`) is a flow term between literal parentheses.
fn apply_index_terms(
    text: Text,
    attributes: &mut Evaluation<'_>,
    depth: usize,
    report: &mut Report<'_>,
) -> Text {
    let src = &text.text;
    if !(src.contains("((") && src.contains("))"))
        && !(src.contains(":[") && src.contains("dexterm"))
    {
        return text;
    }
    let mut output = Text::default();
    let mut copied = 0;
    let mut at = 0;
    while at < src.len() {
        let escaped = src.as_bytes()[at] == b'\\';
        let start = at + usize::from(escaped);
        let Some(m) = index_term_at(src, start) else {
            at += src[at..].chars().next().unwrap().len_utf8();
            continue;
        };
        text.copy(copied..at, &mut output);
        copied = m.end;
        let content = m.content.clone();
        let (term, before, after) = match m.kind {
            TermKind::Macro { .. } if escaped => {
                text.copy(start..m.end, &mut output);
                at = m.end;
                continue;
            }
            TermKind::Macro { visible } => (
                macro_term(&text, content, visible, attributes, depth, report),
                None,
                None,
            ),
            TermKind::Parens => {
                let opens = src[content.clone()].starts_with('(');
                let closes = src[content.clone()].ends_with(')');
                let open = content.start..content.start + 1;
                let close = content.end.saturating_sub(1).max(content.start)..content.end;
                let (content, visible, before, after) = match (escaped, opens, closes) {
                    // Only a nested flow term is processed inside an escape.
                    (true, true, true) if content.len() >= 2 => {
                        (open.end..close.start, true, Some(open), Some(close))
                    }
                    (true, ..) => {
                        text.copy(start..m.end, &mut output);
                        at = m.end;
                        continue;
                    }
                    (false, true, true) if content.len() >= 2 => {
                        (open.end..close.start, false, None, None)
                    }
                    (false, true, _) => (open.end..content.end, true, Some(open), None),
                    (false, false, true) => (content.start..close.start, true, None, Some(close)),
                    (false, false, false) => (content, true, None, None),
                };
                (parens_term(&text, content, visible), before, after)
            }
        };
        // An escape processed as a flow term is part of the construct.
        report.index_terms.push(text.match_span(at..m.end));
        if let Some(before) = before {
            text.copy(before, &mut output);
        }
        if term.text.is_some() {
            text.replace_ui(at..m.end, Marked::IndexTerm(term), &mut output);
        } else {
            text.replace_parts(at..m.end, &[Part::IndexTerm(Box::new(term))], &mut output);
        }
        if let Some(after) = after {
            text.copy(after, &mut output);
        }
        at = m.end;
    }
    text.copy(copied..src.len(), &mut output);
    output
}

struct TermMatch {
    end: usize,
    kind: TermKind,
    content: Range<usize>,
}

enum TermKind {
    /// `indexterm:[…]` (concealed) or `indexterm2:[…]` (visible).
    Macro { visible: bool },
    /// `((…))`, the enclosed text deciding the kind.
    Parens,
}

/// `(indexterm2?):\[(CC_ALL*?[^\\])\]|\(\((CC_ALL+?)\)\)(?!\))` at
/// `start`: the macro's content is not empty and ends at the first `]` not
/// preceded by a backslash; the enclosed text is the shortest one followed
/// by `))` but not by a third `)`. Both may span lines.
fn index_term_at(src: &str, start: usize) -> Option<TermMatch> {
    let rest = &src[start..];
    for (prefix, visible) in [("indexterm:[", false), ("indexterm2:[", true)] {
        if rest.starts_with(prefix) {
            let open = start + prefix.len();
            let close = src[open..]
                .match_indices(']')
                .map(|(i, _)| open + i)
                .find(|&close| close > open && src.as_bytes()[close - 1] != b'\\')?;
            return Some(TermMatch {
                end: close + 1,
                kind: TermKind::Macro { visible },
                content: open..close,
            });
        }
    }
    let open = start + rest.strip_prefix("((").map(|_| 2)?;
    let first = src[open..].chars().next()?.len_utf8();
    // Overlapping candidates: in `x)))` the first `))` is followed by `)`.
    let close = src[open + first..]
        .match_indices(')')
        .map(|(i, _)| open + first + i)
        .find(|&close| src[close..].starts_with("))") && !src[close + 2..].starts_with(')'))?;
    Some(TermMatch {
        end: close + 2,
        kind: TermKind::Parens,
        content: open..close,
    })
}

/// Ruby's `normalize_text` with whitespace normalization (and `\]`
/// unescaped for the macro forms) over a range, as a new lexical text.
fn normalized(text: &Text, range: Range<usize>, brackets: bool) -> Text {
    // Ruby's reader removes CR in CRLF before substitutions. Source fragments
    // keep those bytes for spans, so normalize them in the term's value only.
    let parts = slice(text, range, None, brackets)
        .into_iter()
        .map(|part| match part {
            Part::Text(value) => Part::Text(value.replace("\r\n", "\n")),
            Part::Raw(value) => Part::Raw(value.replace("\r\n", "\n")),
            part => part,
        })
        .collect();
    let parts = normalize_text(parts);
    Text::from_parts(&parts)
}

/// A range of a lexical text as parts, line feeds as spaces.
fn term_parts(text: &Text, range: Range<usize>) -> Vec<Part> {
    let mut selected = Text::default();
    text.copy(range, &mut selected);
    selected
        .into_parts()
        .into_iter()
        .map(|part| match part {
            Part::Text(t) => Part::Text(t.replace('\n', " ")),
            Part::Raw(t) => Part::Raw(t.replace('\n', " ")),
            part => part,
        })
        .collect()
}

/// `((Tigers))` or `(((Big cats,Tigers)))`: ` >> ` names a `see` target,
/// ` &> ` separates `see-also` targets (in their escaped spelling).
fn parens_term(text: &Text, range: Range<usize>, visible: bool) -> IndexTerm {
    let normalized = normalized(text, range, false);
    let src = normalized.text.as_str();
    let mut end = src.len();
    let mut see = None;
    let mut see_also = Vec::new();
    if src.contains(";&") {
        if let Some(at) = src.find(" &gt;&gt; ") {
            end = at;
            see = Some(src[at + " &gt;&gt; ".len()..].to_owned());
        } else if src.contains(" &amp;&gt; ") {
            let mut pieces = ruby_split(src, " &amp;&gt; ");
            end = pieces.first().map_or(0, |p| p.len());
            pieces.remove(0);
            see_also = pieces.into_iter().map(str::to_owned).collect();
        }
    }
    if visible {
        IndexTerm {
            text: Some(term_parts(&normalized, 0..end)),
            terms: Vec::new(),
            see,
            see_also,
        }
    } else {
        IndexTerm {
            text: None,
            terms: split_simple_csv(&src[..end]),
            see,
            see_also,
        }
    }
}

/// `indexterm:[Big cats,Tigers]` or `indexterm2:[Tigers]`: with a `=` the
/// content is an attribute list (positional terms, `see-also`), else a
/// simple CSV of terms or the term itself.
fn macro_term(
    text: &Text,
    range: Range<usize>,
    visible: bool,
    attributes: &mut Evaluation<'_>,
    depth: usize,
    report: &mut Report<'_>,
) -> IndexTerm {
    let span = text.match_span(range.clone());
    let normalized = normalized(text, range, true);
    let src = normalized.text.as_str();
    let mut term = IndexTerm {
        text: None,
        terms: Vec::new(),
        see: None,
        see_also: Vec::new(),
    };
    let mut positional = BTreeMap::new();
    let mut named = BTreeMap::new();
    if src.contains('=') {
        for field in crate::attribute_list::parse(src) {
            let Some(range) = field.value.clone() else {
                continue;
            };
            let name = field.name(src);
            let mut value = match &field.gap {
                Some(gap) if src[gap.clone()].contains('\t') => {
                    let mut value = Text::default();
                    normalized.copy(range.start..gap.start, &mut value);
                    normalized.replacement(gap.clone(), &" ".repeat(gap.len()), None, &mut value);
                    normalized.copy(gap.end..range.end, &mut value);
                    value.into_parts()
                }
                _ => slice(&normalized, range, field.quote, false),
            };
            // AttributeList repeats normal substitutions for single-quoted
            // fields, even unused ones, with the shared state and depth limit.
            if field.single_quoted
                && !matches!(name, Some("title" | "reftext" | "opts" | "options"))
            {
                value = report.passes.normal_parameter(
                    crate::attributes::reference_text(&value),
                    attributes,
                    attributes.get("compat-mode").is_some(),
                    span,
                    depth,
                );
            }
            if let Some(name) = name {
                named.insert(name.to_owned(), crate::attributes::reference_text(&value));
            } else {
                positional.insert(field.index, value);
            }
        }
    }
    if let Some(first) = positional.get(&0) {
        term.see = named.remove("see");
        term.see_also = named
            .remove("see-also")
            .map(|value| {
                if value.contains(',') {
                    ruby_split(&value, ",")
                        .into_iter()
                        .map(|v| v.trim_start_matches(|c| blank(c) || c == '\0').to_owned())
                        .collect()
                } else {
                    vec![value]
                }
            })
            .unwrap_or_default();
        if visible {
            term.text = Some(first.clone());
        } else {
            term.terms = (0..3)
                .map_while(|index| positional.get(&index))
                .map(|parts| crate::attributes::reference_text(parts))
                .collect();
        }
    } else if visible {
        term.text = Some(term_parts(&normalized, 0..src.len()));
    } else if src.contains('=') {
        term.terms = vec![src.to_owned()];
    } else {
        term.terms = split_simple_csv(src);
    }
    term
}

/// Ruby's `split_simple_csv`: commas separate stripped values, except
/// inside double quotes (which are removed).
fn split_simple_csv(src: &str) -> Vec<String> {
    if src.is_empty() {
        return Vec::new();
    }
    if !src.contains('"') {
        return ruby_split(src, ",")
            .into_iter()
            .map(|v| ruby_strip(v).to_owned())
            .collect();
    }
    let mut values = Vec::new();
    let mut accum = String::new();
    let mut quoted = false;
    for c in src.chars() {
        match c {
            ',' if !quoted => values.push(ruby_strip(&std::mem::take(&mut accum)).to_owned()),
            '"' => quoted = !quoted,
            c => accum.push(c),
        }
    }
    values.push(ruby_strip(&accum).to_owned());
    values
}

/// Ruby's `String#strip` (whitespace or NUL on either side).
fn ruby_strip(value: &str) -> &str {
    value.trim_matches(|c| blank(c) || c == '\0')
}

/// Ruby's `String#split` with a string: trailing empty fields are dropped.
fn ruby_split<'a>(src: &'a str, separator: &str) -> Vec<&'a str> {
    let mut pieces: Vec<&str> = src.split(separator).collect();
    while pieces.last().is_some_and(|p| p.is_empty()) {
        pieces.pop();
    }
    pieces
}

/// Ruby's first `InlineBiblioAnchorRx` match in a bibliography list item.
/// Registration is separate: generated anchors render without joining refs.
fn apply_bibliography(
    text: Text,
    attributes: &mut Evaluation<'_>,
    report: &mut Report<'_>,
) -> Text {
    if !attributes.bibliography {
        return text;
    }
    let found = std::iter::once(0)
        .chain(text.text.match_indices('\n').map(|(at, _)| at + 1))
        .find_map(|at| crate::anchor_catalog::bibliography_at(&text.text, at));
    let Some(found) = found else { return text };
    let mut output = Text::default();
    text.copy(0..found.start, &mut output);
    let span = text.match_span(found.start..found.end);
    report.anchors.push(span);
    let mut parts = vec![
        Part::Anchor(text.text[found.id.clone()].to_owned()),
        Part::Text("[".into()),
    ];
    if let Some(label) = found.reftext {
        // Inline#reftext applies c/q/r again to the already substituted label.
        // Consequently generated formatting tags become visible escaped text.
        use crate::pass_subs::Stage::{Quotes, Replacements, Specials};
        parts.extend(Passes::selected_stages(
            &text.text[label],
            &[Specials, Quotes, Replacements],
            None,
            attributes,
            attributes.get("compat-mode").is_some(),
            span,
            &mut report.passes.diagnostics,
        ));
    } else {
        parts.push(Part::Text(text.text[found.id].to_owned()));
    }
    parts.push(Part::Text("]".into()));
    text.replace_parts(found.start..found.end, &parts, &mut output);
    text.copy(found.end..text.text.len(), &mut output);
    output
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
            let mut parts = xref_parts(&text, &m, attributes, compat, span, report);
            // At conversion the label is built here, like Ruby's xref text:
            // later macros of the field (footnotes) see it.
            let textless = parts.len() == 2;
            if let Some(Part::OpenXref(xref)) = parts.first_mut() {
                if let Some(label) = attributes.label(xref, textless, span) {
                    xref.resolved = true;
                    if let Some(mut nodes) = label.filter(|_| textless) {
                        crate::resolve::strip_label(&mut nodes);
                        let inserted = crate::resolve::label_parts(&nodes);
                        parts.splice(1..1, inserted.clone());
                        report
                            .filled
                            .insert((span.start, span.end), (inserted, nodes));
                    }
                }
            }
            text.replace_parts(at..m.end, &parts, &mut output);
        }
        copied = m.end;
        at = m.end;
    }
    text.copy(copied..src.len(), &mut output);
    output
}

/// Ruby's `InlineFootnoteMacroRx`, after cross-references: `footnote:[text]`,
/// `footnote:id[text]`, `footnote:id[]` and the legacy `footnoteref:[id,text]`
/// / `footnoteref:[id]`, over the lexical result of the earlier stages (the
/// content keeps their output). A definition takes the next `footnote-number`
/// and registers in the document; a known id is a reference, an unknown one
/// without text warns. An escaped macro loses its backslash; an empty one,
/// or any without a footnote registry, stays text.
fn apply_footnotes(text: Text, attributes: &mut Evaluation<'_>, report: &mut Report<'_>) -> Text {
    let src = &text.text;
    if !src.contains("tnote") || !src.contains('[') || attributes.footnotes().is_none() {
        return text;
    }
    let compat = attributes.get("compat-mode").is_some();
    let mut output = Text::default();
    let mut copied = 0;
    let mut at = 0;
    while at < src.len() {
        let escaped = src.as_bytes()[at] == b'\\';
        let start = at + usize::from(escaped);
        let Some(m) = footnote_at(src, start) else {
            at += src[at..].chars().next().unwrap().len_utf8();
            continue;
        };
        let content = m.content.clone().map(|r| slice(&text, r, None, true));
        let (id, content) = match (m.legacy, content) {
            (true, None) => (None, None),
            (true, Some(content)) => {
                let lexical = crate::attributes::reference_text(&content);
                let (id, rest) = match lexical.split_once(',') {
                    Some((id, _)) => {
                        let rest = m.content.clone().map(|r| {
                            let comma = r.start + text.text[r.clone()].find(',').unwrap() + 1;
                            slice(&text, comma..r.end, None, true)
                        });
                        (id.to_owned(), rest)
                    }
                    None => (lexical, None),
                };
                (Some(crate::attributes::decode_special(&id)), rest)
            }
            (false, content) => (m.id.clone().map(|r| src[r].to_owned()), content),
        };
        if escaped || (id.is_none() && content.is_none()) {
            text.copy(copied..at, &mut output);
            text.copy(start..m.end, &mut output);
            copied = m.end;
            at = m.end;
            continue;
        }
        text.copy(copied..at, &mut output);
        let span = text.match_span(at..m.end);
        if m.legacy && !compat {
            report.passes.diagnostics.push(crate::Diagnostic {
                span,
                severity: crate::Severity::Warning,
                message: format!(
                    "found deprecated footnoteref macro: {}; use footnote macro with target instead",
                    crate::attributes::decode_special(&src[start..m.end])
                ),
            });
        }
        let known = id.as_deref().and_then(|id| {
            attributes
                .footnotes()
                .and_then(|f| f.find(id).map(str::to_owned))
        });
        let footnote = match (known, content) {
            (Some(index), _) => crate::Footnote {
                kind: crate::FootnoteKind::Reference,
                index: Some(index),
                id: None,
                content: Vec::new(),
                sequence: None,
            },
            (None, Some(content)) => {
                let effect = attributes.execute(
                    "counter:footnote-number",
                    span,
                    &mut None,
                    &mut report.passes.diagnostics,
                );
                let index = crate::attributes::parts_text(&effect.parts);
                let sequence = attributes
                    .footnotes()
                    .map(|f| f.register(id.as_deref(), &index));
                crate::Footnote {
                    kind: crate::FootnoteKind::Definition,
                    index: Some(index),
                    id,
                    content: normalize_text(content),
                    sequence,
                }
            }
            (None, None) => {
                report.passes.diagnostics.push(crate::Diagnostic {
                    span,
                    severity: crate::Severity::Warning,
                    message: format!(
                        "invalid footnote reference: {}",
                        id.as_deref().unwrap_or_default()
                    ),
                });
                crate::Footnote {
                    kind: crate::FootnoteKind::Unresolved,
                    index: None,
                    id,
                    content: Vec::new(),
                    sequence: None,
                }
            }
        };
        text.replace_parts(
            at..m.end,
            &[Part::Footnote(Box::new(footnote))],
            &mut output,
        );
        copied = m.end;
        at = m.end;
    }
    text.copy(copied..src.len(), &mut output);
    output
}

/// Ruby's `normalize_text` with whitespace normalization: strip the edges
/// (up to a generated boundary) and turn line feeds into spaces.
fn normalize_text(parts: Vec<Part>) -> Vec<Part> {
    let mut parts: Vec<Part> = parts
        .into_iter()
        .map(|part| match part {
            Part::Text(t) => Part::Text(t.replace('\n', " ")),
            Part::Raw(t) => Part::Raw(t.replace('\n', " ")),
            part => part,
        })
        .collect();
    let space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r' | '\0');
    while let Some(Part::Text(t) | Part::Raw(t)) = parts.first_mut() {
        *t = t.trim_start_matches(space).to_owned();
        if !t.is_empty() {
            break;
        }
        parts.remove(0);
    }
    while let Some(Part::Text(t) | Part::Raw(t)) = parts.last_mut() {
        *t = t.trim_end_matches(space).to_owned();
        if !t.is_empty() {
            break;
        }
        parts.pop();
    }
    parts
}

struct FootnoteMatch {
    end: usize,
    legacy: bool,
    id: Option<Range<usize>>,
    content: Option<Range<usize>>,
}

/// `footnote(?:(ref):|:([\w-]+)?)\[(?:|(.*?[^\\]))\](?!</a>)`, across lines.
fn footnote_at(src: &str, start: usize) -> Option<FootnoteMatch> {
    let rest = src[start..].strip_prefix("footnote")?;
    let after = start + 8;
    let (legacy, id, bracket) = if rest.starts_with("ref:") {
        (true, None, after + 4)
    } else {
        let id_start = after + rest.strip_prefix(':').map(|_| 1)?;
        let len: usize = src[id_start..]
            .chars()
            .take_while(|&c| word(c) || c == '-')
            .map(char::len_utf8)
            .sum();
        (
            false,
            (len > 0).then_some(id_start..id_start + len),
            id_start + len,
        )
    };
    if !src[bracket..].starts_with('[') {
        return None;
    }
    // Not the end of a generated link label.
    let closes = |i: usize| !src[i + 1..].starts_with("</a>");
    if src[bracket + 1..].starts_with(']') && closes(bracket + 1) {
        return Some(FootnoteMatch {
            end: bracket + 2,
            legacy,
            id,
            content: None,
        });
    }
    let first = src[bracket + 1..].chars().next()?.len_utf8();
    let close = src[bracket + 1 + first..]
        .match_indices(']')
        .map(|(i, _)| bracket + 1 + first + i)
        .find(|&i| src.as_bytes()[i - 1] != b'\\' && closes(i))?;
    Some(FootnoteMatch {
        end: close + 1,
        legacy,
        id,
        content: Some(bracket + 1..close),
    })
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
/// fragment of this document and `path#id` another document (see
/// xref_target.rs); the macro form unescapes `\]` and, with an `=`, reads its
/// label, role and link constraints from an AttributeList like Ruby's
/// `extract_attributes_from_text`. A natural (title) reference keeps its
/// refid until conversion looks it up.
fn xref_parts(
    text: &Text,
    m: &XrefMatch,
    attributes: &mut Evaluation<'_>,
    compat: bool,
    span: Span,
    report: &mut Report<'_>,
) -> Vec<Part> {
    let refid = crate::attributes::reference_text(&slice(text, m.refid.clone(), None, false));
    let mut xref =
        crate::xref_target::resolve(&refid, m.macro_form, compat, attributes, attributes.xrefs());
    let mut label = m
        .label
        .clone()
        .map_or_else(Vec::new, |r| slice(text, r, None, m.macro_form));
    if m.macro_form && !compat && crate::attributes::reference_text(&label).contains('=') {
        let (first, mut named, _) = parse_label(&label, attributes, compat, 0, span, report);
        label = first;
        xref.role = named.remove("role");
        xref.xrefstyle = named.remove("xrefstyle");
        // Ruby's append_link_constraint_attrs, for interdocument references.
        if xref.path.is_some() {
            let nofollow = named.contains_key("nofollow-option");
            xref.window = named.remove("window");
            xref.rel = match &xref.window {
                Some(window) => {
                    (window == "_blank" || named.contains_key("noopener-option")).then(|| {
                        if nofollow {
                            "nofollow noopener"
                        } else {
                            "noopener"
                        }
                        .into()
                    })
                }
                None => nofollow.then(|| "nofollow".into()),
            };
        }
    }
    if let Some(range) = m.label.clone() {
        report
            .labels
            .insert((span.start, span.end), text.cover_span(range));
    }
    let mut parts = vec![Part::OpenXref(xref)];
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

#[cfg(test)]
mod tests {
    use super::*;

    fn found(src: &str) -> Option<(bool, &str, &str, usize)> {
        let m = image_at(src, 0)?;
        Some((m.icon, &src[m.target], &src[m.list], m.end))
    }

    #[test]
    fn image_macro_grammar_follows_inline_image_macro_rx() {
        assert_eq!(found("image:a b.png[x]"), Some((false, "a b.png", "x", 16)));
        assert_eq!(found("icon:heart[]"), Some((true, "heart", "", 12)));
        // The list ends at the first unescaped bracket and may span lines.
        assert_eq!(found("image:a[x\\]y]z]"), Some((false, "a", "x\\]y", 13)));
        assert_eq!(found("image:a[x[1]]"), Some((false, "a", "x[1", 12)));
        assert_eq!(found("image:a[x\ny]"), Some((false, "a", "x\ny", 12)));
        assert_eq!(found("image:a[\\]"), None);
        for src in [
            "image: a[]",
            "image::a[]",
            "image:[x]",
            "image:a [x]",
            "image:a\nb[x]",
            "image:a",
            "img:a[]",
        ] {
            assert!(found(src).is_none(), "{src}");
        }
    }
}
