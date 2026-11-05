//! Ruby's `PathResolver#system_path` and `AbstractNode#normalize_system_path`:
//! lexical resolution of a target against a start directory, confined to a
//! jail (the document's base directory from safe mode up) with Ruby's
//! automatic recovery and its warnings. No I/O: the result names the path
//! Ruby reads and reports (e.g. "include file not found: <path>"); the host
//! provider reads it.
//!
//! Like Ruby's resolver, whose `file_separator` is the host's
//! (`File::ALT_SEPARATOR` on Windows), Windows hosts also accept drive roots
//! (`C:/`, `C:\`) and backslashes, which are posixified to `/`; results
//! always use `/` (e.g. `C:/doc/a.adoc`).
use std::borrow::Cow;

use crate::SafeMode;

/// A resolved path and the warnings Ruby logs while resolving it.
#[derive(Debug, PartialEq, Eq)]
pub struct SystemPath {
    pub path: String,
    pub warnings: Vec<String>,
}

pub(crate) type Resolved = SystemPath;

/// Ruby's `PathResolver` file separator: `\` (Windows) also accepts drive
/// roots and backslashes, `/` only POSIX paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathStyle {
    Posix,
    Windows,
}

impl PathStyle {
    /// The host's style, as Ruby's `File::ALT_SEPARATOR || File::SEPARATOR`.
    pub(crate) const HOST: Self = if cfg!(windows) {
        Self::Windows
    } else {
        Self::Posix
    };

    /// Ruby's `posixify`: backslashes to slashes on Windows.
    fn posixify(self, path: &str) -> Cow<'_, str> {
        if self == Self::Windows && path.contains('\\') {
            Cow::Owned(path.replace('\\', "/"))
        } else {
            Cow::Borrowed(path)
        }
    }

    /// Ruby's `root?` (`absolute_path?`): a leading slash, or on Windows
    /// `WindowsRootRx` (`^(?:[a-zA-Z]:)?[\\/]`).
    fn is_root(self, path: &str) -> bool {
        path.starts_with('/')
            || self == Self::Windows && {
                let bytes = path.as_bytes();
                let rest = match bytes {
                    [drive, b':', rest @ ..] if drive.is_ascii_alphabetic() => rest,
                    _ => bytes,
                };
                matches!(rest.first(), Some(b'/' | b'\\'))
            }
    }
}

/// Ruby's `PathResolver#root?` for this host: `/x`, and on Windows also
/// `C:/x`, `C:\x` and `\x`.
pub fn is_root_path(path: &str) -> bool {
    PathStyle::HOST.is_root(path)
}

/// Ruby's `PathResolver#posixify` for this host: on Windows backslashes
/// become slashes; elsewhere the path is unchanged.
pub fn posixify_path(path: &str) -> String {
    PathStyle::HOST.posixify(path).into_owned()
}

/// Ruby's `File.dirname` of a posixified absolute or relative path: the
/// root (`/`, or `C:/` on Windows) stays a root, a bare name gives `.`.
pub fn parent_system_dir(path: &str) -> String {
    parent_dir(path, PathStyle::HOST)
}

fn parent_dir(path: &str, style: PathStyle) -> String {
    let path = style.posixify(path);
    let (_, root) = partition(&path, style);
    let body = path[root.map_or(0, str::len)..].trim_end_matches('/');
    match (body.rfind('/'), root) {
        (Some(slash), root) => format!("{}{}", root.unwrap_or(""), &body[..slash]),
        (None, Some(root)) if root != "./" => root.to_owned(),
        (None, _) => ".".to_owned(),
    }
}

