//! Native filesystem adapter; the parser and WASM never import this module.
use adoctra::{
    BinaryResource, IncludeInput, IncludeResource, Resource, ResourceError, ResourceErrorKind,
    ResourceProvider, ResourceRequest, ResourceResult,
};
use std::path::{Path, PathBuf};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// The OpenURI cache of one CLI run. open-uri-cached patches OpenURI for the
/// whole Ruby process, so once a document enables `cache-uri` every later
/// URI read of the run (other inputs too) goes through it. `None` until
/// enabled; HTTP responses are never shared beyond the handle's holders.
#[derive(Clone, Default)]
pub(crate) struct UriCache(Arc<Mutex<Option<BTreeMap<String, BinaryResource>>>>);

impl UriCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<BTreeMap<String, BinaryResource>>> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }
}

pub(crate) struct FileResources {
    root: PathBuf,
    /// Confine paths to the root (safe mode and above); unsafe mode reads
    /// any path, like Ruby without a jail.
    jail: bool,
    /// The primary input, which Ruby reads by its path even outside the base
    /// directory (`-B`); every other path stays confined to the root.
    input: Option<PathBuf>,
    /// OpenURI caching is sticky once enabled. Shared only with providers
    /// given the same handle ([`Self::with_uri_cache`]).
    uri_cache: UriCache,
}

/// Ruby's `uriish?`: a scheme of two or more characters and a colon.
fn uriish(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme.chars().next().is_some_and(|c| c.is_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '.' | '+' | '-'))
    })
}

/// Read a URI (the core asks only with `allow-uri-read`), like Ruby's
/// `open-uri`, with the response's content type.
fn fetch(uri: &str) -> ResourceResult<BinaryResource> {
    let scheme = uri
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(ResourceError::new(
            ResourceErrorKind::InvalidTarget,
            uri,
            "unsupported URI scheme",
        ));
    }
    let response = ureq::get(uri).call().map_err(|error| {
        let kind = match &error {
            ureq::Error::Status(404 | 410, _) => ResourceErrorKind::NotFound,
            ureq::Error::Status(401 | 403, _) => ResourceErrorKind::AccessDenied,
            _ => ResourceErrorKind::Io,
        };
        ResourceError::new(kind, uri, error.to_string())
    })?;
    let content_type = Some(response.content_type().to_owned());
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes)
        .map_err(|error| io_error(uri, error))?;
    Ok(BinaryResource {
        name: uri.to_owned(),
        bytes,
        content_type,
    })
}

/// A text resource from loaded bytes.
fn text(resource: BinaryResource) -> ResourceResult<Resource> {
    let text = String::from_utf8(resource.bytes).map_err(|_| {
        ResourceError::new(
            ResourceErrorKind::InvalidEncoding,
            &resource.name,
            "resource is not UTF-8",
        )
    })?;
    Ok(Resource {
        name: resource.name,
        text,
    })
}

impl FileResources {
    /// An existing directory as the root (tests).
    #[cfg(test)]
    pub fn new(root: impl AsRef<Path>) -> ResourceResult<Self> {
        let input = root.as_ref();
        let root = input
            .canonicalize()
            .map_err(|e| io_error(&input.to_string_lossy(), e))?;
        if !root.is_dir() {
            return Err(ResourceError::new(
                ResourceErrorKind::InvalidTarget,
                input.to_string_lossy(),
                "resource root is not a directory",
            ));
        }
        Ok(Self {
            root,
            jail: true,
            input: None,
            uri_cache: UriCache::default(),
        })
    }

    /// Ruby's `base_dir` as the root: like `File.expand_path`, it need not
    /// exist or be a directory (nothing then resolves under it). An existing
    /// root is canonical, as reads compare canonical paths.
    pub fn base_dir(root: &Path) -> Self {
        Self {
            root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            jail: true,
            input: None,
            uri_cache: UriCache::default(),
        }
    }

    /// Read URIs through the run's shared OpenURI cache.
    pub fn with_uri_cache(mut self, cache: UriCache) -> Self {
        self.uri_cache = cache;
        self
    }

    /// Also allow the explicitly requested primary file (canonical path).
    pub fn with_input(mut self, input: PathBuf) -> Self {
        self.input = Some(input);
        self
    }

