//! Native filesystem adapter; the parser and WASM never import this module.
use adoctra::{
    BinaryResource, IncludeResource, Resource, ResourceError, ResourceErrorKind, ResourceProvider,
    ResourceRequest, ResourceResult,
};
use std::path::{Path, PathBuf};

pub(crate) struct FileResources {
    root: PathBuf,
    /// Confine paths to the root (safe mode and above); unsafe mode reads
    /// any path, like Ruby without a jail.
    jail: bool,
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
        Ok(Self { root, jail: true })
    }

    /// Read paths outside the root too (unsafe mode).
    pub fn unjailed(mut self) -> Self {
        self.jail = false;
        self
    }
}

impl ResourceProvider for FileResources {
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
        if uriish(target) {
            return fetch(target);
        }
        if let Some(referrer) = request.referrer.filter(|referrer| uriish(referrer)) {
            let base = referrer
                .rfind('/')
                .map_or(referrer, |slash| &referrer[..slash]);
            return fetch(&format!("{base}/{target}"));
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
        if self.jail && !path.starts_with(&self.root) {
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
        std::io::ErrorKind::NotFound => ResourceErrorKind::NotFound,
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

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let target = format!("http://{}/part.adoc", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for bytes in [&b"\xcf\xf0\xe8\xe2\xe5\xf2"[..], &b"\x95\x5c\r\n\x81"[..]] {
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
}
