//! Temporary lexical text for ordered substitutions. Protection, node ownership
//! and source ranges travel with fragments; HTML escaping stays in the backend.
use crate::{AttributePart, IndexTerm, Inline, Span, UiMacro};
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone)]
struct Boundary {
    original: Arc<AttributePart>,
    range: Range<usize>,
}

impl Boundary {
    fn slice(&self, range: Range<usize>, length: usize) -> Option<Self> {
        let selected = if range == (0..length) {
            self.range.clone()
        } else if self.range.len() == length {
            self.range.start + range.start..self.range.start + range.end
        } else {
            return None;
        };
        Some(Self {
            original: self.original.clone(),
            range: selected,
        })
    }
}

/// A piece of a keyboard, button or menu macro's markup (or the empty markup
/// around a flow index term) while the later macro stages run over its labels
/// (`index` 0 opens, the last closes, the others separate labels). Leaving
/// the lexical text rebuilds the macro around whatever those stages made of
/// its labels.
#[derive(Clone)]
struct UiMark {
    ui: Arc<Marked>,
    index: usize,
}

/// A macro whose labels later macro stages see between its markup.
#[derive(Clone)]
pub(crate) enum Marked {
    Ui(UiMacro),
    IndexTerm(IndexTerm),
}

impl Marked {
    fn pieces(&self) -> Vec<String> {
        match self {
            Self::Ui(ui) => ui.pieces(),
            // Ruby's flow term is its bare text.
            Self::IndexTerm(_) => vec![String::new(), String::new()],
        }
    }

    fn labels_mut(&mut self) -> Vec<&mut Vec<AttributePart>> {
        match self {
            Self::Ui(ui) => ui.labels_mut(),
            Self::IndexTerm(term) => term.labels_mut(),
        }
    }

