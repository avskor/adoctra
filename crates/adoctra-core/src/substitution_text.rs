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

impl Fragment {
    /// End the lexical view without copying its owned spelling. Semantic parts
    /// take precedence; decoded characters retain the fragment's protection.
    pub(crate) fn into_part(self) -> AttributePart {
        self.part.unwrap_or_else(|| {
            let value = self.decoded.unwrap_or(self.text);
            if self.raw {
                AttributePart::Raw(value)
            } else {
                AttributePart::Text(value)
            }
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct Text {
    pub text: String,
    pub fragments: Vec<(Range<usize>, Fragment)>,
}

impl Text {
    /// Protect the source-only stream after passthrough/STEM extraction. At
    /// this point fragments contain source spelling or private slot keys, not
    /// semantic parts. Match append_source's mapping even for transformed keys
    /// whose byte length happens to equal their physical span.
    pub fn protect_source(mut self) -> Self {
        if !self.text.contains(['&', '<', '>']) {
            for (_, fragment) in &mut self.fragments {
                debug_assert!(fragment.part.is_none() && fragment.boundary.is_none());
                debug_assert!(fragment.ui.is_none() && fragment.decoded.is_none());
                fragment.raw = false;
                fragment.owner = 0;
                fragment.exact = fragment.span.end - fragment.span.start == fragment.text.len();
                fragment.extend_span = true;
            }
            return self;
        }
        let mut escaped = Self::default();
        for fragment in self.into_fragments() {
            escaped.append_source(&fragment.text, false, 0, fragment.span);
        }
        escaped
    }

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
            crate::attributes::push_value(&mut parts, fragment.into_part());
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
/// A paragraph's line breaks read as spaces. A run of two or more line feeds
/// (a blank line, which only a table cell's text can hold) stays: Ruby's
/// `Cell#content` splits the converted text there.
fn fold_line_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => out.push(' '),
            '\n' => {
                let mut run = 1;
                while chars.peek() == Some(&'\n') {
                    chars.next();
                    run += 1;
                }
                if run >= 2 {
                    out.extend(std::iter::repeat('\n').take(run));
                } else {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    out
}

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
            if fold_newlines && !f.raw && f.text.contains(['\r', '\n']) {
                f.text = fold_line_breaks(&f.text);
            }
            pieces.push(f);
        }
        match &mut node {
            Inline::AttributeRef {
                name, value, parts, ..
            } => {
                parts.clear();
                for f in pieces {
                    crate::attributes::push_value(parts, f.into_part());
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
                            crate::attributes::push_value(&mut parts, f.into_part());
                        }
                        if semantic {
                            result.push(Inline::Substituted {
                                value: crate::attributes::parts_text(&parts),
                                parts,
                                span,
                            });
                        } else {
                            for part in parts {
                                result.push(match part {
                                    AttributePart::Raw(value) => Inline::Raw { value, span },
                                    AttributePart::Text(value) => Inline::Text { value, span },
                                    _ => unreachable!("the group has no semantic parts"),
                                });
                            }
                        }
                    }
                    continue;
                }
                if !pieces.is_empty() && pieces.iter().all(|f| !f.raw) {
                    let end = pieces.iter().map(|f| f.span.end).max().unwrap();
                    // A macro that ends inside this node consumed its start
                    // (e.g. the `]` after a reference in the macro's text):
                    // the rest starts at its first remaining fragment.
                    let start = pieces.iter().map(|f| f.span.start).min().unwrap();
                    result.push(Inline::Text {
                        value: pieces.into_iter().map(|f| f.text).collect(),
                        span: Span {
                            start: original_span.start.max(start),
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
pub(crate) mod tests {
    use super::*;
    use crate::{AttributeFormat, SourceId};

    #[test]
    fn write_back_keeps_buffers_when_newline_folding_has_nothing_to_change() {
        for (source, raw, fold, expected, reuse) in [
            ("Ж🙂 no breaks", false, true, "Ж🙂 no breaks", true),
            ("Ж🙂\r\nnext", false, false, "Ж🙂\r\nnext", true),
            ("Ж🙂\r\nnext", true, true, "Ж🙂\r\nnext", true),
            ("Ж🙂\nnext", false, true, "Ж🙂 next", false),
            ("Ж🙂\r\nnext", false, true, "Ж🙂  next", false),
        ] {
            let span = Span::in_source(SourceId(7), 10, 10 + source.len());
            let mut text = Text::default();
            text.append_source(source, raw, 0, span);
            let buffer = text.fragments[0].1.text.as_ptr();
            let nodes = vec![Inline::Text {
                value: source.into(),
                span,
            }];
            let result = write_back(nodes, text, fold);
            assert_eq!(result.len(), 1);
            let (value, actual_span) = match &result[0] {
                Inline::Text { value, span } if !raw => (value, span),
                Inline::Raw { value, span } if raw => (value, span),
                _ => panic!("{result:?}"),
            };
            assert_eq!(value, expected);
            assert_eq!(*actual_span, span);
            if reuse {
                assert_eq!(value.as_ptr(), buffer);
            }
        }
    }

    #[test]
    fn into_parts_moves_decoded_raw_and_literal_buffers_without_losing_boundaries() {
        let mut text = Text::default();
        let span = Span::in_source(SourceId(7), 10, 11);
        text.append_source("&", false, 0, span);
        let decoded_buffer = text.fragments[0].1.decoded.as_ref().unwrap().as_ptr();
        text.append_part(&AttributePart::Anchor("target".into()), 0, span);
        text.append_source("Ж🙂\r\n", false, 0, Span::in_source(SourceId(7), 20, 28));
        let literal_buffer = text.fragments.last().unwrap().1.text.as_ptr();
        text.append_source("&amp;", true, 0, Span::in_source(SourceId(7), 30, 35));
        let raw_buffer = text.fragments.last().unwrap().1.text.as_ptr();
        let hidden = AttributePart::IndexTerm(Box::new(crate::IndexTerm {
            text: None,
            terms: vec!["hidden".into()],
            see: None,
            see_also: vec![],
        }));
        text.append_part(&hidden, 1, Span::in_source(SourceId(8), 50, 57));
        let parts = text.into_parts();
        assert_eq!(parts[0].text().as_ptr(), decoded_buffer);
        assert_eq!(parts[2].text().as_ptr(), literal_buffer);
        assert_eq!(parts[3].text().as_ptr(), raw_buffer);
        assert_eq!(
            parts,
            [
                AttributePart::Text("&".into()),
                AttributePart::Anchor("target".into()),
                AttributePart::Text("Ж🙂\r\n".into()),
                AttributePart::Raw("&amp;".into()),
                hidden,
            ]
        );
    }

    #[test]
    fn write_back_moves_owned_strings_to_source_and_attribute_nodes_with_original_spans() {
        for newline in ["\n", "\r\n"] {
            let source = format!("Ж🙂{newline}");
            let source_span = Span::in_source(SourceId(7), 10, 10 + source.len());
            let break_span = Span::in_source(SourceId(7), source_span.end, source_span.end + 2);
            let raw_span = Span::in_source(SourceId(7), break_span.end, break_span.end + 5);
            let attribute_span = Span::in_source(SourceId(8), 50, 57);
            let empty_span = Span::in_source(SourceId(8), 60, 67);
            let mut text = Text::default();
            text.append_source(&source, false, 0, source_span);
            let source_buffer = text.fragments[0].1.text.as_ptr();
            text.append_part(&AttributePart::LineBreak, 0, break_span);
            text.append_source("&amp;", true, 0, raw_span);
            let raw_buffer = text.fragments.last().unwrap().1.text.as_ptr();
            for part in [
                AttributePart::Open(AttributeFormat::Strong),
                AttributePart::Text("label".into()),
                AttributePart::Close(AttributeFormat::Strong),
            ] {
                text.append_part(&part, 1, attribute_span);
            }
            let label_buffer = text.fragments[text.fragments.len() - 2].1.text.as_ptr();
            let nodes = vec![
                Inline::Text {
                    value: String::new(),
                    span: Span {
                        end: raw_span.end,
                        ..source_span
                    },
                },
                Inline::AttributeRef {
                    name: "v".into(),
                    value: Some("old".into()),
                    parts: vec![],
                    span: attribute_span,
                },
                Inline::AttributeRef {
                    name: "empty".into(),
                    value: Some("old".into()),
                    parts: vec![],
                    span: empty_span,
                },
            ];
            let result = write_back(nodes, text, false);
            assert_eq!(result.len(), 5);
            let Inline::Text { value, span } = &result[0] else {
                panic!("{result:?}")
            };
            assert_eq!(value, &source);
            assert_eq!(value.as_ptr(), source_buffer);
            assert_eq!(*span, source_span);
            assert!(matches!(&result[1], Inline::Substituted { parts, span, .. }
                if parts == &[AttributePart::LineBreak] && *span == break_span));
            let Inline::Raw { value, span } = &result[2] else {
                panic!("{result:?}")
            };
            assert_eq!(value, "&amp;");
            assert_eq!(value.as_ptr(), raw_buffer);
            assert_eq!(*span, raw_span);
            let Inline::AttributeRef {
                value, parts, span, ..
            } = &result[3]
            else {
                panic!("{result:?}")
            };
            assert_eq!(value.as_deref(), Some("label"));
            assert_eq!(parts[1].text().as_ptr(), label_buffer);
            assert_eq!(*span, attribute_span);
            assert!(
                matches!(&result[4], Inline::AttributeRef { value: Some(value), parts, span, .. }
                if value.is_empty() && parts.is_empty() && *span == empty_span)
            );
        }
    }

    #[test]
    fn source_protection_reuses_plain_buffers_and_resets_transformed_mapping() {
        for newline in ["\n", "\r\n"] {
            let mut text = Text::default();
            let source = format!("Ж🙂{newline}plain");
            let span = Span {
                provenance: Some(2),
                ..Span::in_source(SourceId(7), 13, 13 + source.len())
            };
            text.append_source(&source, true, 0, span);
            // Like extracted slot keys: coarse mapping, including the case
            // where the replacement and the source have equal byte lengths.
            text.append("key", true, 0, Span::in_source(SourceId(7), 50, 53), false);
            text.append("slot", true, 0, Span::in_source(SourceId(7), 60, 80), false);
            let lexical_buffer = text.text.as_ptr();
            let fragments_buffer = text.fragments.as_ptr();
            let buffers: Vec<_> = text
                .fragments
                .iter()
                .map(|(_, f)| f.text.as_ptr())
                .collect();
            let text = text.protect_source();
            assert_eq!(text.text.as_ptr(), lexical_buffer);
            assert_eq!(text.fragments.as_ptr(), fragments_buffer);
            assert_eq!(
                text.fragments
                    .iter()
                    .map(|(_, f)| f.text.as_ptr())
                    .collect::<Vec<_>>(),
                buffers
            );
            assert!(text
                .fragments
                .iter()
                .all(|(_, f)| !f.raw && f.extend_span && f.owner == 0));
            assert!(text.fragments[0].1.exact);
            assert!(text.fragments[1].1.exact);
            assert!(!text.fragments[2].1.exact);
            assert_eq!(text.match_span(0..2), Span { end: 15, ..span });
            assert_eq!(
                text.match_span(source.len() + 1..source.len() + 2),
                Span::in_source(SourceId(7), 51, 52)
            );
            assert_eq!(
                text.match_span(source.len() + 3..source.len() + 4),
                Span::in_source(SourceId(7), 60, 80)
            );
            assert_eq!(
                text.into_parts(),
                vec![AttributePart::Text(format!("{source}keyslot"))]
            );
        }
        assert!(Text::default().protect_source().fragments.is_empty());
    }

    #[test]
    fn source_protection_encodes_specials_and_keeps_physical_unicode_spans() {
        for source in ["Ж🙂 &<>\n", "Ж🙂 &amp;\r\n", "&", "<", ">"] {
            let mut text = Text::default();
            let span = Span::in_source(SourceId(4), 10, 10 + source.len());
            text.append_source(source, true, 0, span);
            let text = text.protect_source();
            assert_eq!(
                text.text,
                source
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
            );
            for (range, fragment) in &text.fragments {
                assert_eq!(text.match_span(range.clone()), fragment.span);
                assert_eq!(fragment.owner, 0);
                assert!(!fragment.raw);
                assert_eq!(fragment.span.source_id, span.source_id);
                assert_eq!(
                    &source[fragment.span.start - 10..fragment.span.end - 10],
                    fragment.decoded.as_deref().unwrap_or(&fragment.text)
                );
            }
            assert_eq!(text.into_parts(), vec![AttributePart::Text(source.into())]);
        }
    }

    /// A mixed stream with physical origins, transformed attribute metadata and
    /// a trailing zero-width value. Used to check no-op substitution stages.
    pub(crate) fn marked_text(source: &str) -> Text {
        let mut text = Text::default();
        text.append_source(
            source,
            false,
            3,
            Span {
                provenance: Some(2),
                ..Span::in_source(SourceId(7), 13, 13 + source.len())
            },
        );
        let attribute = Span::in_source(SourceId(8), 50, 57);
        for part in [
            AttributePart::OpenWithAttributes {
                format: AttributeFormat::Strong,
                id: Some("keep".into()),
                role: Some("plain".into()),
            },
            AttributePart::Text("Attribute Ж🙂".into()),
            AttributePart::Close(AttributeFormat::Strong),
            AttributePart::IndexTerm(Box::new(crate::IndexTerm {
                text: None,
                terms: vec!["hidden".into()],
                see: None,
                see_also: vec![],
            })),
        ] {
            text.append_part(&part, 4, attribute);
        }
        text
    }

    pub(crate) fn assert_reused(text: Text, transform: impl FnOnce(Text) -> Text) {
        let expected = text.clone().into_parts();
        let lexical = text.text.clone();
        let owners: Vec<_> = text
            .fragments
            .iter()
            .map(|(r, f)| (r.clone(), f.owner, f.span))
            .collect();
        let text_buffer = text.text.as_ptr();
        let fragment_buffer = text.fragments.as_ptr();
        let result = transform(text);
        assert_eq!(result.text.as_ptr(), text_buffer, "{lexical}");
        assert_eq!(result.fragments.as_ptr(), fragment_buffer, "{lexical}");
        assert_eq!(result.text, lexical);
        assert_eq!(
            result
                .fragments
                .iter()
                .map(|(r, f)| (r.clone(), f.owner, f.span))
                .collect::<Vec<_>>(),
            owners
        );
        assert_eq!(result.into_parts(), expected);
    }

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
