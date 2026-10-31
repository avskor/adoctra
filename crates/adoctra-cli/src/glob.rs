//! Ruby's `Dir.glob` for the CLI's input files (an argument that names no
//! file): `*`, `?`, `[...]` (`[!...]`/`[^...]` negate), `{a,b}` and `**/`
//! (any number of directories) per path segment; wildcards do not match a
//! leading `.`; results are sorted like Ruby's default `sort: true`.

use std::path::Path;

/// The paths matching `pattern`, relative like the pattern.
pub fn glob(pattern: &str) -> Vec<String> {
    let mut results = Vec::new();
    for pattern in expand_braces(pattern) {
        let (root, rest) = match pattern.strip_prefix('/') {
            Some(rest) => ("/".to_owned(), rest.to_owned()),
            None => (String::new(), pattern.clone()),
        };
        let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        walk(&root, &segments, &mut results);
    }
    results.sort();
    results.dedup();
    results
}

/// `{a,b}` alternatives (nested braces expand recursively).
fn expand_braces(pattern: &str) -> Vec<String> {
    let bytes = pattern.as_bytes();
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_owned()];
    };
    let mut depth = 0;
    let mut close = None;
    let mut commas = Vec::new();
    for (index, &byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(index);
                    break;
                }
            }
            b',' if depth == 1 => commas.push(index),
            _ => {}
        }
    }
    let Some(close) = close else {
        return vec![pattern.to_owned()];
    };
    let mut bounds = vec![open];
    bounds.extend(&commas);
    bounds.push(close);
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    bounds
        .windows(2)
        .flat_map(|w| expand_braces(&format!("{head}{}{tail}", &pattern[w[0] + 1..w[1]])))
        .collect()
}

fn join(base: &str, name: &str) -> String {
    match base {
        "" => name.to_owned(),
        "/" => format!("/{name}"),
        _ => format!("{base}/{name}"),
    }
}

fn entries(dir: &str) -> Vec<(String, bool)> {
    let path = if dir.is_empty() { "." } else { dir };
    let Ok(read) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    read.filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let is_dir = entry.path().is_dir();
            Some((name, is_dir))
        })
        .collect()
}

fn walk(base: &str, segments: &[&str], out: &mut Vec<String>) {
    let Some((&segment, rest)) = segments.split_first() else {
        if !base.is_empty() && Path::new(base).exists() {
            out.push(base.to_owned());
        }
        return;
    };
    if segment == "**" {
        // Zero directories, then each non-hidden subdirectory recursively.
        walk(base, rest, out);
        for (name, is_dir) in entries(base) {
            if is_dir && !name.starts_with('.') {
                walk(&join(base, &name), segments, out);
            }
        }
        return;
    }
    if !segment.contains(['*', '?', '[']) {
        let next = join(base, segment);
        if rest.is_empty() || Path::new(&next).is_dir() {
            walk(&next, rest, out);
        }
        return;
    }
    for (name, is_dir) in entries(base) {
        if name.starts_with('.') && !segment.starts_with('.') {
            continue;
        }
        if matches(segment.as_bytes(), name.as_bytes()) && (rest.is_empty() || is_dir) {
            walk(&join(base, &name), rest, out);
        }
    }
}

/// `File.fnmatch` for one segment.
fn matches(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|skip| matches(rest, &name[skip..])),
        Some((b'?', rest)) => !name.is_empty() && matches(rest, &name[1..]),
        Some((b'[', rest)) => {
            let Some(end) = rest.iter().skip(1).position(|&b| b == b']').map(|i| i + 1) else {
                return name.first() == Some(&b'[') && matches(rest, &name[1..]);
            };
            let (class, after) = (&rest[..end], &rest[end + 1..]);
            let (negate, class) = match class.first() {
                Some(b'!' | b'^') => (true, &class[1..]),
                _ => (false, class),
            };
            let Some(&first) = name.first() else {
                return false;
            };
            let mut hit = false;
            let mut index = 0;
            while index < class.len() {
                if index + 2 < class.len() && class[index + 1] == b'-' {
                    hit |= class[index] <= first && first <= class[index + 2];
                    index += 3;
                } else {
                    hit |= class[index] == first;
                    index += 1;
                }
            }
            hit != negate && matches(after, &name[1..])
        }
        Some((b'\\', rest)) if !rest.is_empty() => {
            name.first() == Some(&rest[0]) && matches(&rest[1..], &name[1..])
        }
        Some((&byte, rest)) => name.first() == Some(&byte) && matches(rest, &name[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_patterns() {
        assert!(matches(b"*.adoc", b"a.adoc"));
        assert!(!matches(b"*.adoc", b"a.html"));
        assert!(matches(b"?.adoc", b"a.adoc"));
        assert!(matches(b"[a-c].adoc", b"b.adoc"));
        assert!(!matches(b"[!a-c].adoc", b"b.adoc"));
        assert_eq!(expand_braces("{a,b{c,d}}.x"), ["a.x", "bc.x", "bd.x"]);
    }

    #[test]
    fn walks_directories() {
        let root = std::env::temp_dir().join(format!("adoctra-glob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for path in [
            "a.adoc",
            "b.txt",
            ".h.adoc",
            "sub/c.adoc",
            "sub/deep/d.adoc",
            ".hid/e.adoc",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        let base = root.to_str().unwrap();
        let rel = |found: Vec<String>| -> Vec<String> {
            found
                .iter()
                .map(|f| f[base.len() + 1..].to_owned())
                .collect()
        };
        assert_eq!(rel(glob(&format!("{base}/*.adoc"))), ["a.adoc"]);
        assert_eq!(
            rel(glob(&format!("{base}/**/*.adoc"))),
            ["a.adoc", "sub/c.adoc", "sub/deep/d.adoc"]
        );
        assert_eq!(rel(glob(&format!("{base}/{{a,b}}.*"))), ["a.adoc", "b.txt"]);
        assert!(glob(&format!("{base}/none*")).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
