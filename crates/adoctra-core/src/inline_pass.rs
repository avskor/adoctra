//! Extract protected inline regions before quotes and restore their semantic
//! parts only after the outer substitutions finish. Slots are private and
//! collision-free for this source and its attribute context.
use crate::attributes::{parts_text, push_part, push_value, reference_text};
use crate::pass_subs::{self, Stage};
use crate::substitution_context::Evaluation;
use crate::substitution_text::Text;
use crate::{AttributeFormat as F, AttributePart as P, Inline, Span, Token, TokenKind};

struct Slot {
    parts: Vec<P>,
    span: Span,
    mono: bool,
    stem: Option<crate::StemStyle>,
    pending: Option<(String, Vec<Stage>, Option<P>)>,
}

pub(crate) struct Passes {
    prefix: String,
    slots: Vec<Slot>,
    token_slots: usize,
    pub diagnostics: Vec<crate::Diagnostic>,
    /// The block's `line-comment` for its callouts stage.
    pub line_comment: Option<String>,
}

struct Match {
    start: usize,
    open: usize,
    inner: usize,
    close: usize,
    end: usize,
    metadata: Option<String>,
    escaped: usize,
    escaped_metadata: bool,
    subs: String,
    mono: bool,
    macro_form: bool,
}

fn word(c: char) -> bool {
    crate::unicode_word::is_alphanumeric(c) || crate::unicode_word::is_mark_or_connector(c)
}

fn space(c: char) -> bool {
    c.is_ascii_whitespace()
}

impl Passes {
    pub fn extract(
        src: &str,
        base: usize,
        attrs: &mut Evaluation<'_>,
        compat: bool,
    ) -> (Text, Self) {
        let mut prefix = "\u{96}".to_owned();
        while src.contains(&prefix)
            || attrs.iter().any(|(name, value)| {
                name.contains(&prefix)
                    || value.contains(&prefix)
                    || attrs
                        .raw_parts(name)
                        .is_some_and(|p| reference_text(p).contains(&prefix))
            })
        {
            prefix.push('\u{96}');
        }
        let mut passes = Self {
            prefix,
            slots: Vec::new(),
            token_slots: 0,
            line_comment: None,
            diagnostics: Vec::new(),
        };
        let mut text = Text::default();
        text.append_source(src, true, 0, Span::new(base, base + src.len()));
        for constrained in [false, true] {
            text = passes.stage(text, attrs, compat, constrained);
        }
        text = passes.stem_stage(text, attrs);
        passes.token_slots = passes.slots.len();
        if !attrs.stateful() {
            passes.evaluate_all(0, attrs, compat, 0);
        }
        // Special-character protection follows extraction, before quotes.
        let mut escaped = Text::default();
        for fragment in text.into_fragments() {
            escaped.append_source(&fragment.text, false, 0, fragment.span);
        }
        (escaped, passes)
    }

