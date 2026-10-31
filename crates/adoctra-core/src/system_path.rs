//! Ruby's `PathResolver#system_path` and `AbstractNode#normalize_system_path`
//! for POSIX paths: lexical resolution of a target against a start
//! directory, confined to a jail (the document's base directory from safe
//! mode up) with Ruby's automatic recovery and its warnings. No I/O: the
//! result names the path Ruby reads and reports (e.g. "include file not
//! found: <path>"); the host provider reads it.
use crate::SafeMode;

/// A resolved path and the warnings Ruby logs while resolving it.
#[derive(Debug, PartialEq, Eq)]
pub struct SystemPath {
    pub path: String,
    pub warnings: Vec<String>,
}

pub(crate) type Resolved = SystemPath;

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
    resolve(target, Some(start), jail, target_name, start, recover)
}

/// Ruby's `partition_path` (no web paths): segments without `.` entries
/// and the root (`/`, `//` or `./`), if any.
fn partition(path: &str) -> (Vec<&str>, Option<&'static str>) {
    let root = if path.starts_with("//") {
        Some("//")
    } else if path.starts_with('/') {
        Some("/")
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

/// Ruby's `expand_path` (lexical).
fn expand(path: &str) -> String {
    let (segments, root) = partition(path);
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
    resolve(target, start, jail, target_name, working_dir, true)
        .expect("recovering resolution never fails")
}

fn resolve(
    target: &str,
    start: Option<&str>,
    jail: Option<&str>,
    target_name: &str,
    working_dir: &str,
    recover: bool,
) -> Result<Resolved, String> {
    let mut warnings = Vec::new();
    let outside = |warnings: &mut Vec<String>, jail: &str| {
        if !recover {
            return Err(format!(
                "{target_name} {target} is outside of jail: {jail} (disallowed in safe mode)"
            ));
        }
        warnings.push(format!(
            "{target_name} is outside of jail; recovering automatically"
        ));
        Ok(())
    };
    if target.starts_with('/') {
        let target_path = expand(target);
        if let Some(jail) = jail.filter(|jail| !descends_from(&target_path, jail)) {
            outside(&mut warnings, jail)?;
            let (target_segments, _) = partition(&target_path);
            let (mut jail_segments, jail_root) = partition(jail);
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
    let mut target_segments = partition(target).0;
    let start = match start.filter(|start| !start.is_empty()) {
        _ if target_segments.is_empty() => match start.filter(|start| !start.is_empty()) {
            None => {
                return Ok(Resolved {
                    path: fallback.to_owned(),
                    warnings,
                });
            }
            Some(start) if start.starts_with('/') => {
                if jail.is_none() {
                    return Ok(Resolved {
                        path: expand(start),
                        warnings,
                    });
                }
                start.to_owned()
            }
            Some(start) => {
                target_segments = partition(start).0;
                fallback.to_owned()
            }
        },
        None => fallback.to_owned(),
        Some(start) if start.starts_with('/') => start.to_owned(),
        Some(start) => format!("{}/{start}", fallback.trim_end_matches('/')),
    };
    let recheck = jail.is_some_and(|jail| !descends_from(&start, jail));
    let (start_segments, root) = partition(&start);
    let mut resolved: Vec<&str> = start_segments;
    resolved.extend(target_segments.iter().copied());
    if resolved.contains(&"..") {
        let unresolved = std::mem::take(&mut resolved);
        if let Some(jail) = jail {
            let jail_depth = partition(jail).0.len();
            let mut warned = false;
            for segment in unresolved {
                if segment != ".." {
                    resolved.push(segment);
                } else if resolved.len() > jail_depth {
                    resolved.pop();
                } else if !recover {
                    return Err(format!(
                        "{target_name} {target} refers to location outside jail: {jail} (disallowed in safe mode)"
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
        let (mut jail_segments, jail_root) = partition(jail);
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
            Some(start) if start.starts_with('/') => start.to_owned(),
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