/// Ruby's `PathResolver#system_path(target, start, jail, target_name:,
/// recover:)` for a host (e.g. a CLI resolving its output paths): `start`
/// is absolute, `jail` (if any) absolute. With `recover` an escape from the
/// jail is moved inside it with Ruby's warnings; without it the result is
/// Ruby's `SecurityError` message, e.g. `to_dir /x is outside of jail: /doc
/// (disallowed in safe mode)`.
pub fn resolve_system_path(
    target: &str,
    start: &str,
    jail: Option<&str>,
    target_name: &str,
    recover: bool,
) -> Result<SystemPath, String> {
    resolve(
        target,
        Some(start),
        jail,
        target_name,
        start,
        recover,
        PathStyle::HOST,
    )
}

/// Ruby's `partition_path` (no web paths) of a posixified path: segments
/// without `.` entries and the root (`/`, `//`, `./` or on Windows `C:/`),
/// if any.
fn partition(path: &str, style: PathStyle) -> (Vec<&str>, Option<&str>) {
    let root = if path.starts_with("//") {
        Some("//")
    } else if path.starts_with('/') {
        Some("/")
    } else if style.is_root(path) {
        // A drive root, through its first slash.
        path.find('/').map(|slash| &path[..=slash])
    } else if path.starts_with("./") {
        Some("./")
    } else {
        None
    };
    let rest = &path[root.map_or(0, str::len)..];
    // Ruby's String#split drops trailing empty fields.
    let mut segments: Vec<&str> = rest.split('/').collect();
    while segments.last() == Some(&"") {
        segments.pop();
    }
    if rest.is_empty() {
        segments.clear();
    }
    segments.retain(|segment| *segment != ".");
    (segments, root)
}

fn join(segments: &[&str], root: Option<&str>) -> String {
    format!("{}{}", root.unwrap_or(""), segments.join("/"))
}

/// Ruby's `descends_from?`.
fn descends_from(path: &str, base: &str) -> bool {
    base == path
        || if base == "/" {
            path.starts_with('/')
        } else {
            path.strip_prefix(base)
                .is_some_and(|rest| rest.starts_with('/'))
        }
}

/// Ruby's `expand_path` (lexical) of a posixified path.
fn expand(path: &str, style: PathStyle) -> String {
    let (segments, root) = partition(path, style);
    let mut resolved = Vec::new();
    for segment in segments {
        if segment == ".." {
            resolved.pop();
        } else {
            resolved.push(segment);
        }
    }
    join(&resolved, root)
}

/// Ruby's `system_path(target, start, jail, target_name:)` with recovery.
/// `working_dir` is Ruby's `@working_dir` (the process directory); the
/// callers pass a start, so it is only reached for an empty start.
pub(crate) fn system_path(
    target: &str,
    start: Option<&str>,
    jail: Option<&str>,
    target_name: &str,
    working_dir: &str,
) -> Resolved {
    resolve(
        target,
        start,
        jail,
        target_name,
        working_dir,
        true,
        PathStyle::HOST,
    )
    .expect("recovering resolution never fails")
}

