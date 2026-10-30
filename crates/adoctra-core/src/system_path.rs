//! Ruby's `PathResolver#system_path` and `AbstractNode#normalize_system_path`
//! for POSIX paths: lexical resolution of a target against a start
//! directory, confined to a jail (the document's base directory from safe
//! mode up) with Ruby's automatic recovery and its warnings. No I/O: the
//! result names the path Ruby reads and reports (e.g. "include file not
//! found: <path>"); the host provider reads it.
use crate::SafeMode;

/// A resolved path and the warnings Ruby logs while resolving it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub path: String,
    pub warnings: Vec<String>,
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
    let mut warnings = Vec::new();
    let outside = |warnings: &mut Vec<String>| {
        warnings.push(format!(
            "{target_name} is outside of jail; recovering automatically"
        ));
    };
    if target.starts_with('/') {
        let target_path = expand(target);
        if let Some(jail) = jail.filter(|jail| !descends_from(&target_path, jail)) {
            outside(&mut warnings);
            let (target_segments, _) = partition(&target_path);
            let (mut jail_segments, jail_root) = partition(jail);
            jail_segments.extend(target_segments);
            return Resolved {
                path: join(&jail_segments, jail_root),
                warnings,
            };
        }
        return Resolved {
            path: target_path,
            warnings,
        };
    }
    let fallback = jail.unwrap_or(working_dir);
    let mut target_segments = partition(target).0;
    let start = match start.filter(|start| !start.is_empty()) {
        _ if target_segments.is_empty() => match start.filter(|start| !start.is_empty()) {
            None => {
                return Resolved {
                    path: fallback.to_owned(),
                    warnings,
                }
            }
            Some(start) if start.starts_with('/') => {
                if jail.is_none() {
                    return Resolved {
                        path: expand(start),
                        warnings,
                    };
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
            return Resolved {
                path: target_path,
                warnings,
            };
        }
        outside(&mut warnings);
        let (mut jail_segments, jail_root) = partition(jail);
        jail_segments.extend(target_segments.iter().copied());
        return Resolved {
            path: join(&jail_segments, jail_root),
            warnings,
        };
    }
    Resolved {
        path: join(&resolved, root),
        warnings,
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
