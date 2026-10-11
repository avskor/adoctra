//! Host-controlled text resources. Core provides no filesystem or network loader.
//!
//! Providers resolve a target relative to the referring resource's canonical
//! name and return an unmodified UTF-8 snapshot. Include selection, normalization,
//! attribute substitution and recursion belong to the reader, not the provider.
use std::{collections::BTreeMap, fmt};

/// A request after target attribute substitution. `referrer` is a canonical
/// name returned by this same provider, or None for a top-level resource.
#[derive(Debug, Clone, Copy)]
pub struct ResourceRequest<'a> {
    pub target: &'a str,
    pub referrer: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// Canonical identity within the provider. Nested requests use this name
    /// as their referrer; aliases must resolve to the same name.
    pub name: String,
    /// Original UTF-8 text, including its BOM and line endings, if present.
    pub text: String,
}

pub trait ResourceProvider {
    /// Load one snapshot. Missing and denied resources must be distinguished,
    /// so optional includes can suppress only the appropriate failures.
    /// Providers own access policy; the parser must not fall back to ambient I/O.
    fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource>;

    /// Opt into document path attributes for a registered canonical name.
    /// Return an absolute, slash-separated file path in this provider's
    /// namespace (which may be virtual). This must not load the resource again.
    /// The default leaves opaque identities alone. The path is metadata only;
    /// include resolution still uses the original canonical name as referrer.
    fn document_path(&self, _name: &str) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResourceErrorKind {
    NotFound,
    AccessDenied,
    InvalidTarget,
    InvalidEncoding,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceError {
    pub kind: ResourceErrorKind,
    pub target: String,
    pub message: String,
}

impl ResourceError {
    pub fn new(
        kind: ResourceErrorKind,
        target: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            target: target.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.target, self.message)
    }
}

impl std::error::Error for ResourceError {}

pub type ResourceResult<T> = std::result::Result<T, ResourceError>;

/// Application-owned resources in a virtual directory tree. Names use `/` on
/// every platform; an initial slash addresses the virtual root. `.` and `..`
/// are normalized, but traversal above that root is rejected. No URL decoding,
/// filesystem access or network access is performed.
#[derive(Debug, Clone, Default)]
pub struct MemoryResources {
    files: BTreeMap<String, String>,
}

impl MemoryResources {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a UTF-8 buffer. Returns the previous text, if any.
    pub fn insert(
        &mut self,
        name: &str,
        text: impl Into<String>,
    ) -> ResourceResult<Option<String>> {
        let name = virtual_name(ResourceRequest {
            target: name,
            referrer: None,
        })?;
        Ok(self.files.insert(name, text.into()))
    }

    pub fn remove(&mut self, name: &str) -> ResourceResult<Option<String>> {
        let name = virtual_name(ResourceRequest {
            target: name,
            referrer: None,
        })?;
        Ok(self.files.remove(&name))
    }
}

impl ResourceProvider for MemoryResources {
    fn document_path(&self, name: &str) -> Option<String> {
        Some(name.to_owned())
    }

    fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
        let name = virtual_name(request)?;
        match self.files.get(&name) {
            Some(text) => Ok(Resource {
                name,
                text: text.clone(),
            }),
            None => Err(ResourceError::new(
                ResourceErrorKind::NotFound,
                name,
                "resource not found",
            )),
        }
    }
}

fn virtual_name(request: ResourceRequest<'_>) -> ResourceResult<String> {
    let target = request.target;
    // A virtual resource is a file, not a directory or an OS/URI path.
    if target.is_empty() || target.ends_with('/') || target.contains(['\\', '\0', ':']) {
        return Err(ResourceError::new(
            ResourceErrorKind::InvalidTarget,
            target,
            "expected a virtual file path using '/' separators",
        ));
    }
    let mut parts = Vec::new();
    if !target.starts_with('/') {
        if let Some(referrer) = request.referrer {
            let parent = referrer.rsplit_once('/').map_or("", |(parent, _)| parent);
            parts.extend(parent.split('/').filter(|part| !part.is_empty()));
        }
    }
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(ResourceError::new(
                        ResourceErrorKind::AccessDenied,
                        target,
                        "path escapes the virtual root",
                    ));
                }
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() || matches!(target.rsplit('/').next(), Some("." | "..")) {
        return Err(ResourceError::new(
            ResourceErrorKind::InvalidTarget,
            target,
            "expected a file name",
        ));
    }
    Ok(format!("/{}", parts.join("/")))
}
