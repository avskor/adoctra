//! Extract protected inline regions before quotes and restore their semantic
//! parts only after the outer substitutions finish. Slots are private and
//! collision-free for this source and its attribute context.
use crate::attributes::{parts_text, push_part, push_value, reference_text};
use crate::pass_subs::{self, Stage};
use crate::substitution_text::Text;
use crate::{AttributeFormat as F, AttributePart as P, Attributes, Inline, Span, Token, TokenKind};

struct Slot {
    parts: Vec<P>,
    span: Span,
    mono: bool,
    pending: Option<(String, Vec<Stage>, Option<P>)>,
}

pub(crate) struct Passes {
    prefix: String,
    slots: Vec<Slot>,
    token_slots: usize,
    pub diagnostics: Vec<crate::Diagnostic>,
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
    pub fn extract(src: &str, base: usize, attrs: &Attributes, compat: bool) -> (Text, Self) {
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
            diagnostics: Vec::new(),
        };
        let mut text = Text::default();
        text.append_source(src, true, 0, Span::new(base, base + src.len()));
        for constrained in [false, true] {
            text = passes.stage(text, attrs, compat, constrained);
        }
        passes.token_slots = passes.slots.len();
        passes.evaluate_all(0, attrs, compat, 0);
        // Special-character protection follows extraction, before quotes.
        let mut escaped = Text::default();
        for fragment in text.into_fragments() {
            escaped.append_source(&fragment.text, false, 0, fragment.span);
        }
        (escaped, passes)
    }

    fn stage(&mut self, text: Text, attrs: &Attributes, compat: bool, constrained: bool) -> Text {
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
                    pending: Some((body.into(), stages, wrapper)),
                });
                output.append(&key, true, 0, span, false);
            }
            copied = m.end;
            at = m.end;
        }
        text.copy(copied..src.len(), &mut output);
        output
    }

    fn evaluate_all(&mut self, start: usize, attrs: &Attributes, compat: bool, depth: usize) {
        // Reserve all slots from this extraction before expanding nested macros.
        // This also preserves Ruby's observable numbering in mailto URI encoding.
        let end = self.slots.len();
        for i in start..end {
            let Some((body, stages, wrapper)) = self.slots[i].pending.take() else {
                continue;
            };
            let span = self.slots[i].span;
            let mut parts = if depth >= pass_subs::MAX_DEPTH {
                self.diagnostics.push(crate::Diagnostic {
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
            self.slots[i].parts = self.restore_parts(parts);
        }
    }

    fn apply_selected(
        &mut self,
        body: String,
        stages: &[Stage],
        attrs: &Attributes,
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
            self.evaluate_all(first, attrs, compat, depth + 1);
            text.into_parts()
        } else {
            vec![P::Raw(body)]
        };
        for &stage in stages {
            parts = pass_subs::apply(parts, stage, attrs, compat, span, depth, self);
        }
        self.restore_parts(parts)
    }

    pub fn normal_parameter(
        &mut self,
        body: String,
        attrs: &Attributes,
        compat: bool,
        span: Span,
        depth: usize,
    ) -> Vec<P> {
        if depth >= pass_subs::MAX_DEPTH {
            self.diagnostics.push(crate::Diagnostic {
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
        attrs: &Attributes,
        compat: bool,
        span: Span,
        diagnostics: &mut Vec<crate::Diagnostic>,
    ) -> Vec<P> {
        let stages = pass_subs::resolve(subs, span, diagnostics);
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
            diagnostics: Vec::new(),
        };
        let parts = passes.apply_selected(body.into(), &stages, attrs, compat, span, 0);
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
            kind: if s.mono {
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

    pub fn restore(&self, nodes: &mut [Inline]) {
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
                    let parts = self.restore_text(value, raw);
                    *node = Inline::Substituted {
                        value: parts_text(&parts),
                        parts,
                        span,
                    };
                }
                Inline::Substituted { value, parts, .. } => {
                    *parts = self.restore_parts(std::mem::take(parts));
                    *value = parts_text(parts);
                }
                Inline::AttributeRef {
                    value: Some(value),
                    parts,
                    ..
                } => {
                    *parts = if parts.is_empty() {
                        self.restore_text(value, false)
                    } else {
                        self.restore_parts(std::mem::take(parts))
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
                | Inline::Subscript { children, .. } => self.restore(children),
                Inline::Link {
                    target, children, ..
                }
                | Inline::CrossReference {
                    target, children, ..
                } => {
                    *target = reference_text(&self.restore_text(target, true));
                    self.restore(children);
                }
                Inline::Anchor { id, reftext, .. } => {
                    *id = reference_text(&self.restore_text(id, true));
                    if let Some(children) = reftext {
                        self.restore(children);
                    }
                }
                _ => {}
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
