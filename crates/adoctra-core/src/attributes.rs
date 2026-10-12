//! Document attributes (the `:name: value` header entries, plus built-ins).

use std::collections::BTreeMap;

/// Attribute text retains whether substitutions protected literal characters or
/// deliberately passed them through. Backends decide how to render each part.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributePart {
    Text(String),
    Raw(String),
}

impl AttributePart {
    pub fn text(&self) -> &str {
        match self {
            Self::Text(text) | Self::Raw(text) => text,
        }
    }

    pub fn is_raw(&self) -> bool {
        matches!(self, Self::Raw(_))
    }
}

pub(crate) fn push_part(parts: &mut Vec<AttributePart>, text: &str, raw: bool) {
    if text.is_empty() {
        return;
    }
    match parts.last_mut() {
        Some(AttributePart::Raw(last)) if raw => last.push_str(text),
        Some(AttributePart::Text(last)) if !raw => last.push_str(text),
        _ => parts.push(if raw {
            AttributePart::Raw(text.to_owned())
        } else {
            AttributePart::Text(text.to_owned())
        }),
    }
}

pub(crate) fn parts_text(parts: &[AttributePart]) -> String {
    parts.iter().map(AttributePart::text).collect()
}

/// Ordered map of document attributes. `BTreeMap` keeps output deterministic,
/// which matters for snapshot/TCK testing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Attributes {
    map: BTreeMap<String, String>,
    parts: BTreeMap<String, Vec<AttributePart>>,
}

impl Attributes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into().to_lowercase();
        self.parts.remove(&key);
        self.map.insert(key, value.into());
    }

    /// Preserve literal/raw boundaries through attribute substitutions.
    pub fn set_parts(&mut self, key: impl Into<String>, parts: Vec<AttributePart>) {
        let key = key.into().to_lowercase();
        let mut merged = Vec::new();
        for part in parts {
            push_part(&mut merged, part.text(), part.is_raw());
        }
        self.set(&key, parts_text(&merged));
        if merged.iter().any(AttributePart::is_raw) {
            self.parts.insert(key, merged);
        }
    }

    /// Raw boundaries, when present; absent means the entire value is literal text.
    pub fn raw_parts(&self, key: &str) -> Option<&[AttributePart]> {
        self.parts.get(&key.to_lowercase()).map(Vec::as_slice)
    }

    pub(crate) fn value_parts(&self, key: &str) -> Option<Vec<AttributePart>> {
        self.get(key).map(|text| {
            self.raw_parts(key)
                .map_or_else(|| vec![AttributePart::Text(text.to_owned())], <[_]>::to_vec)
        })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(&key.to_lowercase()).map(String::as_str)
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.parts.remove(&key.to_lowercase());
        self.map.remove(&key.to_lowercase())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.map.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The reference compares the protected attribute to unprotected declarations.
/// This temporary lexical view is not stored in the AST or rendered by core.
pub(crate) fn reference_text(parts: &[AttributePart]) -> String {
    let mut result = String::new();
    for part in parts {
        for ch in part.text().chars() {
            if part.is_raw() {
                result.push(ch);
            } else {
                result.push_str(match ch {
                    '<' => "&lt;",
                    '>' => "&gt;",
                    '&' => "&amp;",
                    _ => {
                        result.push(ch);
                        continue;
                    }
                });
            }
        }
    }
    result
}