    fn stage(
        &mut self,
        text: Text,
        attrs: &mut Evaluation<'_>,
        compat: bool,
        constrained: bool,
    ) -> Text {
        let src = &text.text;
        let mut output = Text::default();
        let mut copied = 0;
        let mut at = 0;
        let index = Index::new(src, compat);
        while at < src.len() {
            let Some(m) = index.find(src, at, copied, compat, constrained) else {
                at += src[at..].chars().next().unwrap().len_utf8();
                continue;
            };
            text.copy(copied..m.start, &mut output);
            if m.escaped > 0 {
                // Escaping removes one slash and leaves the contents available
                // to the subsequent quote/attribute/replacement/macro stages.
                text.copy(m.start..m.open - m.escaped, &mut output);
                text.copy(m.open - m.escaped + 1..m.end, &mut output);
            } else {
                let mut start = m.start;
                if m.escaped_metadata {
                    text.copy(m.start + 1..m.open, &mut output);
                    start = m.open;
                }
                let span = text.match_span(start..m.end);
                let body = &src[m.inner..m.close];
                let normalized;
                let body = if m.macro_form {
                    normalized = body.replace("\\]", "]");
                    normalized.as_str()
                } else {
                    body
                };
                let selector_span = if m.macro_form {
                    text.match_span(m.open + 5..m.inner - 1)
                } else {
                    span
                };
                let stages = pass_subs::resolve(&m.subs, selector_span, &mut self.diagnostics);
                let (id, role) = m
                    .metadata
                    .as_deref()
                    .filter(|_| !m.escaped_metadata)
                    .map(|s| {
                        let mut metadata = Text::default();
                        // Legacy suffix removal can rewrite metadata; the
                        // enclosing passthrough remains its physical origin.
                        metadata.append(s, true, 0, span, false);
                        crate::attribute_quotes::parse_attributes(
                            metadata,
                            attrs,
                            &mut self.diagnostics,
                        )
                    })
                    .unwrap_or_default();
                let wrapper = if m.mono || id.is_some() || role.is_some() {
                    let kind = if m.mono { F::Monospace } else { F::Span };
                    let open = if id.is_some() || role.is_some() {
                        P::OpenWithAttributes {
                            format: kind,
                            id,
                            role,
                        }
                    } else {
                        P::Open(kind)
                    };
                    Some(open)
                } else {
                    None
                };
                let key = format!("{}{}\u{97}", self.prefix, self.slots.len());
                self.slots.push(Slot {
                    parts: Vec::new(),
                    span,
                    mono: m.mono,
                    stem: None,
                    pending: Some((body.into(), stages, wrapper)),
                });
                output.append(&key, true, 0, span, false);
            }
            copied = m.end;
            at = m.end;
        }
        if copied == 0 {
            return text;
        }
        text.copy(copied..src.len(), &mut output);
        output
    }

    /// Ruby extracts STEM after ordinary passthroughs, so those can protect
    /// the macro itself or parts of its body. Slots keep the notation chosen
    /// at extraction time, before any stateful inner substitutions run.
    fn stem_stage(&mut self, text: Text, attrs: &Evaluation<'_>) -> Text {
        let src = &text.text;
        if !src.contains(':') || !(src.contains("stem:") || src.contains("math:")) {
            return text;
        }
        let index = Index::new(src, false);
        let mut output = Text::default();
        let mut copied = 0;
        let mut at = 0;
        while at < src.len() {
            let escaped = src.as_bytes()[at] == b'\\';
            let start = at + usize::from(escaped);
            let rest = &src[start..];
            let prefix = ["stem:", "asciimath:", "latexmath:"]
                .into_iter()
                .find(|prefix| rest.starts_with(prefix));
            let matched = prefix.and_then(|prefix| {
                let selector = start + prefix.len();
                let bracket = Index::next(&index.opens, selector)?;
                let subs = &src[selector..bracket];
                if !pass_subs::syntax(subs) {
                    return None;
                }
                // Unlike pass:[], STEM requires at least one content byte.
                let close = Index::next(&index.macro_ends, bracket + 2)?;
                Some((prefix, selector, bracket, close))
            });
            let Some((prefix, selector, bracket, close)) = matched else {
                at += src[at..].chars().next().unwrap().len_utf8();
                continue;
            };
            text.copy(copied..at, &mut output);
            let end = close + 1;
            if escaped {
                text.copy(start..end, &mut output);
            } else {
                let span = text.match_span(at..end);
                let style = crate::StemStyle::from_alias(if prefix == "stem:" {
                    attrs.get("stem")
                } else {
                    Some(prefix.trim_end_matches(':'))
                });
                let mut body = src[bracket + 1..close]
                    .replace("\\]", "]")
                    .replace("\r\n", "\n");
                if style == crate::StemStyle::LatexMath
                    && body.starts_with('$')
                    && body.ends_with('$')
                {
                    // Ruby raises on a single dollar; keep editor input safe.
                    body = body
                        .get(1..body.len().saturating_sub(1))
                        .unwrap_or("")
                        .to_owned();
                }
                let stages = if selector == bracket {
                    vec![Stage::Specials]
                } else {
                    pass_subs::resolve(
                        &src[selector..bracket],
                        text.match_span(selector..bracket),
                        &mut self.diagnostics,
                    )
                };
                let key = format!("{}{}\u{97}", self.prefix, self.slots.len());
                self.slots.push(Slot {
                    parts: Vec::new(),
                    span,
                    mono: false,
                    stem: Some(style),
                    pending: Some((body, stages, None)),
                });
                output.append(&key, true, 0, span, false);
            }
            at = end;
            copied = end;
        }
        if copied == 0 {
            return text;
        }
        text.copy(copied..src.len(), &mut output);
        output
    }

