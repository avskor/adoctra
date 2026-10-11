//! Parse configuration and attribute state shared by recursive block parsing.

use crate::{Attributes, SourceId};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    /// SourceMap id for this input. The convenience APIs use source zero.
    pub source_id: SourceId,
    /// Hard attribute overrides: document assignments cannot change these keys.
    /// This initial API supports set overrides; soft defaults and locked unsets
    /// will be added alongside the remaining Asciidoctor option semantics.
    pub attributes: Attributes,
    /// Limits for explicit resource parsing; ordinary string parsing performs no loads.
    pub includes: IncludeLimits,
}

pub(crate) struct ParseContext {
    pub attributes: Arc<Attributes>,
    locked: BTreeSet<String>,
    pub comments: Vec<crate::Span>,
    pub comment_diagnostics: Vec<crate::Diagnostic>,
}

impl ParseContext {
    pub fn new(options: &ParseOptions) -> Self {
        let mut attributes = Attributes::new();
        attributes.set("example-caption", "Example");
        let mut locked = BTreeSet::new();
        for (key, value) in options.attributes.iter() {
            attributes.set(key, value);
            locked.insert(key.to_string());
        }
        Self {
            attributes: Arc::new(attributes),
            locked,
            comments: Vec::new(),
            comment_diagnostics: Vec::new(),
        }
    }

    pub fn apply(&mut self, entry: AttributeEntry) {
        let (key, value) = match entry {
            AttributeEntry::Set(key, value) => (key, Some(value)),
            AttributeEntry::Unset(key) => (key, None),
        };
        if self.locked.contains(&key) {
            return;
        }
        let value = value.map(|v| {
            let value = substitute_attribute_value(&v, &self.attributes);
            if key == "leveloffset" {
                let current = self
                    .attributes
                    .get("leveloffset")
                    .map_or(0, crate::include_selection::integer_prefix);
                if let Some(relative) = value.strip_prefix('+') {
                    return current
                        .saturating_add(crate::include_selection::integer_prefix(relative))
                        .to_string();
                }
                if let Some(relative) = value.strip_prefix('-') {
                    return current
                        .saturating_sub(crate::include_selection::integer_prefix(relative))
                        .to_string();
                }
            }
            value
        });
        let attributes = Arc::make_mut(&mut self.attributes);
        match value {
            Some(value) => attributes.set(key, value),
            None => {
                attributes.remove(&key);
            }
        }
    }

    pub fn section_level(&self, level: u8) -> i128 {
        self.attributes
            .get("leveloffset")
            .map_or(0, crate::include_selection::integer_prefix)
            .saturating_add(i128::from(level))
    }
}

#[derive(Debug)]
pub(crate) enum AttributeEntry {
    Set(String, String),
    Unset(String),
}

pub(crate) fn parse_attribute_entry(line: &str) -> Option<AttributeEntry> {
    let rest = line.trim_end().strip_prefix(':')?;
    let (key, value) = rest.split_once(':')?;
    if !value.is_empty() && !value.starts_with([' ', '\t']) {
        return None;
    }
    let (key, unset) = if let Some(key) = key.strip_suffix('!') {
        (key, true)
    } else if let Some(key) = key.strip_prefix('!') {
        (key, true)
    } else {
        (key, false)
    };
    let first = key.chars().next()?;
    if !first.is_alphanumeric() && first != '_' {
        return None;
    }
    // Ruby Parser.sanitize_attribute_name removes punctuation and spaces.
    let key: String = key
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
        .collect();
    let key = key.to_lowercase();
    Some(if unset {
        AttributeEntry::Unset(key)
    } else {
        AttributeEntry::Set(key, value.trim().to_owned())
    })
}

/// Header attribute values substitute attribute references, not inline markup.
/// Substituted strings are not rescanned, which also bounds self references.
pub(crate) fn substitute_attribute_value(value: &str, attributes: &Attributes) -> String {
    let mut result = String::new();
    let mut rest = value;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open + 1..].find('}').map(|i| open + 1 + i) else {
            break;
        };
        if rest[..open].ends_with('\\') {
            result.push_str(&rest[..open - 1]);
            result.push_str(&rest[open..=close]);
        } else {
            result.push_str(&rest[..open]);
            let name = &rest[open + 1..close];
            result.push_str(attributes.get(name).unwrap_or(&rest[open..=close]));
        }
        rest = &rest[close + 1..];
    }
    result.push_str(rest);
    result
}

/// Bounds for recursive resource expansion (the primary source is excluded).
#[derive(Debug, Clone, Copy)]
pub struct IncludeLimits {
    pub max_depth: usize,
    pub max_bytes: usize,
}

impl Default for IncludeLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_bytes: 16 * 1024 * 1024,
        }
    }
}
