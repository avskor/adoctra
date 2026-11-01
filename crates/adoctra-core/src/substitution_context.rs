//! Mutable attribute evaluation is scoped to a real execution phase: reading
//! definitions or evaluating body content. Syntax-only scans borrow a read-only
//! view and cannot repeat side effects from either phase.
use crate::{AttributePart as P, Attributes, Diagnostic, Span};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    ops::Deref,
};

pub(crate) struct Evaluation<'a> {
    pub bibliography: bool,
    attributes: Cow<'a, Attributes>,
    compat: bool,
    state: Option<(&'a BTreeSet<String>, &'a mut BTreeMap<String, String>)>,
    xrefs: Option<crate::xref_target::Scope<'a>>,
    /// Inside a nested document, Ruby's `counter` delegates to the root
    /// document: its attributes and locks.
    root: Option<(&'a mut Attributes, &'a BTreeSet<String>)>,
    /// The document's footnote registry while fields convert (Ruby's
    /// `Document#footnotes`); without it footnote macros stay text.
    footnotes: Option<&'a mut Footnotes>,
    /// Image reads for `data-uri` while converting.
    images: Option<&'a crate::data_uri::Images<'a>>,
    /// Cross-reference labels while fields convert.
    labels: Option<&'a mut (dyn Labels + 'a)>,
    /// Ruby's `Document#compat_mode` where it differs from the attribute
    /// (reading keeps the header's; conversion plays entries back).
    quote_mode: Option<bool>,
}

/// Ruby converts a cross-reference inside the macros stage: the target's
/// natural lookup and a textless reference's label (which may convert the
/// target's title) happen there, before the field's later macros. The
/// converter receives the evaluation's state for that conversion.
pub(crate) trait Labels {
    fn label(
        &mut self,
        state: Lent<'_>,
        xref: &mut crate::Xref,
        textless: bool,
        span: Span,
    ) -> Option<Vec<crate::Inline>>;
}

/// The state of an evaluation lent to a label conversion.
pub(crate) struct Lent<'b> {
    pub attributes: &'b mut Attributes,
    pub counters: &'b mut BTreeMap<String, String>,
    pub root: Option<&'b mut Attributes>,
    pub footnotes: &'b mut Footnotes,
}

/// Registered footnote ids (first wins) and the registration order.
#[derive(Clone, Default, Debug)]
pub(crate) struct Footnotes {
    ids: Vec<(String, String)>,
    definitions: Vec<crate::ast::FootnoteDefinition>,
}

impl Footnotes {
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Deferred inline fields first register with local coordinates. Once the
    /// field's source mapping is restored, retain those same reader origins.
    pub fn restore_origins(&mut self, nodes: &mut [crate::Inline], first: usize) {
        struct Restore<'a>(&'a mut Footnotes, usize);
        impl crate::visit::VisitorMut for Restore<'_> {
            fn visit_inline(&mut self, inline: &mut crate::Inline) {
                inline_footnotes(inline, &mut |footnote, span| {
                    if let Some(sequence) = footnote.sequence.filter(|&i| i >= self.1) {
                        if let Some(definition) = self.0.definitions.get_mut(sequence) {
                            definition.span = span;
                        }
                    }
                });
                crate::visit::walk_inline_mut(self, inline);
            }
        }
        crate::visit::walk_inlines_mut(&mut Restore(self, first), nodes);
    }

    pub fn find(&self, id: &str) -> Option<&str> {
        self.ids
            .iter()
            .find(|(candidate, _)| candidate == id)
            .map(|(_, index)| index.as_str())
    }

    /// Register a definition; returns its registration sequence.
    pub fn register(&mut self, id: Option<&str>, index: &str, content: &[P], span: Span) -> usize {
        if let Some(id) = id {
            self.ids.push((id.to_owned(), index.to_owned()));
        }
        self.definitions.push(crate::ast::FootnoteDefinition {
            index: index.to_owned(),
            id: id.map(str::to_owned),
            content: content.to_vec(),
            span,
        });
        self.definitions.len() - 1
    }

    /// Passthrough placeholders are restored after macro registration.
    pub fn restore(&mut self, footnote: &crate::Footnote) {
        if let Some(definition) = footnote.sequence.and_then(|i| self.definitions.get_mut(i)) {
            definition.content.clone_from(&footnote.content);
        }
    }

    pub fn into_definitions(
        self,
        fixed: &crate::resolve::FixedXrefs,
    ) -> Vec<crate::ast::FootnoteDefinition> {
        let mut definitions = self.definitions;
        for definition in &mut definitions {
            crate::resolve::resolve_definition_parts(&mut definition.content, fixed);
        }
        definitions
    }
}

