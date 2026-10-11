//! Document attributes (the `:name: value` header entries, plus built-ins).

use std::collections::BTreeMap;

/// Ordered map of document attributes. `BTreeMap` keeps output deterministic,
/// which matters for snapshot/TCK testing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Attributes {
    map: BTreeMap<String, String>,
}

impl Attributes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.map.insert(key.into().to_lowercase(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(&key.to_lowercase()).map(String::as_str)
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.map.remove(&key.to_lowercase())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.map.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}