    fn evaluate_all(
        &mut self,
        start: usize,
        attrs: &mut Evaluation<'_>,
        compat: bool,
        depth: usize,
    ) {
        let end = self.slots.len();
        for i in start..end {
            self.evaluate_slot(i, attrs, compat, depth);
        }
    }

    fn evaluate_slot(&mut self, i: usize, attrs: &mut Evaluation<'_>, compat: bool, depth: usize) {
        let pending = if attrs.stateful() {
            self.slots[i].pending.clone()
        } else {
            self.slots[i].pending.take()
        };
        let Some((body, stages, wrapper)) = pending else {
            return;
        };
        let span = self.slots[i].span;
        let mut parts = if depth >= pass_subs::MAX_DEPTH {
            self.diagnostics.push(crate::Diagnostic {
                report: Default::default(),
                span,
                severity: crate::Severity::Warning,
                message: "passthrough substitution nesting limit (32) exceeded".into(),
            });
            vec![P::Raw(body)]
        } else {
            self.apply_selected(body, &stages, attrs, compat, span, depth)
        };
        if let Some(open) = wrapper {
            let kind = match &open {
                P::Open(f) | P::OpenWithAttributes { format: f, .. } => *f,
                _ => unreachable!(),
            };
            parts.insert(0, open);
            parts.push(P::Close(kind));
        }
        let parts = self.restore_parts(parts);
        self.slots[i].parts = if let Some(style) = self.slots[i].stem {
            vec![P::Stem(Box::new(crate::Stem { style, parts }))]
        } else {
            parts
        };
    }

    fn restore_evaluated(
        &mut self,
        parts: Vec<P>,
        attrs: &mut Evaluation<'_>,
        compat: bool,
        depth: usize,
    ) -> Vec<P> {
        let mut out = Vec::new();
        for part in parts {
            if let P::Footnote(mut footnote) = part {
                let content = std::mem::take(&mut footnote.content);
                footnote.content = self.restore_evaluated(content, attrs, compat, depth);
                if let Some(registry) = attrs.footnotes() {
                    registry.restore(&footnote);
                }
                out.push(P::Footnote(footnote));
            } else if let P::Ui(mut ui) = part {
                for label in ui.labels_mut() {
                    *label = self.restore_evaluated(std::mem::take(label), attrs, compat, depth);
                }
                out.push(P::Ui(ui));
            } else if let P::Stem(mut stem) = part {
                stem.parts =
                    self.restore_evaluated(std::mem::take(&mut stem.parts), attrs, compat, depth);
                out.push(P::Stem(stem));
            } else if let P::IndexTerm(mut term) = part {
                for label in term.labels_mut() {
                    *label = self.restore_evaluated(std::mem::take(label), attrs, compat, depth);
                }
                out.push(P::IndexTerm(term));
            } else if part.is_boundary() {
                push_value(
                    &mut out,
                    part.map_metadata(|s| {
                        reference_text(&self.restore_stateful_text(s, true, attrs, compat, depth))
                    }),
                );
            } else {
                for p in
                    self.restore_stateful_text(part.text(), part.is_raw(), attrs, compat, depth)
                {
                    push_value(&mut out, p);
                }
            }
        }
        out
    }