/// Footnotes owned by one inline node, including semantic parts in UI labels.
/// The caller handles traversal of ordinary inline children.
pub(crate) fn inline_footnotes(inline: &crate::Inline, f: &mut impl FnMut(&crate::Footnote, Span)) {
    fn visit_parts(parts: &[P], span: Span, f: &mut impl FnMut(&crate::Footnote, Span)) {
        for part in parts {
            match part {
                P::Footnote(footnote) => f(footnote, span),
                P::Ui(ui) => {
                    for label in ui.clone().labels_mut() {
                        visit_parts(label, span, f);
                    }
                }
                P::Stem(stem) => visit_parts(&stem.parts, span, f),
                P::IndexTerm(term) => {
                    for label in term.clone().labels_mut() {
                        visit_parts(label, span, f);
                    }
                }
                _ => {}
            }
        }
    }
    match inline {
        crate::Inline::Footnote { footnote, span } => f(footnote, *span),
        crate::Inline::AttributeRef {
            parts: value, span, ..
        }
        | crate::Inline::Substituted {
            parts: value, span, ..
        } => visit_parts(value, *span, f),
        crate::Inline::Ui { ui, span } => {
            for label in ui.clone().labels_mut() {
                visit_parts(label, *span, f);
            }
        }
        crate::Inline::Stem { stem, span } => visit_parts(&stem.parts, *span, f),
        crate::Inline::IndexTerm { term, span } => {
            for label in term.clone().labels_mut() {
                visit_parts(label, *span, f);
            }
        }
        _ => {}
    }
}

impl Deref for Evaluation<'_> {
    type Target = Attributes;
    fn deref(&self) -> &Attributes {
        &self.attributes
    }
}

pub(crate) struct Effect {
    pub parts: Vec<P>,
    pub remove_empty: bool,
    pub remove_line: bool,
}