    /// Read paths outside the root too (unsafe mode).
    pub fn unjailed(mut self) -> Self {
        self.jail = false;
        self
    }

    fn uri(request: ResourceRequest<'_>) -> Option<String> {
        if uriish(request.target) {
            Some(request.target.to_owned())
        } else {
            request.referrer.filter(|name| uriish(name)).map(|name| {
                let base = name.rfind('/').map_or(name, |slash| &name[..slash]);
                format!("{base}/{}", request.target)
            })
        }
    }

    /// Cache the complete successful response before any decoding/selection.
    /// Errors are retried. Keys retain the requested URI, including query and
    /// fragment; redirects do not replace the source identity.
    fn fetch(&self, uri: &str) -> ResourceResult<(BinaryResource, bool)> {
        if let Some(resource) = self.uri_cache.lock().as_ref().and_then(|c| c.get(uri)) {
            return Ok((resource.clone(), true));
        }
        let resource = fetch(uri)?;
        if let Some(cache) = self.uri_cache.lock().as_mut() {
            cache.insert(uri.to_owned(), resource.clone());
        }
        Ok((resource, false))
    }

    fn include_bytes(
        &self,
        request: ResourceRequest<'_>,
    ) -> ResourceResult<(BinaryResource, bool)> {
        match Self::uri(request) {
            Some(uri) => self.fetch(&uri),
            None => self.load_binary(request).map(|resource| (resource, false)),
        }
    }
}

impl ResourceProvider for FileResources {
    fn load_asset_text(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
        self.load_with_encoding(request, None)
    }

    fn enable_uri_cache(&self) {
        self.uri_cache.lock().get_or_insert_with(BTreeMap::new);
    }

    fn prepare_include(
        &self,
        request: ResourceRequest<'_>,
        encoding: Option<&str>,
        line_limit: Option<usize>,
    ) -> ResourceResult<IncludeInput> {
        if Self::uri(request).is_some() {
            let (resource, cached) = self.include_bytes(request)?;
            // OpenURI tags the response with the external encoding but does
            // not transcode it. open-uri-cached returns a fresh binary
            // StringIO on hits and ignores the caller's encoding.
            return Ok(resource.prepare_uri_include(if cached {
                Some("BINARY")
            } else {
                encoding
            }));
        }
        let (resource, _) = self.include_bytes(request)?;
        resource.prepare_include(encoding, line_limit)
    }

    fn load_include(
        &self,
        request: ResourceRequest<'_>,
        encoding: Option<&str>,
        line_limit: Option<usize>,
    ) -> ResourceResult<IncludeResource> {
        let (resource, cached) = self.include_bytes(request)?;
        resource.decode_include(if cached { Some("BINARY") } else { encoding }, line_limit)
    }

    fn load_with_encoding(
        &self,
        request: ResourceRequest<'_>,
        encoding: Option<&str>,
    ) -> ResourceResult<Resource> {
        let (resource, cached) = self.include_bytes(request)?;
        resource.decode(if cached { Some("BINARY") } else { encoding })
    }

    fn document_path(&self, name: &str) -> Option<String> {
        // Files load under their absolute path; standard input has none.
        if uriish(name) || !Path::new(name).is_absolute() {
            return None;
        }
        Some(if std::path::MAIN_SEPARATOR == '/' {
            name.to_owned()
        } else {
            name.replace(std::path::MAIN_SEPARATOR, "/")
        })
    }

    fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
        let target = request.target;
        let resource = self.load_binary(request)?;
        text(resource).map_err(|error| ResourceError {
            target: target.to_owned(),
            ..error
        })
    }

    fn load_binary(&self, request: ResourceRequest<'_>) -> ResourceResult<BinaryResource> {
        let target = request.target;
        // A URI, or a relative target inside a URI resource.
        if let Some(uri) = Self::uri(request) {
            return self.fetch(&uri).map(|(resource, _)| resource);
        }
        if target.is_empty()
            || target.contains('\0')
            || target.contains("://")
            || target.starts_with("file:")
        {
            return Err(ResourceError::new(
                ResourceErrorKind::InvalidTarget,
                target,
                "expected a filesystem path",
            ));
        }
        let parent = request
            .referrer
            .and_then(|name| Path::new(name).parent())
            .unwrap_or(&self.root);
        let path = self.root.join(parent).join(target);
        let path = path.canonicalize().map_err(|e| io_error(target, e))?;
        if self.jail && !path.starts_with(&self.root) && self.input.as_ref() != Some(&path) {
            return Err(ResourceError::new(
                ResourceErrorKind::AccessDenied,
                target,
                "path escapes the resource root",
            ));
        }
        let name = path.to_str().ok_or_else(|| {
            ResourceError::new(
                ResourceErrorKind::InvalidTarget,
                target,
                "resolved path is not UTF-8",
            )
        })?;
        if !path.metadata().map_err(|e| io_error(target, e))?.is_file() {
            return Err(ResourceError::new(
                ResourceErrorKind::InvalidTarget,
                target,
                "resource is not a regular file",
            ));
        }
        let bytes = std::fs::read(&path).map_err(|e| io_error(target, e))?;
        Ok(BinaryResource {
            name: name.to_owned(),
            bytes,
            content_type: None,
        })
    }
}