    fn restore_stateful_text(
        &mut self,
        value: &str,
        raw: bool,
        attrs: &mut Evaluation<'_>,
        compat: bool,
        depth: usize,
    ) -> Vec<P> {
        // A slot can occur in both a link target and its fallback label. Each
        // occurrence executes independently, in restoration order, as in Ruby.
        let mut out = Vec::new();
        let mut rest = value;
        while let Some(at) = rest.find(&self.prefix) {
            let after = at + self.prefix.len();
            let Some((number, tail)) = rest[after..].split_once('\u{97}') else {
                break;
            };
            let Some(i) = number
                .parse::<usize>()
                .ok()
                .filter(|&i| i < self.slots.len())
            else {
                push_part(&mut out, &rest[..after], raw);
                rest = &rest[after..];
                continue;
            };
            push_part(&mut out, &rest[..at], raw);
            self.evaluate_slot(i, attrs, compat, depth + 1);
            for part in self.slots[i].parts.clone() {
                push_value(&mut out, part);
            }
            rest = tail;
        }
        push_part(&mut out, rest, raw);
        out
    }

    fn apply_selected(
        &mut self,
        body: String,
        stages: &[Stage],
        attrs: &mut Evaluation<'_>,
        compat: bool,
        span: Span,
        depth: usize,
    ) -> Vec<P> {
        let mut parts = if stages.contains(&Stage::Macros) && !body.is_empty() {
            let first = self.slots.len();
            let mut text = Text::default();
            // A nested selector can be transformed by outer unescaping; use
            // its enclosing physical construct instead of synthetic offsets.
            text.append(&body, true, 0, span, false);
            for constrained in [false, true] {
                text = self.stage(text, attrs, compat, constrained);
            }
            text = self.stem_stage(text, attrs);
            if !attrs.stateful() {
                self.evaluate_all(first, attrs, compat, depth + 1);
            }
            text.into_parts()
        } else {
            vec![P::Raw(body)]
        };
        for &stage in stages {
            parts = pass_subs::apply(parts, stage, attrs, compat, span, depth, self);
        }
        if attrs.stateful() {
            self.restore_evaluated(parts, attrs, compat, depth)
        } else {
            self.restore_parts(parts)
        }
    }

    pub fn normal_parameter(
        &mut self,
        body: String,
        attrs: &mut Evaluation<'_>,
        compat: bool,
        span: Span,
        depth: usize,
    ) -> Vec<P> {
        if depth >= pass_subs::MAX_DEPTH {
            self.diagnostics.push(crate::Diagnostic {
                report: Default::default(),
                span,
                severity: crate::Severity::Warning,
                message: "link parameter substitution nesting limit (32) exceeded".into(),
            });
            return vec![P::Text(body)];
        }
        use Stage::*;
        self.apply_selected(
            body,
            &[Specials, Quotes, Attributes, Replacements, Macros, Post],
            attrs,
            compat,
            span,
            depth + 1,
        )
    }

    pub fn selected(
        body: &str,
        subs: &str,
        attrs: &mut Evaluation<'_>,
        compat: bool,
        span: Span,
        diagnostics: &mut Vec<crate::Diagnostic>,
    ) -> Vec<P> {
        let stages = pass_subs::resolve(subs, span, diagnostics);
        Self::selected_stages(body, &stages, None, attrs, compat, span, diagnostics)
    }

    /// `line_comment` is a block's `line-comment` for its callouts stage.
    pub fn selected_stages(
        body: &str,
        stages: &[Stage],
        line_comment: Option<&str>,
        attrs: &mut Evaluation<'_>,
        compat: bool,
        span: Span,
        diagnostics: &mut Vec<crate::Diagnostic>,
    ) -> Vec<P> {
        // Use the same collision check as ordinary source extraction, without
        // extracting anything unless macros is one of the requested stages.
        let mut prefix = "\u{96}".to_owned();
        while body.contains(&prefix)
            || attrs.iter().any(|(k, v)| {
                v.contains(&prefix)
                    || attrs
                        .raw_parts(k)
                        .is_some_and(|p| reference_text(p).contains(&prefix))
            })
        {
            prefix.push('\u{96}');
        }
        let mut passes = Self {
            prefix,
            slots: Vec::new(),
            token_slots: 0,
            line_comment: line_comment.map(str::to_owned),
            diagnostics: Vec::new(),
        };
        let parts = passes.apply_selected(body.into(), stages, attrs, compat, span, 0);
        diagnostics.append(&mut passes.diagnostics);
        parts
    }

    pub fn contains(&self, at: usize) -> bool {
        self.slots
            .iter()
            .any(|s| s.span.start <= at && at < s.span.end)
    }