impl<'a> Evaluation<'a> {
    pub fn read_only(attributes: &'a Attributes) -> Self {
        Self {
            bibliography: false,
            attributes: Cow::Borrowed(attributes),
            state: None,
            compat: false,
            xrefs: None,
            root: None,
            footnotes: None,
            images: None,
            labels: None,
            quote_mode: None,
        }
    }
    pub fn definition(
        attributes: &'a Attributes,
        locked: &'a BTreeSet<String>,
        counters: &'a mut BTreeMap<String, String>,
        compat: bool,
    ) -> Self {
        Self {
            bibliography: false,
            attributes: Cow::Borrowed(attributes),
            state: Some((locked, counters)),
            compat,
            xrefs: None,
            root: None,
            footnotes: None,
            images: None,
            labels: None,
            quote_mode: None,
        }
    }
    /// Quote and link rules in this compat mode instead of the attribute's.
    pub fn with_quote_mode(mut self, compat: bool) -> Self {
        self.quote_mode = Some(compat);
        self
    }
    /// Ruby's `compat_mode` for quote and link rules.
    pub fn quote_compat(&self) -> bool {
        self.quote_mode
            .unwrap_or_else(|| self.get("compat-mode").is_some())
    }
    /// Footnote macros register in this document's footnotes.
    pub fn with_footnotes(mut self, footnotes: &'a mut Footnotes) -> Self {
        self.footnotes = Some(footnotes);
        self
    }
    pub fn footnotes(&mut self) -> Option<&mut Footnotes> {
        self.footnotes.as_deref_mut()
    }
    pub fn with_images(mut self, images: Option<&'a crate::data_uri::Images<'a>>) -> Self {
        self.images = images;
        self
    }
    /// Ruby's `image_uri` with the attributes at this point; without image
    /// reads (no conversion context) the web path.
    pub fn image_uri(
        &self,
        target: &str,
        dir: Option<&str>,
        span: crate::Span,
        diagnostics: &mut Vec<crate::Diagnostic>,
    ) -> String {
        match self.images {
            Some(images) => images.uri(target, dir, &self.attributes, span, diagnostics),
            None => crate::image::web_path(target, dir),
        }
    }
    /// Ruby's SVG embedding of an image at this point (see
    /// [`crate::data_uri::Images::svg`]).
    pub fn svg(
        &self,
        target: &str,
        named: &BTreeMap<String, String>,
        options: &[String],
        span: crate::Span,
        diagnostics: &mut Vec<crate::Diagnostic>,
    ) -> Option<crate::Svg> {
        crate::data_uri::Images::svg(
            self.images,
            target,
            named,
            options,
            &self.attributes,
            span,
            diagnostics,
        )
    }
    /// Counters of a nested document read and update the root document.
    pub fn with_root(mut self, root: Option<(&'a mut Attributes, &'a BTreeSet<String>)>) -> Self {
        self.root = root;
        self
    }
    /// Interdocument references see the includes read so far and the pinned
    /// `outfilesuffix`.
    pub fn with_xrefs(mut self, scope: crate::xref_target::Scope<'a>) -> Self {
        self.xrefs = Some(scope);
        self
    }
    pub fn xrefs(&self) -> Option<crate::xref_target::Scope<'a>> {
        self.xrefs
    }
    /// Cross-references convert their labels in the macros stage.
    pub fn with_labels(mut self, labels: &'a mut (dyn Labels + 'a)) -> Self {
        self.labels = Some(labels);
        self
    }
    /// Resolve a cross-reference in the macros stage: `None` without a
    /// converter (reading, syntax scans), else the label of a textless one.
    pub fn label(
        &mut self,
        xref: &mut crate::Xref,
        textless: bool,
        span: Span,
    ) -> Option<Option<Vec<crate::Inline>>> {
        let Evaluation {
            labels: Some(labels),
            attributes,
            state: Some((_, counters)),
            root,
            footnotes: Some(footnotes),
            ..
        } = self
        else {
            return None;
        };
        let state = Lent {
            attributes: attributes.to_mut(),
            counters,
            root: root.as_mut().map(|(attributes, _)| &mut **attributes),
            footnotes,
        };
        Some(labels.label(state, xref, textless, span))
    }
    pub fn changed_attributes(self) -> Option<Attributes> {
        match self.attributes {
            Cow::Owned(attrs) => Some(attrs),
            Cow::Borrowed(_) => None,
        }
    }
    pub fn stateful(&self) -> bool {
        self.state.is_some()
    }

    pub fn execute(
        &mut self,
        expression: &str,
        span: Span,
        undefined_policy: &mut Option<String>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Effect {
        let mut args = expression.splitn(3, ':');
        let operation = args.next().unwrap();
        let name = args.next().unwrap();
        let seed = args.next();
        if operation == "set" {
            let unset = name.starts_with('!') || name.ends_with('!');
            let name = name
                .strip_suffix('!')
                .or_else(|| name.strip_prefix('!'))
                .unwrap_or(name);
            let name: String = name
                .chars()
                .filter(|&ch| crate::unicode_word::is_word(ch) || ch == '-')
                .collect::<String>()
                .to_lowercase();
            let name = match name.as_str() {
                "hardbreaks" => "hardbreaks-option",
                "numbered" => "sectnums",
                _ => &name,
            };
            if name == "showtitle" && !self.state.as_ref().unwrap().0.contains("notitle") {
                if unset {
                    self.attributes.to_mut().set("notitle", "");
                } else {
                    self.attributes.to_mut().remove("notitle");
                }
            }
            if !self.state.as_ref().unwrap().0.contains(name) {
                if unset {
                    self.attributes.to_mut().remove(name);
                } else {
                    let mut value = seed.unwrap_or("").to_owned();
                    if name == "leveloffset" && value.starts_with(['+', '-']) {
                        let current = self
                            .get(name)
                            .map_or(0, crate::include_selection::integer_prefix);
                        value = current
                            .saturating_add(crate::include_selection::integer_prefix(&value))
                            .to_string();
                    }
                    let compat = self.compat;
                    let parts = crate::context::attribute_entry_parts_report(
                        &value,
                        self,
                        compat,
                        span,
                        diagnostics,
                    );
                    self.attributes.to_mut().set_parts(name, parts);
                }
            }
            // Unsetting suppresses the whole transformed line by default,
            // even when the target is locked and deletion was ignored.
            return Effect {
                parts: Vec::new(),
                remove_empty: true,
                remove_line: unset
                    && undefined_policy.get_or_insert_with(|| {
                        self.get("attribute-undefined")
                            .unwrap_or("drop-line")
                            .to_owned()
                    }) == "drop-line",
            };
        }
        let locked = match &self.root {
            Some((_, locked)) => locked.contains(name),
            None => self.state.as_ref().unwrap().0.contains(name),
        };
        let value = |this: &Self| match &this.root {
            Some((attributes, _)) => attributes.counter_value(name),
            None => this.counter_value(name),
        };
        let current = if locked {
            self.state
                .as_ref()
                .unwrap()
                .1
                .get(name)
                .cloned()
                .or_else(|| value(self).filter(|s| !s.is_empty()))
        } else {
            value(self).filter(|s| !s.is_empty())
        };
        let next = current.map_or_else(
            || seed.unwrap_or("1").to_owned(),
            |value| next_value(&value),
        );
        self.state
            .as_mut()
            .unwrap()
            .1
            .insert(name.into(), next.clone());
        if !locked {
            match &mut self.root {
                Some((attributes, _)) => attributes.set_counter(name, &next),
                None => self.attributes.to_mut().set_counter(name, &next),
            }
        }
        Effect {
            parts: if operation == "counter2" {
                Vec::new()
            } else {
                vec![P::Raw(next)]
            },
            remove_empty: operation == "counter2",
            remove_line: false,
        }
    }
}

