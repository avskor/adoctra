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
/// asciidoctor's literal newline and our folded space compare equal. The open
/// tag may carry attributes (`<p id="…" class="…">`): asciidoctor hangs a
/// block's id/role on a wrapping `<div>`, but our backend emits no wrapper and
/// puts them on the `<p>` itself, so we match `<p>` or `<p …>` (but not `<pre>`)
/// and take the content after the tag's closing `>`.
fn paragraph_inner(html: &str) -> String {
    let open = ["<p>", "<p "]
        .iter()
        .filter_map(|m| html.find(m))
        .min()
        .expect("no <p> in output");
    let start = html[open..].find('>').expect("unterminated <p tag") + open + 1;
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

/// The first `<pre …>…</pre>` block (open tag, inner markup incl. any
/// `<code …>`, and close), with whitespace runs collapsed. asciidoctor wraps a
/// listing in `<div class="listingblock"><div class="content">`; our backend
/// emits no wrapper, so comparing just the `<pre>` aligns the two.
fn listing_block(html: &str) -> String {
    let start = html.find("<pre").expect("no <pre> in output");
    let end = html[start..].find("</pre>").expect("no </pre> in output") + start + "</pre>".len();
    html[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Inner markup of the first `<pre …>…</pre>` (after the open tag, before the
/// close), with whitespace runs collapsed. Unlike [`listing_block`] this drops
/// the open tag, so it tolerates adoctra's deliberate class divergence on
/// literal blocks: adoctra emits `<pre class="literalblock">` while asciidoctor
/// hangs `literalblock` on a wrapping `<div>` and leaves the `<pre>` bare.
/// Comparing the *content* still pins the verbatim text and HTML escaping.
fn pre_inner(html: &str) -> String {
    let open = html.find("<pre").expect("no <pre> in output");
    let start = html[open..].find('>').expect("unterminated <pre tag") + open + 1;
    let end = html[start..].find("</pre>").expect("no </pre> in output") + start;
    html[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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

/// Normalise an admonition to `(type, body markup)`. adoctra emits a modern
/// `<aside class="admonition" data-type="TYPE">…<p>BODY</p></aside>`; asciidoctor
/// a `<div class="admonitionblock TYPE"><table>…<td class="content">BODY</td>`.
/// We pin the admonition *type* and the body's inline markup; the surrounding
/// structure (aside vs table, the title/icon cell) is adoctra's deliberate
/// divergence and is not compared.
fn admonition(html: &str) -> (String, String) {
    // Restrict to <body>: adoctra inlines its stylesheet in <head>, which also
    // mentions `data-type` and `.admonition`. asciidoctor's `-s` fragment has no
    // <body>, so it falls through unchanged.
    let body = html.split("<body>").nth(1).unwrap_or(html);

    let ty = if let Some(i) = body.find("data-type=\"") {
        let rest = &body[i + "data-type=\"".len()..];
        rest[..rest.find('"').unwrap()].to_string()
    } else {
        let i = body
            .find("admonitionblock ")
            .expect("no admonition in output")
            + "admonitionblock ".len();
        let rest = &body[i..];
        let end = rest.find(['"', ' ']).unwrap_or(rest.len());
        rest[..end].to_string()
    };

    let content = if let Some(i) = body.find("<td class=\"content\">") {
        let start = i + "<td class=\"content\">".len();
        let end = body[start..].find("</td>").unwrap() + start;
        &body[start..end]
    } else {
        // adoctra: the body blocks follow the title <p> and run to the closing
        // </aside> (a delimited admonition holds several blocks, not just one <p>).
        let after_title = body.find("</p>").map(|i| i + "</p>".len()).unwrap_or(0);
        let seg = &body[after_title..];
        let end = seg.find("</aside>").unwrap_or(seg.len());
        &seg[..end]
    };

    // Both bodies normalise to their surviving inline markup: asciidoctor wraps
    // each child in `<div class="paragraph"><p>…</p></div>` (and its shorthand form
    // emits bare text), adoctra in a bare `<p>…</p>` — so drop the `<div>` and `<p>`
    // wrappers on both sides and compare what is left.
    (ty, normalize_container(content, true))
}

/// Normalise an example block to its children's markup in document order.
/// asciidoctor nests `<div class="exampleblock"><div class="content">` with a
/// `<div class="paragraph">` around each child; adoctra emits a single
/// `<div class="exampleblock">` with no inner wrappers. We strip every structural
/// `<div>` (so both collapse to the same shape) and rewrite `<pre …>` to a bare
/// `<pre>` to absorb adoctra's deliberate listing/literal class divergence — what
/// remains is the children (`<p>`/`<pre>`/inline markup), which must match.
fn example_body(html: &str) -> String {
    // Restrict to <body> so adoctra's inlined stylesheet (which mentions
    // `.exampleblock`) is ignored; asciidoctor's `-s` fragment has no <body> and
    // falls through unchanged.
    let body = html.split("<body>").nth(1).unwrap_or(html);
    let body = body.split("</body>").next().unwrap_or(body);
    let at = body
        .find("exampleblock")
        .expect("no example block in output");
    let open = body[..at].rfind('<').expect("malformed example block tag");
    normalize_container(&body[open..], false)
}

/// Drop `<div …>`/`</div>` wrapper tags, rewrite `<pre …>` open tags to a bare
/// `<pre>`, optionally also drop `<p>`/`</p>` (admonition bodies — asciidoctor's
/// shorthand form emits bare text while adoctra wraps in `<p>`), keep all other
/// tags and text, then collapse whitespace.
fn normalize_container(s: &str, strip_p: bool) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let gt = rest[lt..].find('>').map_or(rest.len(), |i| lt + i);
        let tag = &rest[lt..gt.min(rest.len() - 1) + 1];
        let inner = tag.trim_matches(['<', '>']).trim();
        let bare = inner.trim_start_matches('/').trim();
        let name = bare.split_whitespace().next().unwrap_or("");
        match name {
            "div" => {}          // structural wrapper — drop it
            "p" if strip_p => {} // admonition body paragraph wrapper — drop it
            "pre" if !inner.starts_with('/') => out.push_str("<pre>"),
            _ => out.push_str(tag),
        }
        rest = &rest[gt + 1..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
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

#[test]
fn html_source_blocks_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/blocks");

    for entry in fs::read_dir(&dir).expect("golden/blocks dir is missing") {
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
            listing_block(&ours),
            listing_block(&expected),
            "HTML source-block markup diverges from asciidoctor for `{name}`"
        );
    }
}

#[test]
fn html_literal_blocks_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/literals");

    for entry in fs::read_dir(&dir).expect("golden/literals dir is missing") {
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
            pre_inner(&ours),
            pre_inner(&expected),
            "HTML literal-block content diverges from asciidoctor for `{name}`"
        );
    }
}

#[test]
fn html_admonition_blocks_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/admonitions");

    for entry in fs::read_dir(&dir).expect("golden/admonitions dir is missing") {
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
            admonition(&ours),
            admonition(&expected),
            "HTML admonition type/content diverges from asciidoctor for `{name}`"
        );
    }
}

#[test]
fn html_example_blocks_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/examples");

    for entry in fs::read_dir(&dir).expect("golden/examples dir is missing") {
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
            example_body(&ours),
            example_body(&expected),
            "HTML example-block content diverges from asciidoctor for `{name}`"
        );
    }
}