    pub fn tokens(&self) -> impl Iterator<Item = Token> + '_ {
        // Parameters generated during the macro stage have only an enclosing
        // link origin; they must not invent passthrough highlight tokens.
        self.slots.iter().take(self.token_slots).map(|s| Token {
            span: s.span,
            kind: if s.stem.is_some() {
                TokenKind::InlineMacro
            } else if s.mono {
                TokenKind::Monospace
            } else {
                TokenKind::PassthroughText
            },
        })
    }

    fn restore_text(&self, value: &str, raw: bool) -> Vec<P> {
        let mut out = Vec::new();
        let mut rest = value;
        while let Some(at) = rest.find(&self.prefix) {
            let after = at + self.prefix.len();
            let Some((number, tail)) = rest[after..].split_once('\u{97}') else {
                break;
            };
            let Some(slot) = number.parse::<usize>().ok().and_then(|i| self.slots.get(i)) else {
                // A literal prefix cannot occur in source/host values; tolerate
                // one introduced by an expansion without dropping its text.
                push_part(&mut out, &rest[..after], raw);
                rest = &rest[after..];
                continue;
            };
            push_part(&mut out, &rest[..at], raw);
            for part in &slot.parts {
                push_value(&mut out, part.clone());
            }
            rest = tail;
        }
        push_part(&mut out, rest, raw);
        out
    }

    fn restore_parts(&self, parts: Vec<P>) -> Vec<P> {
        let mut out = Vec::new();
        for part in parts {
            if part.is_boundary() {
                push_value(
                    &mut out,
                    part.map_metadata(|s| reference_text(&self.restore_text(s, true))),
                );
            } else {
                for p in self.restore_text(part.text(), part.is_raw()) {
                    push_value(&mut out, p);
                }
            }
        }
        out
    }

    pub fn restore(&mut self, nodes: &mut [Inline], attrs: &mut Evaluation<'_>, compat: bool) {
        if self.slots.is_empty() {
            return;
        }
        for node in nodes {
            match node {
                Inline::Text { value, span } | Inline::Raw { value, span }
                    if value.contains(&self.prefix) =>
                {
                    let span = *span;
                    let raw = matches!(node, Inline::Raw { .. });
                    let value = match node {
                        Inline::Text { value, .. } | Inline::Raw { value, .. } => value,
                        _ => unreachable!(),
                    };
                    let parts = self.restore_evaluated(
                        vec![if raw {
                            P::Raw(value.clone())
                        } else {
                            P::Text(value.clone())
                        }],
                        attrs,
                        compat,
                        0,
                    );
                    *node = Inline::Substituted {
                        value: parts_text(&parts),
                        parts,
                        span,
                    };
                }
                Inline::Substituted { value, parts, .. } => {
                    *parts = self.restore_evaluated(std::mem::take(parts), attrs, compat, 0);
                    *value = parts_text(parts);
                }
                Inline::AttributeRef {
                    value: Some(value),
                    parts,
                    ..
                } => {
                    *parts = if parts.is_empty() {
                        self.restore_evaluated(vec![P::Text(value.clone())], attrs, compat, 0)
                    } else {
                        self.restore_evaluated(std::mem::take(parts), attrs, compat, 0)
                    };
                    *value = parts_text(parts);
                    if !parts.iter().any(P::is_special) {
                        parts.clear();
                    }
                }
                Inline::Strong { children, .. }
                | Inline::Emphasis { children, .. }
                | Inline::Monospace { children, .. }
                | Inline::Mark { children, .. }
                | Inline::Superscript { children, .. }
                | Inline::Subscript { children, .. } => self.restore(children, attrs, compat),
                Inline::Link {
                    target, children, ..
                } => {
                    *target = reference_text(&self.restore_evaluated(
                        vec![P::Raw(target.clone())],
                        attrs,
                        compat,
                        0,
                    ));
                    self.restore(children, attrs, compat);
                }
                Inline::CrossReference { xref, children, .. } => {
                    xref.map_fields(|value| {
                        reference_text(&self.restore_evaluated(
                            vec![P::Raw(value.to_owned())],
                            attrs,
                            compat,
                            0,
                        ))
                    });
                    self.restore(children, attrs, compat);
                }
                Inline::Image { image, .. } => {
                    image.map_fields(|value| {
                        reference_text(&self.restore_evaluated(
                            vec![P::Raw(value.to_owned())],
                            attrs,
                            compat,
                            0,
                        ))
                    });
                }
                Inline::Footnote { footnote, .. } => {
                    let content = std::mem::take(&mut footnote.content);
                    footnote.content = self.restore_evaluated(content, attrs, compat, 0);
                    if let Some(registry) = attrs.footnotes() {
                        registry.restore(footnote);
                    }
                }
                Inline::Ui { ui, .. } => {
                    for label in ui.labels_mut() {
                        *label = self.restore_evaluated(std::mem::take(label), attrs, compat, 0);
                    }
                }
                Inline::Stem { stem, .. } => {
                    stem.parts =
                        self.restore_evaluated(std::mem::take(&mut stem.parts), attrs, compat, 0);
                }
                Inline::IndexTerm { term, .. } => {
                    for label in term.labels_mut() {
                        *label = self.restore_evaluated(std::mem::take(label), attrs, compat, 0);
                    }
                }
                Inline::Anchor { id, .. } => {
                    *id = reference_text(&self.restore_evaluated(
                        vec![P::Raw(id.clone())],
                        attrs,
                        compat,
                        0,
                    ));
                }
                _ => {}
            }
            if let Inline::Substituted { parts, span, .. } = node {
                if let [P::Stem(stem)] = parts.as_slice() {
                    *node = Inline::Stem {
                        stem: *stem.clone(),
                        span: *span,
                    };
                }
            }
        }
    }
}