fn io_error(target: &str, error: std::io::Error) -> ResourceError {
    let kind = match error.kind() {
        // A path through a file (`-B` naming a file) does not exist either,
        // as Ruby's `File.file?` sees it.
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
            ResourceErrorKind::NotFound
        }
        std::io::ErrorKind::PermissionDenied => ResourceErrorKind::AccessDenied,
        _ => ResourceErrorKind::Io,
    };
    ResourceError::new(kind, target, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use adoctra::{SourceMap, Span};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Http {
        uri: String,
        count: std::sync::Arc<AtomicUsize>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl Http {
        fn new(responses: Vec<(u16, Vec<u8>)>) -> Self {
            use std::io::{BufRead, BufReader, Write};
            use std::sync::{atomic::AtomicBool, Arc};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let uri = format!("http://{}", listener.local_addr().unwrap());
            let count = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let (counter, finished) = (count.clone(), stop.clone());
            let worker = std::thread::spawn(move || {
                while !finished.load(Ordering::SeqCst) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                            continue;
                        }
                        Err(error) => panic!("{error}"),
                    };
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    let mut line = String::new();
                    BufReader::new(&stream).read_line(&mut line).unwrap();
                    assert!(line.starts_with("GET "));
                    let index = counter.fetch_add(1, Ordering::SeqCst);
                    let (status, body) = &responses[index.min(responses.len() - 1)];
                    write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                    stream.write_all(body).unwrap();
                }
            });
            Self {
                uri,
                count,
                stop,
                worker: Some(worker),
            }
        }
    }

    impl Drop for Http {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.worker.take().unwrap().join().unwrap();
        }
    }

    #[test]
    fn uri_cache_is_sticky_and_isolated_and_preserves_response_bytes() {
        let server = Http::new(vec![(200, b"first".to_vec()), (200, b"second".to_vec())]);
        let tree = Tree::new();
        let provider = FileResources::new(tree.0.join("docs")).unwrap();
        let uri = format!("{}/part.adoc", server.uri);
        let request = ResourceRequest {
            target: &uri,
            referrer: None,
        };
        assert_eq!(provider.load_binary(request).unwrap().bytes, b"first");
        provider.enable_uri_cache();
        let mut loaded = provider.load_binary(request).unwrap();
        assert_eq!(loaded.bytes, b"second");
        loaded.bytes.clear();
        provider.enable_uri_cache();
        let referrer = format!("{}/outer.adoc", server.uri);
        let cached = provider
            .load_binary(ResourceRequest {
                target: "part.adoc",
                referrer: Some(&referrer),
            })
            .unwrap();
        assert_eq!(cached.bytes, b"second");
        assert_eq!(cached.content_type.as_deref(), Some("text/plain"));
        assert_eq!(cached.name, uri);
        assert_eq!(provider.document_path(&uri), None);
        assert_eq!(server.count.load(Ordering::SeqCst), 2);
        let other = FileResources::new(tree.0.join("docs")).unwrap();
        other.enable_uri_cache();
        other.load_binary(request).unwrap();
        assert_eq!(server.count.load(Ordering::SeqCst), 3);
        // Providers given one run's handle share it, enabled or not.
        let cache = UriCache::default();
        let first = FileResources::new(tree.0.join("docs"))
            .unwrap()
            .with_uri_cache(cache.clone());
        let second = FileResources::new(tree.0.join("docs"))
            .unwrap()
            .with_uri_cache(cache);
        first.enable_uri_cache();
        first.load_binary(request).unwrap();
        assert_eq!(second.load_binary(request).unwrap().bytes, b"second");
        assert_eq!(server.count.load(Ordering::SeqCst), 4);
        std::fs::write(tree.0.join("docs/local.adoc"), "old").unwrap();
        let local = ResourceRequest {
            target: "local.adoc",
            referrer: None,
        };
        assert_eq!(provider.load(local).unwrap().text, "old");
        std::fs::write(tree.0.join("docs/local.adoc"), "new").unwrap();
        assert_eq!(provider.load(local).unwrap().text, "new");
    }

    #[test]
    fn uri_cache_retries_http_errors_and_retains_success_before_decoding() {
        let server = Http::new(vec![(404, vec![]), (200, "Привет".as_bytes().to_vec())]);
        let tree = Tree::new();
        let provider = FileResources::new(tree.0.join("docs")).unwrap();
        provider.enable_uri_cache();
        let uri = format!("{}/part.adoc", server.uri);
        let request = ResourceRequest {
            target: &uri,
            referrer: None,
        };
        assert_eq!(
            provider.load_binary(request).unwrap_err().kind,
            ResourceErrorKind::NotFound
        );
        assert_eq!(
            provider
                .load_with_encoding(request, Some("US-ASCII"))
                .unwrap_err()
                .kind,
            ResourceErrorKind::InvalidEncoding
        );
        // The failed decoder must not evict a successful HTTP response.
        assert_eq!(
            provider.load_binary(request).unwrap().bytes,
            "Привет".as_bytes()
        );
        // A hit ignores the new encoding, as binary StringIO does in Ruby.
        assert_eq!(
            provider
                .load_with_encoding(request, Some("UTF-8"))
                .unwrap_err()
                .kind,
            ResourceErrorKind::InvalidEncoding
        );
        assert_eq!(server.count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn images_and_svg_activate_the_provider_cache_at_conversion() {
        for svg in [false, true] {
            let bytes = if svg {
                "<svg>Привет</svg>".as_bytes()
            } else {
                b"\x89PNG\xff"
            };
            let server = Http::new(vec![(200, bytes.to_vec()), (200, b"changed".to_vec())]);
            let tree = Tree::new();
            let provider = FileResources::new(tree.0.join("docs")).unwrap();
            let target = format!("{}/a.svg", server.uri);
            let directive = format!(
                "image::{target}[Pic{}]",
                if svg { ",opts=inline" } else { "" }
            );
            let source = format!(":cache-uri:\n:data-uri:\n\n{directive}\n\n{directive}\n");
            let mut map = SourceMap::new();
            let id = map.add("/main.adoc", source);
            let mut options = adoctra::ParseOptions {
                source_id: id,
                ..Default::default()
            };
            options.attributes.set("allow-uri-read", "");
            let result = adoctra::parse_resource(&mut map, &options, &provider).unwrap();
            assert_eq!(server.count.load(Ordering::SeqCst), 1);
            let fields: Vec<_> = result
                .document
                .blocks
                .iter()
                .map(|block| {
                    let adoctra::BlockKind::Image { src, svg, .. } = &block.kind else {
                        panic!("image")
                    };
                    (src, svg)
                })
                .collect();
            if svg {
                use adoctra::Backend;
                let json = String::from_utf8(
                    adoctra::json::JsonBackend::new()
                        .convert(&result.document)
                        .unwrap(),
                )
                .unwrap();
                assert!(
                    json.contains(r#""svg":{"mode":"inline","content":"<svg>Привет</svg>"}"#),
                    "{json}"
                );
                assert!(
                    json.contains(r#""svg":{"mode":"inline","content":null}"#),
                    "{json}"
                );
                assert_eq!(result.diagnostics.len(), 1);
                let diagnostic = &result.diagnostics[0];
                assert_eq!(
                    diagnostic.report.location,
                    adoctra::DiagnosticLocation::Unlocated
                );
                assert_eq!(
                    map.get(id)
                        .unwrap()
                        .position(diagnostic.span.start)
                        .unwrap()
                        .line,
                    6
                );
                assert_eq!(map.slice(diagnostic.span), Some(directive.as_str()));
            } else {
                assert_eq!(fields[0].0, "data:text/plain;base64,iVBOR/8=");
                assert_eq!(fields[0].0, fields[1].0);
                assert!(result.diagnostics.is_empty());
            }
        }
    }

    #[test]
    fn cached_includes_keep_snapshots_tokens_and_byte_limits() {
        let server = Http::new(vec![(200, b"*one*\n\xff\n*three*\n".to_vec())]);
        let tree = Tree::new();
        let provider = FileResources::new(tree.0.join("docs")).unwrap();
        let uri = format!("{}/part.adoc", server.uri);
        let mut sources = SourceMap::new();
        let id = sources.add(
            "/main.adoc",
            format!(":cache-uri:\n\ninclude::{uri}[lines=1;3]\n\ninclude::{uri}[lines=3]\n"),
        );
        let mut options = adoctra::ParseOptions {
            source_id: id,
            ..Default::default()
        };
        options.attributes.set("allow-uri-read", "");
        let result = adoctra::parse_resource(&mut sources, &options, &provider).unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let source = sources.get(adoctra::SourceId(1)).unwrap();
        assert_eq!(source.text(), "*one*\n*three*\n");
        assert_eq!(source.omitted_lines()[0].bytes, b"\xff\n");
        assert_eq!(source.position(6).unwrap().line, 3);
        assert!(sources.get(adoctra::SourceId(2)).is_none());
        let tokens = adoctra::tokens_resource(&mut sources, &options, &provider).unwrap();
        assert!(tokens
            .iter()
            .any(|token| token.span.source_id == adoctra::SourceId(1) && token.span.start >= 6));
        assert_eq!(server.count.load(Ordering::SeqCst), 1);
        options.includes.max_bytes = 16;
        let limited = adoctra::parse_resource(&mut sources, &options, &provider).unwrap();
        assert_eq!(limited.diagnostics.len(), 1);
        assert_eq!(
            limited.diagnostics[0].message,
            "maximum included byte count exceeded"
        );
        assert_eq!(
            sources.get(adoctra::SourceId(1)).unwrap().text(),
            "*one*\n*three*\n"
        );
        assert_eq!(server.count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn uri_includes_convert_only_selected_lines_like_open_uri() {
        // Windows-1251 has no mapping for 0x98. OpenURI does not transcode,
        // so the unselected line never fails; a file read transcodes it.
        let body = b"*one*\n\x98\n\xcf\xf0\xe8\xe2\xe5\xf2\n".to_vec();
        let server = Http::new(vec![(200, body.clone())]);
        let tree = Tree::new();
        let root = tree.0.join("docs");
        std::fs::write(root.join("part.adoc"), &body).unwrap();
        let provider = FileResources::new(&root).unwrap();
        let uri = format!("{}/part.adoc", server.uri);
        let request = ResourceRequest {
            target: &uri,
            referrer: None,
        };
        assert!(matches!(
            provider.prepare_include(request, Some("Windows-1251"), Some(3)),
            Ok(adoctra::IncludeInput::Unconverted { encoding: Some(name), .. }) if name == "Windows-1251"
        ));
        let parse = |sources: &mut SourceMap, target: &str| {
            let id = sources.add(
                root.join("main.adoc").to_str().unwrap(),
                format!("include::{target}[encoding=Windows-1251,lines=1;3]\n"),
            );
            let mut options = adoctra::ParseOptions {
                source_id: id,
                ..Default::default()
            };
            options.attributes.set("allow-uri-read", "");
            adoctra::parse_resource(sources, &options, &provider).unwrap()
        };
        let mut sources = SourceMap::new();
        let result = parse(&mut sources, &uri);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let source = sources.get(adoctra::SourceId(1)).unwrap();
        assert_eq!(source.name(), uri);
        assert_eq!(source.text(), "*one*\nПривет\n");
        assert_eq!(source.omitted_lines()[0].bytes, b"\x98\n");
        assert_eq!(source.position(6).unwrap().line, 3);
        // The selected lines form one paragraph across the omitted line.
        let spans: Vec<_> = result.document.blocks.iter().map(|b| b.span).collect();
        assert_eq!(
            spans,
            [adoctra::Span::in_source(adoctra::SourceId(1), 0, 18)]
        );
        let mut sources = SourceMap::new();
        let result = parse(&mut sources, "part.adoc");
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.document.blocks.len() == 1);
        assert_eq!(server.count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn uri_includes_remove_one_bom_and_keep_a_following_one_as_content() {
        // `prepare_source_string` slices off the UTF-16 BOM; the second one
        // is a U+FEFF character, which the reader must not strip again.
        let mut body = b"\xfe\xff\xfe\xff".to_vec();
        body.extend("a\n".encode_utf16().flat_map(u16::to_be_bytes));
        let server = Http::new(vec![(200, body)]);
        let tree = Tree::new();
        let provider = FileResources::new(tree.0.join("docs")).unwrap();
        let mut sources = SourceMap::new();
        let id = sources.add(
            "/main.adoc",
            format!("include::{}/part.adoc[encoding=Windows-1251]\n", server.uri),
        );
        let mut options = adoctra::ParseOptions {
            source_id: id,
            ..Default::default()
        };
        options.attributes.set("allow-uri-read", "");
        let result = adoctra::parse_resource(&mut sources, &options, &provider).unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            sources.get(adoctra::SourceId(1)).unwrap().text(),
            "\u{feff}a\n"
        );
        assert_eq!(
            result.document.blocks[0].span,
            adoctra::Span::in_source(adoctra::SourceId(1), 0, 4)
        );
    }

    struct Tree(PathBuf);
    impl Tree {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "adoctra-resources-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            std::fs::create_dir(path.join("docs")).unwrap();
            std::fs::create_dir(path.join("docs/chapters")).unwrap();
            Self(path)
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn nested_paths_preserve_text_identity_and_source_positions() {
        let tree = Tree::new();
        let root = tree.0.join("docs");
        std::fs::write(root.join("main.adoc"), "main").unwrap();
        std::fs::write(root.join("chapters/часть.adoc"), "\u{feff}Я🙂\r\ntext\n").unwrap();
        let provider = FileResources::new(&root).unwrap();
        let mut sources = SourceMap::new();
        let main = sources.load("main.adoc", None, &provider).unwrap();
        let child = sources
            .load("chapters/часть.adoc", Some(main), &provider)
            .unwrap();
        assert_eq!(sources.get(child).unwrap().text(), "\u{feff}Я🙂\r\ntext\n");
        assert_eq!(sources.slice(Span::in_source(child, 3, 5)), Some("Я"));
        assert_eq!(
            sources
                .load("../main.adoc", Some(child), &provider)
                .unwrap(),
            main
        );
        assert_eq!(
            sources
                .load("./chapters/../main.adoc", None, &provider)
                .unwrap(),
            main
        );
        assert_eq!(
            sources
                .load(
                    sources.get(main).unwrap().name().to_owned().as_str(),
                    None,
                    &provider
                )
                .unwrap(),
            main
        );
    }

    #[test]
    fn load_errors_are_typed_and_do_not_register_sources() {
        let tree = Tree::new();
        std::fs::write(tree.0.join("outside.adoc"), "outside").unwrap();
        std::fs::write(tree.0.join("docs/bad.adoc"), [0xff, 0xfe]).unwrap();
        let provider = FileResources::new(tree.0.join("docs")).unwrap();
        let mut sources = SourceMap::new();
        for (target, kind) in [
            ("missing.adoc", ResourceErrorKind::NotFound),
            ("../outside.adoc", ResourceErrorKind::AccessDenied),
            ("bad.adoc", ResourceErrorKind::InvalidEncoding),
            ("chapters", ResourceErrorKind::InvalidTarget),
            ("file:///etc/hostname", ResourceErrorKind::InvalidTarget),
            ("ftp://example.org/a.adoc", ResourceErrorKind::InvalidTarget),
        ] {
            assert_eq!(
                sources.load(target, None, &provider).unwrap_err().kind,
                kind,
                "{target}"
            );
        }
        assert!(sources.get(adoctra::SourceId(0)).is_none());
    }

    #[test]
    fn unsafe_mode_reads_outside_the_root() {
        let tree = Tree::new();
        std::fs::write(tree.0.join("outside.adoc"), "outside").unwrap();
        let provider = FileResources::new(tree.0.join("docs")).unwrap().unjailed();
        let mut sources = SourceMap::new();
        let id = sources.load("../outside.adoc", None, &provider).unwrap();
        assert_eq!(sources.get(id).unwrap().text(), "outside");
    }

    #[test]
    fn encoding_aware_reads_preserve_access_policy_and_decode_http_bytes() {
        use std::io::{BufRead, BufReader, Write};
        let tree = Tree::new();
        std::fs::write(tree.0.join("outside.adoc"), b"\xff").unwrap();
        let provider = FileResources::new(tree.0.join("docs")).unwrap();
        let error = provider
            .load_with_encoding(
                ResourceRequest {
                    target: "../outside.adoc",
                    referrer: None,
                },
                Some("ISO-8859-1"),
            )
            .unwrap_err();
        assert_eq!(error.kind, ResourceErrorKind::AccessDenied);

        let error = provider
            .load_include(
                ResourceRequest {
                    target: "../outside.adoc",
                    referrer: None,
                },
                Some("Shift_JIS"),
                Some(1),
            )
            .unwrap_err();
        assert_eq!(error.kind, ResourceErrorKind::AccessDenied);

        let error = provider
            .prepare_include(
                ResourceRequest {
                    target: "../outside.adoc",
                    referrer: None,
                },
                Some("UTF-8"),
                None,
            )
            .unwrap_err();
        assert_eq!(error.kind, ResourceErrorKind::AccessDenied);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let target = format!("http://{}/part.adoc", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for bytes in [
                &b"\xcf\xf0\xe8\xe2\xe5\xf2"[..],
                &b"\x95\x5c\r\n\x81"[..],
                &b"first\n\xff\nlast\n"[..],
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                assert!(line.starts_with("GET /part.adoc "));
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .unwrap();
                stream.write_all(bytes).unwrap();
            }
        });
        let resource = provider
            .load_with_encoding(
                ResourceRequest {
                    target: &target,
                    referrer: None,
                },
                Some("CP1251"),
            )
            .unwrap();
        assert_eq!(resource.name, target);
        assert_eq!(resource.text, "Привет");
        let prefix = provider
            .load_include(
                ResourceRequest {
                    target: &target,
                    referrer: None,
                },
                Some("Shift_JIS"),
                Some(1),
            )
            .unwrap();
        assert_eq!(prefix.resource.name, target);
        assert_eq!(prefix.resource.text, "表\r\n");
        assert!(!prefix.complete);
        let mut sources = SourceMap::new();
        let id = sources.add("/main.adoc", format!("include::{target}[lines=1;3]"));
        let mut options = adoctra::ParseOptions {
            source_id: id,
            ..Default::default()
        };
        options.attributes.set("allow-uri-read", "");
        let result = adoctra::parse_resource(&mut sources, &options, &provider).unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let source = sources.get(adoctra::SourceId(1)).unwrap();
        assert_eq!(source.name(), target);
        assert_eq!(source.text(), "first\nlast\n");
        assert_eq!(source.omitted_lines()[0].bytes, b"\xff\n");
        server.join().unwrap();
    }

    /// A one-request-per-connection HTTP server for `paths`.
    fn serve(paths: &'static [(&'static str, &'static str)]) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
                let response = match paths.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{body}",
                        body.len(),
                        if path.ends_with(".png") {
                            "Content-Type: image/png\r\n"
                        } else {
                            ""
                        }
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_owned()
                    }
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{address}")
    }

    #[test]
    fn uris_are_fetched_and_relative_targets_resolve_against_them() {
        let base = serve(&[("/docs/a.adoc", "remote"), ("/docs/part/b.adoc", "child")]);
        let tree = Tree::new();
        let provider = FileResources::new(tree.0.join("docs")).unwrap();
        let mut sources = SourceMap::new();
        let a = sources
            .load(&format!("{base}/docs/a.adoc"), None, &provider)
            .unwrap();
        assert_eq!(sources.get(a).unwrap().text(), "remote");
        assert_eq!(
            sources.get(a).unwrap().name(),
            format!("{base}/docs/a.adoc")
        );
        let b = sources.load("part/b.adoc", Some(a), &provider).unwrap();
        assert_eq!(sources.get(b).unwrap().text(), "child");
        assert_eq!(
            sources
                .load(&format!("{base}/none.adoc"), None, &provider)
                .unwrap_err()
                .kind,
            ResourceErrorKind::NotFound
        );
    }

    #[test]
    fn binary_resources_keep_bytes_and_report_uri_content_types() {
        let tree = Tree::new();
        let root = tree.0.join("docs");
        std::fs::write(root.join("main.adoc"), "main").unwrap();
        std::fs::write(root.join("chapters/a.png"), [0x89, 0xff, 0x00]).unwrap();
        std::fs::write(tree.0.join("outside.png"), "x").unwrap();
        let provider = FileResources::new(&root).unwrap();
        let main = root.join("main.adoc").canonicalize().unwrap();
        let request = |target| ResourceRequest {
            target,
            referrer: main.to_str(),
        };
        let image = provider.load_binary(request("chapters/a.png")).unwrap();
        assert_eq!(image.bytes, [0x89, 0xff, 0x00]);
        assert_eq!(image.content_type, None);
        // As text the same bytes are not UTF-8; the error names the target.
        let error = provider.load(request("chapters/a.png")).unwrap_err();
        assert_eq!(error.kind, ResourceErrorKind::InvalidEncoding);
        assert_eq!(error.target, "chapters/a.png");
        assert_eq!(
            provider
                .load_binary(request("../outside.png"))
                .unwrap_err()
                .kind,
            ResourceErrorKind::AccessDenied
        );
        let base = serve(&[("/i/a.png", "png")]);
        let remote = provider
            .load_binary(request(&format!("{base}/i/a.png")))
            .unwrap();
        assert_eq!(remote.bytes, b"png");
        assert_eq!(remote.content_type.as_deref(), Some("image/png"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_share_identity_and_links_outside_the_root_are_denied() {
        let tree = Tree::new();
        let root = tree.0.join("docs");
        std::fs::write(root.join("main.adoc"), "main").unwrap();
        std::fs::write(tree.0.join("outside.adoc"), "outside").unwrap();
        std::os::unix::fs::symlink("main.adoc", root.join("alias.adoc")).unwrap();
        std::os::unix::fs::symlink("../outside.adoc", root.join("escape.adoc")).unwrap();
        let provider = FileResources::new(root).unwrap();
        let mut sources = SourceMap::new();
        let main = sources.load("main.adoc", None, &provider).unwrap();
        assert_eq!(sources.load("alias.adoc", None, &provider).unwrap(), main);
        assert_eq!(
            sources
                .load("escape.adoc", None, &provider)
                .unwrap_err()
                .kind,
            ResourceErrorKind::AccessDenied
        );
    }

    #[test]
    fn a_base_dir_root_need_not_exist_and_admits_only_the_named_input_outside_it() {
        let tree = Tree::new();
        std::fs::write(tree.0.join("main.adoc"), "main").unwrap();
        std::fs::write(tree.0.join("other.adoc"), "other").unwrap();
        std::fs::write(tree.0.join("docs/inside.adoc"), "inside").unwrap();
        let input = tree.0.join("main.adoc").canonicalize().unwrap();
        fn request(target: &str) -> ResourceRequest<'_> {
            ResourceRequest {
                target,
                referrer: None,
            }
        }
        // `-B docs` while the input lies beside it (Ruby reads it by path).
        let provider = FileResources::base_dir(&tree.0.join("docs")).with_input(input.clone());
        let main = provider.load(request(input.to_str().unwrap())).unwrap();
        assert_eq!(main.text, "main");
        assert_eq!(
            provider.load(request("inside.adoc")).unwrap().text,
            "inside"
        );
        let other = tree.0.join("other.adoc");
        assert_eq!(
            provider
                .load(request(other.to_str().unwrap()))
                .unwrap_err()
                .kind,
            ResourceErrorKind::AccessDenied
        );
        assert_eq!(
            provider
                .unjailed()
                .load(request(other.to_str().unwrap()))
                .unwrap()
                .text,
            "other"
        );
        // A missing root resolves nothing under it, and a root that is a
        // file reports a path through it as missing (Ruby's `File.file?`).
        for root in [tree.0.join("nope"), tree.0.join("other.adoc")] {
            let provider = FileResources::base_dir(&root).with_input(input.clone());
            assert_eq!(
                provider
                    .load(request(input.to_str().unwrap()))
                    .unwrap()
                    .text,
                "main"
            );
            assert_eq!(
                provider.load(request("x.adoc")).unwrap_err().kind,
                ResourceErrorKind::NotFound
            );
        }
    }
}
