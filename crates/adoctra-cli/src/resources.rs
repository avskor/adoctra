//! Native filesystem adapter; the parser and WASM never import this module.
use adoctra::{
    Resource, ResourceError, ResourceErrorKind, ResourceProvider, ResourceRequest, ResourceResult,
};
use std::path::{Path, PathBuf};

pub(crate) struct FileResources {
    root: PathBuf,
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
        Ok(Self { root })
    }
}

impl ResourceProvider for FileResources {
    fn load(&self, request: ResourceRequest<'_>) -> ResourceResult<Resource> {
        let target = request.target;
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
        if !path.starts_with(&self.root) {
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
        let text = String::from_utf8(bytes).map_err(|_| {
            ResourceError::new(
                ResourceErrorKind::InvalidEncoding,
                target,
                "resource is not UTF-8",
            )
        })?;
        Ok(Resource {
            name: name.to_owned(),
            text,
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
            (
                "https://example.org/a.adoc",
                ResourceErrorKind::InvalidTarget,
            ),
        ] {
            assert_eq!(
                sources.load(target, None, &provider).unwrap_err().kind,
                kind,
                "{target}"
            );
        }
        assert!(sources.get(adoctra::SourceId(0)).is_none());
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
