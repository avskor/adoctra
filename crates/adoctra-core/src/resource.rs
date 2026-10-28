//! Host-controlled text and binary resources. Core provides no filesystem or
//! network loader.
//!
//! Providers resolve a target relative to the referring resource's canonical
//! name and return a UTF-8 snapshot. Include selection, normalization,
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
    /// UTF-8 text, including its BOM and line endings, if present. For an encoded
    /// include this is the decoded snapshot; spans address these UTF-8 bytes,
    /// not the byte offsets in the original encoded file.
    pub text: String,
}

/// An include's immutable decoded source. A finite line selection may recover
/// an intact prefix when the unread tail cannot be decoded. The prefix starts
/// at byte zero, so physical line numbers and decoded UTF-8 offsets stay valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeResource {
    pub resource: Resource,
    /// False when this resource is an incomplete decoded view. No replacement
    /// characters or fabricated lines enter the snapshot text.
    pub complete: bool,
}

/// A whole undecodable line excluded from an include snapshot. `offset` is
/// the insertion point in its retained UTF-8 text; `line` is the original
/// one-based physical line. Bytes include the original line ending, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmittedSourceLine {
    pub offset: usize,
    pub line: usize,
    pub bytes: Vec<u8>,
}

/// A byte range within an omitted physical line, including neither an invented
/// UTF-8 decoding nor a Unicode column. The source ID is the diagnostic's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OmittedSourceSpan {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

pub(crate) struct IncludeSnapshot {
    pub decoded: IncludeResource,
    pub omissions: Vec<OmittedSourceLine>,
    pub frames: Option<SourceLineFrames>,
}

/// Reader IO can split an encoded character's LF byte without a Unicode LF.
/// Keep those physical boundaries separately; never invent snapshot text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceLineFrames {
    pub ranges: Vec<(usize, usize)>,
    pub trailing_lf: bool,
}

impl From<IncludeResource> for IncludeSnapshot {
    fn from(decoded: IncludeResource) -> Self {
        Self {
            decoded,
            omissions: Vec::new(),
            frames: None,
        }
    }
}

impl From<Resource> for IncludeSnapshot {
    fn from(resource: Resource) -> Self {
        IncludeResource::from(resource).into()
    }
}

/// Selection sees physical line slots; undecodable slots are empty internal
/// sentinels and must be discarded before normalization or parsing. They are
/// never inserted into the exported source text.
pub(crate) fn snapshot_lines(
    text: &str,
    omissions: &[OmittedSourceLine],
) -> Vec<crate::include_indent::Line> {
    framed_snapshot_lines(text, omissions, None)
}

pub(crate) fn framed_snapshot_lines(
    text: &str,
    omissions: &[OmittedSourceLine],
    frames: Option<&SourceLineFrames>,
) -> Vec<crate::include_indent::Line> {
    use crate::include_indent::Line;
    let strip_bom = omissions.first().map_or(true, |line| line.line != 1);
    let lines = if let Some(frames) = frames {
        frames
            .ranges
            .iter()
            .enumerate()
            .map(|(index, &(start, end))| {
                Line::from_bounds(text, start, end, strip_bom && index == 0)
            })
            .collect()
    } else {
        Line::from_source_with_bom(text, strip_bom)
    };
    if omissions.is_empty() {
        return lines;
    }
    let count = lines.len() + omissions.len();
    let mut lines = lines.into_iter();
    let mut omitted = omissions.iter().peekable();
    (1..=count)
        .map(|number| {
            if omitted.peek().is_some_and(|line| line.line == number) {
                let omission = omitted.next().expect("omitted line");
                let mut line = Line::generated(String::new());
                line.original = Some(omission.offset);
                line
            } else {
                lines.next().expect("retained physical line")
            }
        })
        .collect()
}

/// Provider input awaiting the reader's line/tag selection. Only UTF-8 and
/// ASCII-8BIT defer validation; transcoding codecs validate consumed bytes.
#[derive(Debug, Clone)]
pub enum IncludeInput {
    Decoded(IncludeResource),
    Deferred {
        resource: BinaryResource,
        binary: bool,
    },
}