    fn into_part(self) -> AttributePart {
        match self {
            Self::Ui(ui) => AttributePart::Ui(Box::new(ui)),
            Self::IndexTerm(term) => AttributePart::IndexTerm(Box::new(term)),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Fragment {
    pub text: String,
    pub decoded: Option<String>,
    pub raw: bool,
    pub owner: usize,
    pub span: Span,
    pub part: Option<AttributePart>,
    boundary: Option<Boundary>,
    ui: Option<UiMark>,
    exact: bool,
    extend_span: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Text {
    pub text: String,
    pub fragments: Vec<(Range<usize>, Fragment)>,
}

impl Text {
    /// A source text node may already have undergone replacements. Its byte
    /// mapping is then coarse, but a macro starting here still covers later
    /// source constructs it consumes.
    pub fn append_source(&mut self, text: &str, raw: bool, owner: usize, span: Span) {
        let first = self.fragments.len();
        self.append(text, raw, owner, span, span.end - span.start == text.len());
        for (_, fragment) in &mut self.fragments[first..] {
            fragment.extend_span = true;
        }
    }

    pub fn append_part(&mut self, part: &AttributePart, owner: usize, span: Span) {
        if part.is_boundary() {
            let lexical = part.lexical().into_owned();
            let boundary = matches!(
                part,
                AttributePart::OpenWithAttributes { .. } | AttributePart::OpenLink(_)
            )
            .then(|| Boundary {
                original: Arc::new(part.clone()),
                range: 0..lexical.len(),
            });
            self.push(Fragment {
                text: lexical,
                decoded: None,
                raw: true,
                owner,
                span,
                part: Some(part.clone()),
                boundary,
                ui: None,
                exact: false,
                extend_span: false,
            });
        } else {
            self.append(part.text(), part.is_raw(), owner, span, false);
        }
    }

    pub fn from_parts(parts: &[AttributePart]) -> Self {
        Self::from_parts_at(parts, Span::new(0, 0))
    }

    /// Transformed parts retain their enclosing physical origin for diagnostics.
    pub fn from_parts_at(parts: &[AttributePart], span: Span) -> Self {
        let mut result = Self::default();
        for part in parts {
            result.append_part(part, 0, span);
        }
        result
    }

    pub fn into_fragments(mut self) -> Vec<Fragment> {
        self.restore_boundaries();
        self.restore_ui();
        self.fragments
            .into_iter()
            .map(|(_, fragment)| fragment)
            .collect()
    }

    pub fn into_parts(mut self) -> Vec<AttributePart> {
        self.restore_boundaries();
        self.restore_ui();
        let mut parts = Vec::new();
        for (_, fragment) in self.fragments {
            if let Some(part) = fragment.part {
                crate::attributes::push_value(&mut parts, part);
            } else {
                crate::attributes::push_part(
                    &mut parts,
                    fragment.decoded.as_deref().unwrap_or(&fragment.text),
                    fragment.raw,
                );
            }
        }
        parts
    }

    /// Empty text is dropped, except an atomic value or mark with an empty
    /// spelling (a concealed index term, the markup of a flow term): it stays
    /// as a zero-length fragment, so later stages see Ruby's text.
    pub fn push(&mut self, fragment: Fragment) {
        if fragment.text.is_empty() && fragment.part.is_none() && fragment.ui.is_none() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(&fragment.text);
        self.fragments.push((start..self.text.len(), fragment));
    }

    pub fn append(&mut self, text: &str, raw: bool, owner: usize, span: Span, exact: bool) {
        let mut start = 0;
        for (offset, ch) in text.char_indices() {
            let encoded = match (raw, ch) {
                (false, '&') => "&amp;",
                (false, '<') => "&lt;",
                (false, '>') => "&gt;",
                _ => continue,
            };
            self.literal(
                &text[start..offset],
                raw,
                owner,
                slice_span(span, start..offset, exact),
                exact,
            );
            self.push(Fragment {
                text: encoded.into(),
                decoded: Some(ch.to_string()),
                raw,
                owner,
                span: slice_span(span, offset..offset + ch.len_utf8(), exact),
                part: None,
                boundary: None,
                ui: None,
                exact: false,
                extend_span: exact,
            });
            start = offset + ch.len_utf8();
        }
        self.literal(
            &text[start..],
            raw,
            owner,
            slice_span(span, start..text.len(), exact),
            exact,
        );
    }

    fn literal(&mut self, text: &str, raw: bool, owner: usize, span: Span, exact: bool) {
        self.push(Fragment {
            text: text.into(),
            decoded: None,
            raw,
            owner,
            span,
            part: None,
            boundary: None,
            ui: None,
            exact,
            extend_span: exact,
        });
    }

    /// A zero-length fragment belongs to the range ending at it (or the one
    /// starting the text), so consecutive copies and replacements emit it once.
    pub fn copy(&self, range: Range<usize>, output: &mut Self) {
        let first = self
            .fragments
            .partition_point(|(r, _)| r.end < range.start || (r.end == range.start && r.end > 0));
        for (r, f) in &self.fragments[first..] {
            if r.start > range.end || (r.start == range.end && !r.is_empty()) {
                break;
            }
            let start = range.start.max(r.start) - r.start;
            let end = range.end.min(r.end) - r.start;
            let complete = start == 0 && end == f.text.len();
            // A bare URL can consume only part of an encoded character: Ruby
            // trims the final semicolon even from `&gt;`. Such a slice keeps
            // its lexical spelling, not the original decoded character. Its
            // physical span still covers the character that produced it.
            let partial_encoded = !complete && f.decoded.is_some();
            // Copy only the selected slice: cloning the complete fragment for
            // every match would make a long run of replacements quadratic.
            output.push(Fragment {
                text: f.text[start..end].into(),
                decoded: f.decoded.clone().filter(|_| complete),
                raw: f.raw || partial_encoded,
                owner: f.owner,
                span: slice_span(f.span, start..end, f.exact),
                part: if complete { f.part.clone() } else { None },
                boundary: f
                    .boundary
                    .as_ref()
                    .and_then(|b| b.slice(start..end, f.text.len())),
                ui: f.ui.clone().filter(|_| start == 0 && end == f.text.len()),
                exact: f.exact,
                extend_span: f.extend_span,
            });
        }
    }

    pub fn replacement(
        &self,
        range: Range<usize>,
        lexical: &str,
        decoded: Option<&str>,
        output: &mut Self,
    ) {
        let (first, span) = self.replacement_origin(range.clone());
        let (r, f) = &self.fragments[first];
        let boundary = f.boundary.as_ref().and_then(|b| {
            let mut selected = b.slice(
                range.start - r.start..range.end.min(r.end) - r.start,
                f.text.len(),
            )?;
            for (next_range, next) in &self.fragments[first + 1..] {
                if next_range.start >= range.end {
                    break;
                }
                let next_boundary = next.boundary.as_ref()?.slice(
                    0..range.end.min(next_range.end) - next_range.start,
                    next.text.len(),
                )?;
                if !Arc::ptr_eq(&selected.original, &next_boundary.original)
                    || selected.range.end > next_boundary.range.start
                {
                    return None;
                }
                selected.range.end = next_boundary.range.end;
            }
            Some(selected)
        });
        output.push(Fragment {
            text: lexical.into(),
            decoded: decoded.map(str::to_owned),
            raw: decoded.is_none(),
            owner: f.owner,
            span,
            part: None,
            boundary,
            ui: None,
            exact: false,
            extend_span: f.extend_span,
        });
        self.trailing(range.end, output);
    }

    /// Find the first owner and its physical coverage without materializing a
    /// replacement. Span queries and semantic macros need no lexical buffer,
    /// copied boundary metadata or trailing zero-length marks.
    fn replacement_origin(&self, range: Range<usize>) -> (usize, Span) {
        let first = self
            .fragments
            .partition_point(|(r, _)| r.end <= range.start);
        let (r, f) = &self.fragments[first];
        let mut span = slice_span(
            f.span,
            range.start - r.start..range.end.min(r.end) - r.start,
            f.exact,
        );
        // AttributeRef keeps its own source token. A replacement starting in
        // ordinary text covers all source bytes it consumes, including refs.
        if f.extend_span {
            for (r, next) in &self.fragments[first + 1..] {
                if r.start >= range.end {
                    break;
                }
                let end = slice_span(next.span, 0..range.end.min(r.end) - r.start, next.exact).end;
                span.end = span.end.max(end);
            }
        }
        (first, span)
    }

    /// Zero-length fragments at the end of a replaced range follow it: the
    /// markup closing a flow index term survives a macro consuming its text.
    fn trailing(&self, end: usize, output: &mut Self) {
        let first = self
            .fragments
            .partition_point(|(r, _)| r.end < end || (r.end == end && !r.is_empty()));
        for (r, f) in &self.fragments[first..] {
            if r.start > end || !r.is_empty() {
                break;
            }
            output.push(f.clone());
        }
    }

    /// Physical coverage of a match, using the same ownership rules as replacements.
    pub fn match_span(&self, range: Range<usize>) -> Span {
        self.replacement_origin(range).1
    }

    /// Physical union of the fragments a range selects in the source of its
    /// first owner, e.g. a consumed label starting at a formatting boundary.
    pub fn cover_span(&self, range: Range<usize>) -> Span {
        let mut span = self.match_span(range.clone());
        let first = self
            .fragments
            .partition_point(|(r, _)| r.end <= range.start);
        for (r, f) in &self.fragments[first..] {
            if r.start >= range.end {
                break;
            }
            let selected = range.start.max(r.start) - r.start..range.end.min(r.end) - r.start;
            let part = slice_span(f.span, selected, f.exact);
            if part.source_id == span.source_id && part.provenance == span.provenance {
                span.start = span.start.min(part.start);
                span.end = span.end.max(part.end);
            }
        }
        span
    }

    /// Anchor a generated semantic value to its first source owner. References
    /// consumed later in the range are retained as empty nodes by write_back.
    pub fn replace_parts(&self, range: Range<usize>, parts: &[AttributePart], output: &mut Self) {
        let (first, span) = self.replacement_origin(range.clone());
        let owner = self.fragments[first].1.owner;
        for part in parts {
            output.append_part(part, owner, span);
        }
        self.trailing(range.end, output);
    }

    /// A keyboard, button or menu macro replacing a range: Ruby's markup as
    /// marks around its labels, so the later macro stages see the same
    /// lexical text as in Ruby and nest what they find inside a label.
    pub fn replace_ui(&self, range: Range<usize>, mut ui: Marked, output: &mut Self) {
        let (first, span) = self.replacement_origin(range.clone());
        let owner = self.fragments[first].1.owner;
        let pieces = ui.pieces();
        let labels: Vec<Vec<AttributePart>> =
            ui.labels_mut().into_iter().map(std::mem::take).collect();
        let ui = Arc::new(ui);
        for (index, piece) in pieces.into_iter().enumerate() {
            output.push(Fragment {
                text: piece,
                decoded: None,
                raw: true,
                owner,
                span,
                part: None,
                boundary: None,
                ui: Some(UiMark {
                    ui: ui.clone(),
                    index,
                }),
                exact: false,
                extend_span: false,
            });
            for part in labels.get(index).into_iter().flatten() {
                output.append_part(part, owner, span);
            }
        }
        self.trailing(range.end, output);
    }

    /// Rebuild every complete keyboard, button or menu macro around its
    /// labels. Marks a later stage separated or crossed (an unbalanced
    /// boundary inside a label) stay Ruby's literal markup.
    fn restore_ui(&mut self) {
        if !self.fragments.iter().any(|(_, f)| f.ui.is_some()) {
            return;
        }
        let fragments = std::mem::take(&mut self.fragments);
        let mut i = 0;
        while i < fragments.len() {
            i = rebuild_ui(&self.text, &fragments, i, &mut self.fragments);
        }
    }

    /// Reassemble only boundaries we created, after replacements edit their
    /// lexical id/role values. Arbitrary raw tags never become semantic nodes.
    fn restore_boundaries(&mut self) {
        if !self
            .fragments
            .iter()
            .any(|(_, f)| f.part.is_none() && f.boundary.is_some())
        {
            return;
        }
        let mut pending = std::mem::take(&mut self.fragments).into_iter().peekable();
        while let Some((range, mut fragment)) = pending.next() {
            if fragment.part.is_some() {
                self.fragments.push((range, fragment));
                continue;
            }
            let Some(boundary) = fragment.boundary.clone().filter(|b| b.range.start == 0) else {
                self.fragments.push((range, fragment));
                continue;
            };
            let mut group = Vec::new();
            let mut end = boundary.range.end;
            let mut lexical_end = range.end;
            let mut lexical = fragment.text.clone();
            while let Some((_, next)) = pending.peek() {
                if !next.boundary.as_ref().is_some_and(|b| {
                    // Escape removal can leave gaps in the original range.
                    Arc::ptr_eq(&boundary.original, &b.original) && b.range.start >= end
                }) {
                    break;
                }
                let (r, f) = pending.next().unwrap();
                end = f.boundary.as_ref().unwrap().range.end;
                lexical_end = r.end;
                lexical.push_str(&f.text);
                group.push((r, f));
            }
            if end == boundary.original.lexical().len() {
                if let Some(part) = boundary.original.with_lexical_attributes(&lexical) {
                    fragment.text = lexical;
                    fragment.decoded = None;
                    fragment.part = Some(part);
                    fragment.boundary = None;
                    self.fragments.push((range.start..lexical_end, fragment));
                    continue;
                }
            }
            self.fragments.push((range, fragment));
            self.fragments.extend(group);
        }
    }
}

fn slice_span(span: Span, range: Range<usize>, exact: bool) -> Span {
    if exact {
        Span {
            start: span.start + range.start,
            end: span.start + range.end,
            ..span
        }
    } else {
        span
    }
}

/// Copy `fragments[i]`, or the whole macro it opens as one semantic
/// fragment; returns the next index.
fn rebuild_ui(
    text: &str,
    fragments: &[(Range<usize>, Fragment)],
    i: usize,
    out: &mut Vec<(Range<usize>, Fragment)>,
) -> usize {
    let (range, open) = &fragments[i];
    let plain = |out: &mut Vec<(Range<usize>, Fragment)>| {
        let mut fragment = open.clone();
        fragment.ui = None;
        out.push((range.clone(), fragment));
        i + 1
    };
    let Some(mark) = open.ui.as_ref().filter(|mark| mark.index == 0) else {
        return plain(out);
    };
    let count = mark.ui.pieces().len() - 1;
    let mut labels = Vec::new();
    let mut current = Vec::new();
    let mut j = i + 1;
    let close = loop {
        let Some((r, f)) = fragments.get(j) else {
            break None;
        };
        match &f.ui {
            Some(m) if Arc::ptr_eq(&m.ui, &mark.ui) => {
                if m.index != labels.len() + 1 {
                    break None;
                }
                labels.push(std::mem::take(&mut current));
                if m.index == count {
                    break Some(j);
                }
                j += 1;
            }
            Some(m) if m.index == 0 => j = rebuild_ui(text, fragments, j, &mut current),
            Some(_) => break None,
            None => {
                current.push((r.clone(), f.clone()));
                j += 1;
            }
        }
    };
    let Some(close) = close else {
        return plain(out);
    };
    // Flow terms have no markup of their own: formatting may cross their
    // boundaries without needing the repairs required by UI elements.
    let repaired = if matches!(mark.ui.as_ref(), Marked::IndexTerm(_)) {
        let labels = labels
            .into_iter()
            .map(|label| {
                let mut parts = Vec::new();
                for (_, f) in label {
                    if let Some(part) = f.part {
                        crate::attributes::push_value(&mut parts, part);
                    } else {
                        crate::attributes::push_part(
                            &mut parts,
                            f.decoded.as_deref().unwrap_or(&f.text),
                            f.raw,
                        );
                    }
                }
                parts
            })
            .collect();
        Some((Vec::new(), labels, Vec::new()))
    } else {
        repair(labels)
    };
    let Some((before, labels, after)) = repaired else {
        return plain(out);
    };
    let mut ui = (*mark.ui).clone();
    for (slot, label) in ui.labels_mut().into_iter().zip(labels) {
        *slot = label;
    }
    let generated = |part: AttributePart| Fragment {
        text: part.lexical().into_owned(),
        decoded: None,
        raw: true,
        owner: open.owner,
        span: open.span,
        part: Some(part),
        boundary: None,
        ui: None,
        exact: false,
        extend_span: false,
    };
    for mut fragment in before {
        // Formatting that only opened right before the macro disappears.
        let opener = out
            .last()
            .and_then(|(_, f)| f.part.as_ref())
            .and_then(pairing);
        if opener.is_some_and(|(opens, key)| {
            opens && Some((false, key)) == fragment.part.as_ref().and_then(pairing)
        }) {
            out.pop();
            continue;
        }
        // Keep the macro's owner so write-back sees fragments in order; an
        // empty span at the macro's start lets the quote close there.
        fragment.owner = open.owner;
        fragment.span = Span {
            end: open.span.start,
            ..open.span
        };
        out.push((range.start..range.start, fragment));
    }
    let whole = range.start..fragments[close].0.end;
    out.push((
        whole.clone(),
        Fragment {
            text: text[whole.clone()].to_owned(),
            ..generated(ui.into_part())
        },
    ));
    for part in after {
        let reopened = Fragment {
            span: Span {
                start: open.span.end,
                ..open.span
            },
            ..generated(part)
        };
        out.push((whole.end..whole.end, reopened));
    }
    close + 1
}

/// A boundary's pairing key: its format, or 1/2 for a link/cross-reference.
type Pairing = (bool, Result<crate::AttributeFormat, u8>);

fn pairing(part: &AttributePart) -> Option<Pairing> {
    Some(match part {
        AttributePart::Open(format) | AttributePart::OpenWithAttributes { format, .. } => {
            (true, Ok(*format))
        }
        AttributePart::Close(format) => (false, Ok(*format)),
        AttributePart::OpenLink(_) => (true, Err(1)),
        AttributePart::CloseLink => (false, Err(1)),
        AttributePart::OpenXref(_) => (true, Err(2)),
        AttributePart::CloseXref => (false, Err(2)),
        _ => return None,
    })
}

fn closing(open: &AttributePart) -> AttributePart {
    match pairing(open) {
        Some((_, Ok(format))) => AttributePart::Close(format),
        Some((_, Err(1))) => AttributePart::CloseLink,
        _ => AttributePart::CloseXref,
    }
}

/// What [`repair`] puts before the macro, its labels, and what it reopens
/// after the macro.
type Repaired = (Vec<Fragment>, Vec<Vec<AttributePart>>, Vec<AttributePart>);

/// Labels whose boundaries all close inside them. Where an earlier stage made
/// a boundary cross the macro's markup (Ruby then emits misnested tags), the
/// boundary is split instead: one still open at a label's end closes there
/// and reopens in the next label or after the macro; formatting closed inside
/// a label but opened before the macro closes before it and is reopened at
/// the start of the labels it covered. `None` when a crossing cannot be split
/// (a link opened before the macro): Ruby's markup then stays literal.
fn repair(labels: Vec<Vec<(Range<usize>, Fragment)>>) -> Option<Repaired> {
    let mut before = Vec::new();
    let mut carry: Vec<AttributePart> = Vec::new();
    let mut result: Vec<Vec<AttributePart>> = Vec::new();
    for label in labels {
        let mut stack = carry.clone();
        let mut out = carry.clone();
        for (_, fragment) in label {
            let Some(part) = fragment.part.clone() else {
                crate::attributes::push_part(
                    &mut out,
                    fragment.decoded.as_deref().unwrap_or(&fragment.text),
                    fragment.raw,
                );
                continue;
            };
            match pairing(&part) {
                Some((true, _)) => {
                    stack.push(part.clone());
                    out.push(part);
                }
                Some((false, key)) => {
                    if stack.last().and_then(pairing).map(|(_, k)| k) == Some(key) {
                        stack.pop();
                        out.push(part);
                        continue;
                    }
                    // Only formatting opened before the macro, with nothing
                    // opened inside still pending, can close before it.
                    let (Ok(format), true) = (key, stack.is_empty()) else {
                        return None;
                    };
                    let opener = AttributePart::Open(format);
                    for previous in &mut result {
                        previous.insert(0, opener.clone());
                        previous.push(part.clone());
                    }
                    out.insert(0, opener);
                    out.push(part);
                    before.push(fragment);
                }
                None => crate::attributes::push_value(&mut out, part),
            }
        }
        for open in stack.iter().rev() {
            out.push(closing(open));
        }
        carry = stack;
        result.push(out);
    }
    Some((before, result, carry))
}

/// Restore AST nodes, retaining attribute references even if a replacement
/// consumed their whole value. This keeps editor tokens and provenance stable.
pub(crate) fn write_back(nodes: Vec<Inline>, mut text: Text, fold_newlines: bool) -> Vec<Inline> {
    text.restore_boundaries();
    text.restore_ui();
    let mut fragments = text.fragments.into_iter().peekable();
    let mut result = Vec::new();
    for (owner, mut node) in nodes.into_iter().enumerate() {
        let mut pieces = Vec::new();
        while fragments.peek().is_some_and(|(_, f)| f.owner == owner) {
            let (_, mut f) = fragments.next().unwrap();
            f.text = f.decoded.take().unwrap_or(f.text);
            if fold_newlines && !f.raw {
                f.text = f.text.replace(['\r', '\n'], " ");
            }
            pieces.push(f);
        }
        match &mut node {
            Inline::AttributeRef {
                name, value, parts, ..
            } => {
                parts.clear();
                for f in pieces {
                    if let Some(part) = f.part {
                        crate::attributes::push_value(parts, part);
                    } else {
                        crate::attributes::push_part(parts, &f.text, f.raw);
                    }
                }
                let resolved = crate::attributes::parts_text(parts);
                if value.is_some()
                    || resolved != format!("{{{name}}}")
                    || parts.iter().any(crate::AttributePart::is_special)
                {
                    *value = Some(resolved);
                }
                if !parts.iter().any(crate::AttributePart::is_special) {
                    parts.clear();
                }
                result.push(node);
            }
            Inline::Text {
                span: original_span,
                ..
            }
            | Inline::Raw {
                span: original_span,
                ..
            }
            | Inline::Substituted {
                span: original_span,
                ..
            } => {
                if pieces.iter().any(|f| f.part.is_some()) {
                    // Keep generated constructs separate from their surrounding
                    // source text so direct links have precise physical spans.
                    let mut pieces = pieces.into_iter().peekable();
                    while let Some(first) = pieces.next() {
                        let span = first.span;
                        let mut group = vec![first];
                        while pieces.peek().is_some_and(|f| f.span == span) {
                            group.push(pieces.next().unwrap());
                        }
                        let mut parts = Vec::new();
                        let semantic = group.iter().any(|f| f.part.is_some());
                        for f in group {
                            if let Some(part) = f.part {
                                crate::attributes::push_value(&mut parts, part);
                            } else {
                                crate::attributes::push_part(&mut parts, &f.text, f.raw);
                            }
                        }
                        if semantic {
                            result.push(Inline::Substituted {
                                value: crate::attributes::parts_text(&parts),
                                parts,
                                span,
                            });
                        } else {
                            for part in parts {
                                result.push(if part.is_raw() {
                                    Inline::Raw {
                                        value: part.text().into(),
                                        span,
                                    }
                                } else {
                                    Inline::Text {
                                        value: part.text().into(),
                                        span,
                                    }
                                });
                            }
                        }
                    }
                    continue;
                }
                if !pieces.is_empty() && pieces.iter().all(|f| !f.raw) {
                    let end = pieces.iter().map(|f| f.span.end).max().unwrap();
                    result.push(Inline::Text {
                        value: pieces.into_iter().map(|f| f.text).collect(),
                        span: Span {
                            end: original_span.end.max(end),
                            ..*original_span
                        },
                    });
                    continue;
                }
                let start = result.len();
                for f in pieces {
                    if result.len() > start {
                        if let Some(previous) = result.last_mut() {
                            match previous {
                                Inline::Text { value, span } if !f.raw => {
                                    value.push_str(&f.text);
                                    span.end = span.end.max(f.span.end);
                                    continue;
                                }
                                Inline::Raw { value, span } if f.raw => {
                                    value.push_str(&f.text);
                                    span.end = span.end.max(f.span.end);
                                    continue;
                                }
                                _ => {}
                            }
                        }
                    }
                    result.push(if f.raw {
                        Inline::Raw {
                            value: f.text,
                            span: f.span,
                        }
                    } else {
                        Inline::Text {
                            value: f.text,
                            span: f.span,
                        }
                    });
                }
            }
            _ => unreachable!("only resolved text enters the replacement stream"),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttributeFormat, SourceId};

    #[test]
    fn match_origins_keep_encoded_unicode_and_attribute_ownership() {
        let mut text = Text::default();
        let source = Span {
            provenance: Some(3),
            ..Span::in_source(SourceId(7), 10, 13)
        };
        text.append_source("&Ж", false, 0, source);
        let attribute = Span::in_source(SourceId(8), 30, 37);
        text.append_part(&AttributePart::Text("expanded".into()), 1, attribute);
        text.append_source("🙂\r\n", false, 2, Span::in_source(SourceId(7), 50, 56));
        assert_eq!(text.text, "&amp;Жexpanded🙂\r\n");
        assert_eq!(text.match_span(1..4), Span { end: 11, ..source });
        assert_eq!(
            text.match_span(5..7),
            Span {
                start: 11,
                ..source
            }
        );
        assert_eq!(
            text.match_span(0..text.text.len()),
            Span { end: 56, ..source }
        );
        // Starting inside the expansion stays owned by the attribute, even
        // when the match also consumes following physical source characters.
        assert_eq!(text.match_span(8..text.text.len()), attribute);
        let part = AttributePart::Anchor("target".into());
        let mut out = Text::default();
        text.replace_parts(8..text.text.len(), std::slice::from_ref(&part), &mut out);
        let fragments = out.into_fragments();
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].owner, 1);
        assert_eq!(fragments[0].span, attribute);
        assert_eq!(fragments[0].part, Some(part));
    }

    #[test]
    fn changed_boundaries_restore_metadata_but_raw_tags_stay_raw() {
        let span = Span::in_source(SourceId(2), 5, 9);
        let original = AttributePart::OpenWithAttributes {
            format: AttributeFormat::Strong,
            id: Some("Ж".into()),
            role: Some("old".into()),
        };
        let mut text = Text::from_parts_at(&[original], span);
        text.append_part(&AttributePart::Raw("<b class=\"old\">".into()), 0, span);
        let at = text.text.find("old").unwrap();
        let mut changed = Text::default();
        text.copy(0..at, &mut changed);
        text.replacement(at..at + 3, "new", None, &mut changed);
        text.copy(at + 3..text.text.len(), &mut changed);
        assert_eq!(
            changed.into_parts(),
            vec![
                AttributePart::OpenWithAttributes {
                    format: AttributeFormat::Strong,
                    id: Some("Ж".into()),
                    role: Some("new".into()),
                },
                AttributePart::Raw("<b class=\"old\">".into()),
            ]
        );
    }

    #[test]
    fn semantic_replacements_keep_trailing_zero_length_parts_once() {
        let span = Span::new(10, 14);
        let hidden = AttributePart::IndexTerm(Box::new(IndexTerm {
            text: None,
            terms: vec!["hidden".into()],
            see: None,
            see_also: Vec::new(),
        }));
        let mut text = Text::default();
        text.append_source("word", false, 3, span);
        text.append_part(&hidden, 4, Span::new(14, 14));
        let mut out = Text::default();
        text.replace_parts(0..4, &[AttributePart::Text("Ж".into())], &mut out);
        text.copy(4..4, &mut out);
        let fragments = out.into_fragments();
        assert_eq!(fragments.len(), 2);
        assert_eq!(fragments[0].span, span);
        assert_eq!(fragments[0].owner, 3);
        assert_eq!(fragments[1].owner, 4);
        assert_eq!(fragments[1].part, Some(hidden));
    }
}
