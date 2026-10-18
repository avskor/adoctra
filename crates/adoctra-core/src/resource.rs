//! Host-controlled text and binary resources. Core provides no filesystem or
//! network loader.
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

/// A binary resource (an image embedded as a data URI). `content_type` is a
/// type the source reports (e.g. an HTTP `Content-Type`), if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryResource {
    pub name: String,
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
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

    /// Load the bytes of a resource, resolved like [`ResourceProvider::load`]
    /// (e.g. an image for `data-uri`). The default supports none.
    fn load_binary(&self, request: ResourceRequest<'_>) -> ResourceResult<BinaryResource> {
        Err(ResourceError::new(
            ResourceErrorKind::Unsupported,
            request.target,
            "binary resources are not supported",
        ))
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
    /// The provider does not load this kind of resource.
    Unsupported,
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
    binaries: BTreeMap<String, Vec<u8>>,
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

    /// Insert or replace a binary buffer (e.g. an image), loaded only by
    /// [`ResourceProvider::load_binary`]. Returns the previous bytes, if any.
    pub fn insert_binary(
        &mut self,
        name: &str,
        bytes: impl Into<Vec<u8>>,
    ) -> ResourceResult<Option<Vec<u8>>> {
        let name = virtual_name(ResourceRequest {
            target: name,
            referrer: None,
        })?;
        Ok(self.binaries.insert(name, bytes.into()))
    }

    pub fn remove_binary(&mut self, name: &str) -> ResourceResult<Option<Vec<u8>>> {
        let name = virtual_name(ResourceRequest {
            target: name,
            referrer: None,
        })?;
        Ok(self.binaries.remove(&name))
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

    fn load_binary(&self, request: ResourceRequest<'_>) -> ResourceResult<BinaryResource> {
        let name = virtual_name(request)?;
        let bytes = match (self.binaries.get(&name), self.files.get(&name)) {
            (Some(bytes), _) => bytes.clone(),
            (None, Some(text)) => text.clone().into_bytes(),
            (None, None) => {
                return Err(ResourceError::new(
                    ResourceErrorKind::NotFound,
                    name,
                    "resource not found",
                ))
            }
        };
        Ok(BinaryResource {
            name,
            bytes,
            content_type: None,
        })
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