impl From<Resource> for IncludeResource {
    fn from(resource: Resource) -> Self {
        Self {
            resource,
            complete: true,
        }
    }
}

/// Original resource bytes (an encoded include or an embedded image). `content_type` is a
/// type the source reports (e.g. an HTTP `Content-Type`), if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryResource {
    pub name: String,
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

pub trait ResourceProvider {
    /// Enable URI caching for this provider's session. Called only for an
    /// allowed URI read with `cache-uri`; removing the attribute does not
    /// disable an already enabled cache, like Ruby's OpenURI adapter.
    /// Providers choose cache storage/lifetime and retain access policy.
    /// The default leaves existing host providers unchanged.
    fn enable_uri_cache(&self) {}

    /// Load one snapshot. Missing and denied resources must be distinguished,
    /// so optional includes can suppress only the appropriate failures.
    /// Providers own access policy; the parser must not fall back to ambient I/O.
    fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource>;

    /// Load an include with its declared external encoding, before line/tag
    /// selection. The default keeps existing text-only providers working:
    /// without a selector it uses `load`; with a selector it interprets that
    /// snapshot's UTF-8 bytes as the requested encoding. Providers with original
    /// bytes should override this method and decode those bytes instead.
    /// Hosts may implement additional codecs here. Access policy and canonical
    /// identity must be the same as for `load`; no ambient I/O fallback exists.
    fn load_with_encoding(
        &self,
        request: ResourceRequest<'_>,
        encoding: Option<&str>,
    ) -> ResourceResult<Resource> {
        let resource = self.load(request)?;
        if encoding.is_none() {
            return Ok(resource);
        }
        BinaryResource {
            name: resource.name,
            bytes: resource.text.into_bytes(),
            content_type: None,
        }
        .decode(encoding)
    }

    /// Load an include, optionally stopping decoding after this many physical
    /// LF-terminated lines. `line_limit` is set only for a finite, positive
    /// `lines` selection; tag selection and open ranges require the whole file.
    /// A complete snapshot is preferred when the entire resource is valid.
    /// Existing host decoders keep their behavior through the default method.
    /// Byte providers can opt in using [`BinaryResource::decode_include`].
    /// This is a decoding bound, not permission to bypass access policy.
    fn load_include(
        &self,
        request: ResourceRequest<'_>,
        encoding: Option<&str>,
        line_limit: Option<usize>,
    ) -> ResourceResult<IncludeResource> {
        let _ = line_limit;
        self.load_with_encoding(request, encoding).map(Into::into)
    }

    /// Prepare input for reader-owned selection. The default preserves existing
    /// custom decoders. Original-byte providers may defer UTF-8/BINARY checks
    /// using [`BinaryResource::prepare_include`]; the reader validates selected
    /// lines before registering a snapshot. Access policy remains the provider's.
    fn prepare_include(
        &self,
        request: ResourceRequest<'_>,
        encoding: Option<&str>,
        line_limit: Option<usize>,
    ) -> ResourceResult<IncludeInput> {
        self.load_include(request, encoding, line_limit)
            .map(IncludeInput::Decoded)
    }

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

    /// Insert or replace original bytes (e.g. an image or encoded include).
    /// `load_binary` and `load_with_encoding` prefer these bytes to a text
    /// buffer with the same name. Returns the previous bytes, if any.
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
    fn prepare_include(
        &self,
        request: ResourceRequest<'_>,
        encoding: Option<&str>,
        line_limit: Option<usize>,
    ) -> ResourceResult<IncludeInput> {
        self.load_binary(request)?
            .prepare_include(encoding, line_limit)
    }

    fn load_include(
        &self,
        request: ResourceRequest<'_>,
        encoding: Option<&str>,
        line_limit: Option<usize>,
    ) -> ResourceResult<IncludeResource> {
        self.load_binary(request)?
            .decode_include(encoding, line_limit)
    }

    fn load_with_encoding(
        &self,
        request: ResourceRequest<'_>,
        encoding: Option<&str>,
    ) -> ResourceResult<Resource> {
        self.load_binary(request)?.decode(encoding)
    }

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