fn resolve(
    target: &str,
    start: Option<&str>,
    jail: Option<&str>,
    target_name: &str,
    working_dir: &str,
    recover: bool,
    style: PathStyle,
) -> Result<Resolved, String> {
    let mut warnings = Vec::new();
    // Messages name the target as given and the posixified jail.
    let original_target = target;
    let target = style.posixify(target);
    let target = target.as_ref();
    let start = start.map(|start| style.posixify(start));
    let start = start.as_deref();
    let jail = jail.map(|jail| style.posixify(jail));
    let jail = jail.as_deref();
    let working_dir = style.posixify(working_dir);
    let working_dir = working_dir.as_ref();
    let outside = |warnings: &mut Vec<String>, jail: &str| {
        if !recover {
            return Err(format!(
                "{target_name} {original_target} is outside of jail: {jail} (disallowed in safe mode)"
            ));
        }
        warnings.push(format!(
            "{target_name} is outside of jail; recovering automatically"
        ));
        Ok(())
    };
    if style.is_root(target) {
        let target_path = expand(target, style);
        if let Some(jail) = jail.filter(|jail| !descends_from(&target_path, jail)) {
            outside(&mut warnings, jail)?;
            let (target_segments, _) = partition(&target_path, style);
            let (mut jail_segments, jail_root) = partition(jail, style);
            jail_segments.extend(target_segments);
            return Ok(Resolved {
                path: join(&jail_segments, jail_root),
                warnings,
            });
        }
        return Ok(Resolved {
            path: target_path,
            warnings,
        });
    }
    let fallback = jail.unwrap_or(working_dir);
    let mut target_segments = partition(target, style).0;
    let start = match start.filter(|start| !start.is_empty()) {
        _ if target_segments.is_empty() => match start.filter(|start| !start.is_empty()) {
            None => {
                return Ok(Resolved {
                    path: fallback.to_owned(),
                    warnings,
                });
            }
            Some(start) if style.is_root(start) => {
                if jail.is_none() {
                    return Ok(Resolved {
                        path: expand(start, style),
                        warnings,
                    });
                }
                start.to_owned()
            }
            Some(start) => {
                target_segments = partition(start, style).0;
                fallback.to_owned()
            }
        },
        None => fallback.to_owned(),
        Some(start) if style.is_root(start) => start.to_owned(),
        Some(start) => format!("{}/{start}", fallback.trim_end_matches('/')),
    };
    let mut recheck = jail.is_some_and(|jail| !descends_from(&start, jail));
    let (mut start_segments, mut root) = partition(&start, style);
    // On Windows a start on another drive than the jail cannot be
    // rechecked: Ruby starts from the jail instead.
    if let (true, PathStyle::Windows, Some(jail)) = (recheck, style, jail) {
        let (jail_segments, jail_root) = partition(jail, style);
        if root != jail_root {
            if !recover {
                return Err(format!(
                    "start path for {target_name} {start} refers to location outside jail root: {jail} (disallowed in safe mode)"
                ));
            }
            warnings.push(format!(
                "start path for {target_name} is outside of jail root; recovering automatically"
            ));
            start_segments = jail_segments;
            root = jail_root;
            recheck = false;
        }
    }
    let mut resolved: Vec<&str> = start_segments;
    resolved.extend(target_segments.iter().copied());
    if resolved.contains(&"..") {
        let unresolved = std::mem::take(&mut resolved);
        if let Some(jail) = jail {
            let jail_depth = partition(jail, style).0.len();
            let mut warned = false;
            for segment in unresolved {
                if segment != ".." {
                    resolved.push(segment);
                } else if resolved.len() > jail_depth {
                    resolved.pop();
                } else if !recover {
                    return Err(format!(
                        "{target_name} {original_target} refers to location outside jail: {jail} (disallowed in safe mode)"
                    ));
                } else if !warned {
                    warnings.push(format!(
                        "{target_name} has illegal reference to ancestor of jail; recovering automatically"
                    ));
                    warned = true;
                }
            }
        } else {
            for segment in unresolved {
                if segment == ".." {
                    resolved.pop();
                } else {
                    resolved.push(segment);
                }
            }
        }
    }
    if let (true, Some(jail)) = (recheck, jail) {
        let target_path = join(&resolved, root);
        if descends_from(&target_path, jail) {
            return Ok(Resolved {
                path: target_path,
                warnings,
            });
        }
        outside(&mut warnings, jail)?;
        let (mut jail_segments, jail_root) = partition(jail, style);
        jail_segments.extend(target_segments.iter().copied());
        return Ok(Resolved {
            path: join(&jail_segments, jail_root),
            warnings,
        });
    }
    Ok(Resolved {
        path: join(&resolved, root),
        warnings,
    })
}

/// Where a converter reads an asset (docinfo, a stylesheet, an embedded
/// image): `path` is Ruby's system path (its messages name it), `request` the
/// same path for the resource provider, relative to the primary document's
/// directory, and `warnings` Ruby's jail recovery messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPath {
    pub path: String,
    pub request: String,
    pub warnings: Vec<String>,
}