/// The operation prefix is case sensitive; its nonempty argument may include
/// spaces, punctuation and newlines. The first right brace after at least one
/// argument character closes the expression (even if that character is `}`).
pub(crate) fn expression<'a>(
    src: &'a str,
    at: usize,
    closes: &[usize],
) -> Option<(usize, &'a str, bool)> {
    let leading = src[at..].starts_with("\\{");
    let start = at + usize::from(leading);
    let body = src[start..].strip_prefix('{')?;
    let prefix = ["set:", "counter:", "counter2:"]
        .into_iter()
        .find(|p| body.starts_with(p))?;
    let first = start + 1 + prefix.len() + 1;
    let close = *closes.get(closes.partition_point(|&end| end < first))? - start - 1;
    let mut value = &body[..close];
    let trailing = value.ends_with('\\') && value.len() > prefix.len() + 1;
    if trailing {
        value = &value[..value.len() - 1];
    }
    (value.len() > prefix.len()).then_some((start + close + 2, value, leading || trailing))
}

/// Canonical integer counters are unbounded decimal strings, matching Ruby's
/// integer arithmetic without overflow or a new core dependency.
fn next_value(value: &str) -> String {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let integer = !digits.is_empty()
        && digits.bytes().all(|c| c.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && value != "-0";
    if integer && value.starts_with('-') {
        let mut out = digits.as_bytes().to_vec();
        for ch in out.iter_mut().rev() {
            if *ch > b'0' {
                *ch -= 1;
                break;
            }
            *ch = b'9';
        }
        let text = String::from_utf8(out).unwrap();
        let text = text.trim_start_matches('0');
        return if text.is_empty() {
            "0".into()
        } else {
            format!("-{text}")
        };
    }
    successor(value)
}

fn successor(value: &str) -> String {
    let mut chars: Vec<char> = value.chars().collect();
    let mut insertion = None;
    let mut carry = '1';
    let mut gap = false;
    let mut previous_digit = None;
    for i in (0..chars.len()).rev() {
        let c = chars[i];
        let digit = crate::unicode_word::is_decimal(c);
        if !crate::unicode_word::is_alphanumeric(c) {
            gap = true;
            continue;
        }
        if gap && previous_digit.is_some_and(|d| d != digit) {
            break;
        }
        gap = false;
        previous_digit = Some(digit);
        insertion = Some(i);
        if let Some(next) = neighbor(c, digit, true) {
            chars[i] = next;
            return chars.into_iter().collect();
        }
        let mut first = c;
        while let Some(prev) = neighbor(first, digit, false) {
            first = prev;
        }
        chars[i] = first;
        carry = if digit {
            char::from_u32(first as u32 + 1).unwrap()
        } else {
            first
        };
    }
    if let Some(i) = insertion {
        chars.insert(i, carry);
    } else {
        // Ruby retains the UTF-8 width on non-alphanumeric carry. Surrogates
        // are skipped; exhausting a width prepends a scalar carry of U+0001.
        for i in (0..chars.len()).rev() {
            let c = chars[i];
            let next = if c == '\u{d7ff}' {
                Some('\u{e000}')
            } else {
                char::from_u32(c as u32 + 1)
            };
            if let Some(next) = next.filter(|n| n.len_utf8() == c.len_utf8()) {
                chars[i] = next;
                return chars.into_iter().collect();
            }
            chars[i] = match c.len_utf8() {
                1 => '\0',
                2 => '\u{80}',
                3 => '\u{800}',
                _ => '\u{10000}',
            };
        }
        if !chars.is_empty() {
            chars.insert(0, '\u{1}');
        }
    }
    chars.into_iter().collect()
}

fn neighbor(c: char, digit: bool, forward: bool) -> Option<char> {
    // Ruby permits a one-code-point gap inside an alphabetic range (Ö -> Ø).
    // Digits use their Unicode decimal run, including contiguous math alphabets.
    (1..=2).find_map(|step| {
        let value = if forward {
            (c as u32).checked_add(step)
        } else {
            (c as u32).checked_sub(step)
        }?;
        let next = char::from_u32(value)?;
        (next.len_utf8() == c.len_utf8()
            && if digit {
                crate::unicode_word::is_decimal(next)
            } else {
                next.is_alphabetic()
            })
        .then_some(next)
    })
}
