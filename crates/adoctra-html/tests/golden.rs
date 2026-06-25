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

/// Visible text of each `<li>…</li>`, with inner tags stripped and whitespace
/// collapsed. Robust to asciidoctor wrapping item text in `<p>` while our
/// backend emits `<li>` content directly.
fn list_items(html: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut rest = html;
    while let Some(open) = rest.find("<li>") {
        let after = &rest[open + "<li>".len()..];
        let close = after.find("</li>").expect("unbalanced <li> in output");
        items.push(strip_tags(&after[..close]));
        rest = &after[close + "</li>".len()..];
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
            list_items(&ours),
            list_items(&expected),
            "HTML list items diverge from asciidoctor for `{name}`"
        );
    }
}