/// Ruby's `normalize_system_path(target, start)` for an asset a converter
/// reads: resolved from `base_dir` (the document's, jailed from safe mode
/// up) when it is known, requested relative to `referrer_dir` (the primary
/// document's directory, else `base_dir`). Without a base directory,
/// `target` joined to a relative `start` for both.
pub fn asset_path(
    target: &str,
    start: Option<&str>,
    base_dir: Option<&str>,
    referrer_dir: Option<&str>,
    safe: SafeMode,
    target_name: &str,
) -> AssetPath {
    if let Some(base_dir) = base_dir {
        let resolved = normalize_system_path(target, start, base_dir, safe, target_name);
        let from = referrer_dir.unwrap_or(base_dir);
        let request = crate::xref_target::relative_path(
            &format!("{}/_", from.trim_end_matches('/')),
            &resolved.path,
        );
        return AssetPath {
            path: resolved.path,
            request,
            warnings: resolved.warnings,
        };
    }
    let path = match start.filter(|dir| !dir.is_empty() && !is_root_path(target)) {
        Some(dir) => format!("{}/{target}", dir.trim_end_matches('/')),
        None => target.to_owned(),
    };
    AssetPath {
        request: path.clone(),
        path,
        warnings: Vec::new(),
    }
}

/// Ruby's `normalize_system_path(target, start, nil, target_name:)` for a
/// document whose base directory is `base_dir`: below safe mode a relative
/// start joins the base directory and there is no jail; from safe mode up
/// the base directory is the default start and the jail.
pub(crate) fn normalize_system_path(
    target: &str,
    start: Option<&str>,
    base_dir: &str,
    safe: SafeMode,
    target_name: &str,
) -> Resolved {
    if safe < SafeMode::Safe {
        let start = match start {
            Some(start) if is_root_path(start) => start.to_owned(),
            Some(start) => format!("{base_dir}/{start}"),
            None => base_dir.to_owned(),
        };
        system_path(target, Some(&start), None, target_name, base_dir)
    } else {
        system_path(
            target,
            Some(start.unwrap_or(base_dir)),
            Some(base_dir),
            target_name,
            base_dir,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(target: &str, start: &str, jail: Option<&str>) -> (String, Vec<String>) {
        let resolved = system_path(target, Some(start), jail, "path", "/cwd");
        (resolved.path, resolved.warnings)
    }

    /// Ruby's `recover: false` (Asciidoctor.convert resolving `:to_dir` and
    /// `:to_file`): each jail escape is a SecurityError message instead.
    #[test]
    fn unrecovered_jail_escapes_are_errors() {
        let strict = |target: &str, start: &str, jail: Option<&str>| {
            resolve_system_path(target, start, jail, "to_dir", false)
        };
        assert_eq!(
            strict("/etc", "/doc", Some("/doc")),
            Err("to_dir /etc is outside of jail: /doc (disallowed in safe mode)".to_owned())
        );
        assert_eq!(
            strict("../a.html", "/doc", Some("/doc")),
            Err(
                "to_dir ../a.html refers to location outside jail: /doc (disallowed in safe mode)"
                    .to_owned()
            )
        );
        assert_eq!(
            strict("y/b.htm", "/out", Some("/doc")),
            Err("to_dir y/b.htm is outside of jail: /doc (disallowed in safe mode)".to_owned())
        );
        assert_eq!(
            strict("out/../b.html", "/doc", Some("/doc")).map(|p| p.path),
            Ok("/doc/b.html".to_owned())
        );
        assert_eq!(
            strict("/etc/x", "/doc", None).map(|p| p.path),
            Ok("/etc/x".to_owned())
        );
        // Recovering keeps the warnings.
        let recovered = resolve_system_path("/etc", "/doc", Some("/doc"), "path", true).unwrap();
        assert_eq!(recovered.path, "/doc/etc");
        assert_eq!(
            recovered.warnings,
            ["path is outside of jail; recovering automatically"]
        );
    }

    #[test]
    fn relative_targets_resolve_against_the_start() {
        // path_resolver_test.rb (paths_test.rb) expectations.
        assert_eq!(resolve("images/a.png", "/doc", None).0, "/doc/images/a.png");
        assert_eq!(resolve("../a.png", "/doc/sub", None).0, "/doc/a.png");
        assert_eq!(resolve("./a/./b", "/doc", None).0, "/doc/a/b");
        assert_eq!(resolve("a", "/doc", Some("/doc")).0, "/doc/a");
        assert_eq!(resolve("", "/doc", None).0, "/doc");
    }

    #[test]
    fn ancestors_of_the_jail_are_recovered_once_with_a_warning() {
        let (path, warnings) = resolve("../../../etc/passwd", "/doc/sub", Some("/doc"));
        assert_eq!(path, "/doc/etc/passwd");
        assert_eq!(
            warnings,
            ["path has illegal reference to ancestor of jail; recovering automatically"]
        );
        // Inside the jail `..` is plain resolution.
        assert_eq!(
            resolve("../a", "/doc/sub", Some("/doc")),
            ("/doc/a".to_owned(), vec![])
        );
    }

    #[test]
    fn absolute_targets_outside_the_jail_move_inside_it() {
        let (path, warnings) = resolve("/tmp/x/a.adoc", "/doc", Some("/doc"));
        assert_eq!(path, "/doc/tmp/x/a.adoc");
        assert_eq!(
            warnings,
            ["path is outside of jail; recovering automatically"]
        );
        assert_eq!(
            resolve("/doc/a.adoc", "/doc", Some("/doc")).0,
            "/doc/a.adoc"
        );
        assert_eq!(resolve("/x/../y", "/doc", None).0, "/y");
    }

    #[test]
    fn a_start_outside_the_jail_is_rechecked() {
        let (path, warnings) = resolve("a.adoc", "/elsewhere", Some("/doc"));
        assert_eq!(path, "/doc/a.adoc");
        assert_eq!(
            warnings,
            ["path is outside of jail; recovering automatically"]
        );
    }

    fn on_windows(
        target: &str,
        start: &str,
        jail: Option<&str>,
        recover: bool,
    ) -> Result<(String, Vec<String>), String> {
        super::resolve(
            target,
            Some(start),
            jail,
            "to_dir",
            start,
            recover,
            PathStyle::Windows,
        )
        .map(|resolved| (resolved.path, resolved.warnings))
    }

    /// Ruby's `PathResolver` with the Windows file separator: drive roots
    /// and backslashes, posixified (regression: `-o c:\tmp\a.docx` was
    /// joined to the base directory as a relative `c:` segment).
    #[test]
    fn windows_drive_roots_are_absolute_and_posixified() {
        let style = PathStyle::Windows;
        for root in ["C:/x", "c:\\x", "\\x", "/x", "//server/share"] {
            assert!(style.is_root(root), "{root}");
        }
        for relative in ["C:x", "x", "1:/x", "./x"] {
            assert!(!style.is_root(relative), "{relative}");
        }
        // POSIX hosts keep Ruby's view: `c:/x` is a relative name.
        assert!(!PathStyle::Posix.is_root("c:/x"));
        assert!(!PathStyle::Posix.is_root("c:\\x"));
        assert_eq!(partition("C:/a/./b", style), (vec!["a", "b"], Some("C:/")));
        assert_eq!(
            on_windows("c:\\tmp\\sdc.docx", "C:/Work/doc", None, false),
            Ok(("c:/tmp/sdc.docx".to_owned(), vec![]))
        );
        assert_eq!(
            on_windows("out\\..\\a.docx", "C:/Work/doc", None, false),
            Ok(("C:/Work/doc/a.docx".to_owned(), vec![]))
        );
        assert_eq!(
            on_windows("a.docx", "C:\\Work\\doc", None, false),
            Ok(("C:/Work/doc/a.docx".to_owned(), vec![]))
        );
        assert_eq!(
            on_windows("C:/x/../../y", "D:/doc", None, false),
            Ok(("C:/y".to_owned(), vec![]))
        );
        // Posix hosts join the same target as a relative name (Ruby too).
        assert_eq!(
            super::resolve(
                "c:/tmp/a",
                Some("/doc"),
                None,
                "p",
                "/doc",
                false,
                PathStyle::Posix
            )
            .map(|resolved| resolved.path),
            Ok("/doc/c:/tmp/a".to_owned())
        );
    }

    #[test]
    fn windows_jails_compare_posixified_paths_and_drive_roots() {
        assert_eq!(
            on_windows("c:\\tmp\\sdc.docx", "C:/Work/doc", Some("C:/Work/doc"), false),
            Err("to_dir c:\\tmp\\sdc.docx is outside of jail: C:/Work/doc (disallowed in safe mode)".to_owned())
        );
        assert_eq!(
            on_windows(
                "C:\\Work\\doc\\out\\a.docx",
                "C:/Work/doc",
                Some("C:\\Work\\doc"),
                false
            ),
            Ok(("C:/Work/doc/out/a.docx".to_owned(), vec![]))
        );
        assert_eq!(
            on_windows("D:/x/a", "C:/doc", Some("C:/doc"), true),
            Ok((
                "C:/doc/x/a".to_owned(),
                vec!["to_dir is outside of jail; recovering automatically".to_owned()]
            ))
        );
        // A start on another drive than the jail starts from the jail.
        assert_eq!(
            on_windows("a", "D:/elsewhere", Some("C:/doc"), true),
            Ok((
                "C:/doc/a".to_owned(),
                vec![
                    "start path for to_dir is outside of jail root; recovering automatically"
                        .to_owned()
                ]
            ))
        );
        assert_eq!(
            on_windows("a", "D:/elsewhere", Some("C:/doc"), false),
            Err("start path for to_dir D:/elsewhere refers to location outside jail root: C:/doc (disallowed in safe mode)".to_owned())
        );
        // The same drive is rechecked like a POSIX start.
        assert_eq!(
            on_windows("a", "C:/elsewhere", Some("C:/doc"), true),
            Ok((
                "C:/doc/a".to_owned(),
                vec!["to_dir is outside of jail; recovering automatically".to_owned()]
            ))
        );
    }

    /// Ruby's `File.dirname`, including drive roots on Windows.
    #[test]
    fn parent_directories_keep_their_roots() {
        let posix = |path| parent_dir(path, PathStyle::Posix);
        assert_eq!(posix("/doc/a.html"), "/doc");
        assert_eq!(posix("/a.html"), "/");
        assert_eq!(posix("/doc/out/"), "/doc");
        assert_eq!(posix("a.html"), ".");
        assert_eq!(posix("out/a.html"), "out");
        assert_eq!(posix("C:/a.html"), "C:");
        let windows = |path| parent_dir(path, PathStyle::Windows);
        assert_eq!(windows("C:/tmp/a.docx"), "C:/tmp");
        assert_eq!(windows("c:\\tmp\\a.docx"), "c:/tmp");
        assert_eq!(windows("C:/a.docx"), "C:/");
        assert_eq!(windows("C:/"), "C:/");
        assert_eq!(windows("//server/share/a"), "//server/share");
        assert_eq!(parent_system_dir("/doc/a.html"), "/doc");
    }

    #[test]
    fn normalize_uses_the_base_directory_as_start_and_jail() {
        let safe = normalize_system_path("../x", None, "/doc", SafeMode::Safe, "include file");
        assert_eq!(safe.path, "/doc/x");
        assert_eq!(safe.warnings.len(), 1);
        let unsafe_ = normalize_system_path("../x", None, "/doc", SafeMode::Unsafe, "include file");
        assert_eq!(unsafe_.path, "/x");
        assert!(unsafe_.warnings.is_empty());
        let relative = normalize_system_path("a", Some("img"), "/doc", SafeMode::Unsafe, "image");
        assert_eq!(relative.path, "/doc/img/a");
    }
}