// Index terminators once; unfinished editor input must not repeatedly search
// the remaining suffix for every opening delimiter.
struct Index {
    brackets: Vec<usize>,
    opens: Vec<usize>,
    plus_closes: Vec<usize>,
    backtick_closes: Vec<usize>,
    macro_ends: Vec<usize>,
    plus: Vec<usize>,
    double: Vec<usize>,
    triple: Vec<usize>,
    dollar: Vec<usize>,
    backtick: Vec<usize>,
}
impl Index {
    fn new(src: &str, compat: bool) -> Self {
        let positions = |needle: &str| {
            src.char_indices()
                .filter_map(|(i, _)| src[i..].starts_with(needle).then_some(i))
                .collect()
        };
        let closers = |marker: char| {
            src.char_indices()
                .filter_map(|(i, c)| {
                    (c == marker
                        && src[..i].chars().next_back().is_some_and(|c| !space(c))
                        && !src[i + 1..]
                            .chars()
                            .next()
                            .is_some_and(|c| word(c) || compat && c == '`'))
                    .then_some(i)
                })
                .collect()
        };
        Self {
            opens: positions("["),
            plus_closes: closers('+'),
            backtick_closes: closers('`'),
            brackets: positions("]"),
            macro_ends: src
                .match_indices(']')
                .filter_map(|(i, _)| (i == 0 || src.as_bytes()[i - 1] != b'\\').then_some(i))
                .collect(),
            plus: positions("+"),
            double: positions("++"),
            triple: positions("+++"),
            dollar: positions("$$"),
            backtick: positions("`"),
        }
    }
    fn next(v: &[usize], at: usize) -> Option<usize> {
        v.get(v.partition_point(|&n| n < at)).copied()
    }
    fn find(
        &self,
        src: &str,
        at: usize,
        consumed: usize,
        compat: bool,
        constrained: bool,
    ) -> Option<Match> {
        let mut cursor = at;
        let escaped_metadata = src[cursor..].starts_with("\\[");
        if escaped_metadata {
            cursor += 1;
        }
        let metadata = if src[cursor..].starts_with('[') {
            let end = Self::next(&self.brackets, cursor + 2)?;
            let value = &src[cursor + 1..end];
            cursor = end + 1;
            Some(value)
        } else {
            None
        };
        if escaped_metadata && metadata.is_none() {
            return None;
        }
        let slash_start = cursor;
        while src[cursor..].starts_with('\\')
            && cursor - slash_start < if constrained { 1 } else { 2 }
        {
            cursor += 1;
        }
        let escaped = cursor - slash_start;
        let open = cursor;
        if !constrained && src[open..].starts_with("pass:") && metadata.is_none() && escaped <= 1 {
            let bracket = Self::next(&self.opens, open + 5)?;
            let subs = &src[open + 5..bracket];
            if !pass_subs::syntax(subs) {
                return None;
            }
            let close = if src[bracket + 1..].starts_with(']') {
                bracket + 1
            } else {
                Self::next(&self.macro_ends, bracket + 2)?
            };
            return Some(Match {
                start: at,
                open,
                inner: bracket + 1,
                close,
                end: close + 1,
                metadata: None,
                escaped,
                escaped_metadata,
                subs: subs.into(),
                mono: false,
                macro_form: true,
            });
        }
        let legacy = metadata.is_some_and(|s| s == "x-" || s.ends_with(" x-"));
        // Escaping legacy backtick attributes leaves ordinary quote handling.
        if constrained && !compat && legacy && escaped_metadata && src[open..].starts_with('`') {
            return None;
        }
        let (marker, positions) = if constrained {
            if compat || (legacy && src[open..].starts_with('`')) {
                ("`", &self.backtick)
            } else {
                ("+", &self.plus)
            }
        } else if src[open..].starts_with("+++") && Self::next(&self.triple, open + 3).is_some() {
            ("+++", &self.triple)
        } else if !compat && src[open..].starts_with("++") {
            ("++", &self.double)
        } else {
            ("$$", &self.dollar)
        };
        if !src[open..].starts_with(marker) {
            return None;
        }
        if constrained && at > 0 {
            let (prev_at, prev) = src[..at].char_indices().next_back()?;
            // A normal escaped plus can be recognized even next to a word.
            if !(escaped > 0 && metadata.is_none())
                && (prev_at < consumed
                    || word(prev)
                    || if compat {
                        prev == '`'
                    } else {
                        matches!(prev, ';' | ':' | '\\') && !escaped_metadata
                    })
            {
                return None;
            }
        }
        let inner = open + marker.len();
        let close = if constrained {
            let first = src[inner..].chars().next()?;
            if space(first) || (compat && first == '`') {
                return None;
            }
            let closers = if marker == "`" {
                &self.backtick_closes
            } else {
                &self.plus_closes
            };
            Self::next(closers, inner + first.len_utf8())?
        } else {
            Self::next(positions, inner)?
        };
        let mono =
            constrained && compat || legacy && !escaped_metadata && (constrained || marker == "++");
        let metadata = metadata.map(|s| {
            if legacy && mono && !compat {
                s.strip_suffix(" x-").unwrap_or("").to_owned()
            } else {
                s.to_owned()
            }
        });
        let subs = if mono && !compat && marker != "`" {
            "normal"
        } else if marker == "+++" {
            ""
        } else {
            "c"
        };
        Some(Match {
            start: at,
            open,
            inner,
            close,
            end: close + marker.len(),
            metadata,
            escaped,
            escaped_metadata,
            subs: subs.into(),
            mono,
            macro_form: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::substitution_text::tests::{assert_reused, marked_text};

    #[test]
    fn unmatched_pass_and_stem_stages_reuse_buffers_without_registering_slots() {
        for source in [
            "Ж🙂 & text\r\n",
            "+ unfinished",
            "++",
            "$$",
            "\\+unfinished",
            "pass:q[unfinished",
            "stem:[]",
            "latexmath:unfinished",
        ] {
            for compat in [false, true] {
                let attrs = crate::Attributes::new();
                let mut evaluation = Evaluation::read_only(&attrs);
                let (_, mut passes) = Passes::extract("", 0, &mut evaluation, compat);
                assert_reused(marked_text(source), |mut text| {
                    for constrained in [false, true] {
                        text = passes.stage(text, &mut evaluation, compat, constrained);
                    }
                    passes.stem_stage(text, &evaluation)
                });
                assert!(passes.slots.is_empty());
                assert!(passes.diagnostics.is_empty());
            }
        }
    }
}
