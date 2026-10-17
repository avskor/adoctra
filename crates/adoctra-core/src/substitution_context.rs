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
    attributes: Cow<'a, Attributes>,
    compat: bool,
    state: Option<(&'a BTreeSet<String>, &'a mut BTreeMap<String, String>)>,
    xrefs: Option<crate::xref_target::Scope<'a>>,
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
            attributes: Cow::Borrowed(attributes),
            state: None,
            compat: false,
            xrefs: None,
        }
    }
    pub fn definition(
        attributes: &'a Attributes,
        locked: &'a BTreeSet<String>,
        counters: &'a mut BTreeMap<String, String>,
        compat: bool,
    ) -> Self {
        Self {
            attributes: Cow::Borrowed(attributes),
            state: Some((locked, counters)),
            compat,
            xrefs: None,
        }
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
        let locked = self.state.as_ref().unwrap().0.contains(name);
        let current = if locked {
            self.state
                .as_ref()
                .unwrap()
                .1
                .get(name)
                .cloned()
                .or_else(|| self.counter_value(name).filter(|s| !s.is_empty()))
        } else {
            self.counter_value(name).filter(|s| !s.is_empty())
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
            self.attributes.to_mut().set_counter(name, &next);
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
