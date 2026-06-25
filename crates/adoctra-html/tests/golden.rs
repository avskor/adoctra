//! Golden compatibility tests against the real asciidoctor.
//!
//! Each `tests/golden/<name>.adoc` has a committed `<name>.expected.html`
//! produced by asciidoctor 2.0.23 (see `golden/regenerate.sh`). We render the
//! same input with our HTML backend and compare the paragraph's inline markup
//! after normalizing whitespace and unwrapping asciidoctor's
//! `<div class="paragraph">` wrapper.
//!
//! This pins the *inline scope* of the current iteration (`*strong*`,
//! `_emphasis_`, `{attribute}` resolution, multi-line paragraph folding) to
//! asciidoctor's behavior. It runs entirely off committed fixtures, so no Ruby
//! is needed at test time — only to regenerate.

use std::fs;
use std::path::Path;

use adoctra_core::Backend;
use adoctra_html::HtmlBackend;

/// Inner text of the first `<p>…</p>`, with runs of whitespace collapsed so
/// asciidoctor's literal newline and our folded space compare equal.
fn paragraph_inner(html: &str) -> String {
    let start = html.find("<p>").expect("no <p> in output") + "<p>".len();
    let end = html[start..].find("</p>").expect("no </p> in output") + start;
    html[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Flatten a list into `(depth, principal_text)` per item, depth-aware so it
/// survives nesting. `depth` counts enclosing `<ul>`/`<ol>` (asciidoctor's
/// wrapping `<div class="ulist">` is a `<div>`, so it never affects the count —
/// and neither does our backend's lack of one). `principal_text` is the item's
/// own text up to its first nested list, with inner tags stripped and
/// whitespace collapsed (so asciidoctor's `<p>` wrapper drops out and the text
/// of nested items is excluded). Searching for `<ul`/`<ol` with the leading `<`
/// avoids matching the `ulist`/`olist` substrings in asciidoctor's class names.
fn flat_list(html: &str) -> Vec<(usize, String)> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut pos = 0usize;
    while pos < html.len() {
        let rest = &html[pos..];
        let next = ["<ul", "<ol", "</ul", "</ol", "<li>"]
            .iter()
            .filter_map(|m| rest.find(m).map(|i| (i, *m)))
            .min_by_key(|(i, _)| *i);
        let Some((rel, marker)) = next else { break };
        let at = pos + rel;
        match marker {
            "<ul" | "<ol" => {
                depth += 1;
                pos = at + marker.len();
            }
            "</ul" | "</ol" => {
                depth = depth.saturating_sub(1);
                pos = at + marker.len();
            }
            _ => {
                // "<li>": principal text runs until the item's first nested list
                // (`<ul`/`<ol`) or its close (`</li`).
                let after = at + "<li>".len();
                let tail = &html[after..];
                let stop = ["<ul", "<ol", "</li"]
                    .iter()
                    .filter_map(|m| tail.find(m))
                    .min()
                    .unwrap_or(tail.len());
                items.push((depth, strip_tags(&tail[..stop])));
                pos = after;
            }
        }
    }
    items
}

/// Drop all `<…>` tags and collapse whitespace runs to a single space.
fn strip_tags(s: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn html_inline_markup_matches_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut checked = 0usize;

    for entry in fs::read_dir(&dir).expect("golden dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let expected_path = path.with_extension("expected.html");
        if !expected_path.exists() {
            // No reference yet (fixtures not generated) — skip, don't fail.
            continue;
        }

        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let adoc = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(&expected_path).unwrap();

        let doc = adoctra_core::parse(&adoc).document;
        let ours = String::from_utf8(HtmlBackend::new().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            paragraph_inner(&ours),
            paragraph_inner(&expected),
            "HTML inline markup diverges from asciidoctor for `{name}`"
        );
        checked += 1;
    }

    assert!(checked > 0, "no golden fixtures found in {dir:?}");
}

#[test]
fn html_lists_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/lists");

    for entry in fs::read_dir(&dir).expect("golden/lists dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let expected_path = path.with_extension("expected.html");
        if !expected_path.exists() {
            // No reference yet (asciidoctor unavailable) — skip, don't fail.
            continue;
        }

        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let adoc = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(&expected_path).unwrap();

        let doc = adoctra_core::parse(&adoc).document;
        let ours = String::from_utf8(HtmlBackend::new().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            flat_list(&ours),
            flat_list(&expected),
            "HTML list structure diverges from asciidoctor for `{name}`"
        );
    }
}
