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

/// The inner text of the first `<div class="title">…</div>` (tags stripped,
/// whitespace collapsed). A titled block renders the same `<div class="title">` in
/// both processors — adoctra as a sibling / just inside the block element (no
/// wrapper), asciidoctor inside its wrapping `<div>` — so the title text compares
/// directly. This now *includes* the caption counter a numbered block carries
/// (asciidoctor's `Example 1. `, which adoctra emits too), so the comparison pins
/// caption numbering rather than stripping it.
fn block_titles(html: &str) -> Vec<String> {
    // Restrict to <body> for our full document (its inlined stylesheet has a
    // `.title` rule); asciidoctor's `-s` fragment has no <body> and falls through.
    let body = html.split("<body>").nth(1).unwrap_or(html);
    let marker = "<div class=\"title\">";
    let titles: Vec<_> = body
        .match_indices(marker)
        .map(|(at, _)| {
            let at = at + marker.len();
            let end = body[at..].find("</div>").expect("unterminated title") + at;
            strip_tags(&body[at..end])
        })
        .collect();
    assert!(!titles.is_empty(), "no block title in output");
    titles
}

/// The text of every cross-reference link (`<a href="#…">`), in order.
fn xref_texts(html: &str) -> Vec<String> {
    let body = html.split("<body>").nth(1).unwrap_or(html);
    let marker = "<a href=\"#";
    body.match_indices(marker)
        .map(|(at, _)| {
            let at = body[at..].find('>').unwrap() + at + 1;
            let end = body[at..].find("</a>").expect("unterminated link") + at;
            strip_tags(&body[at..end])
        })
        .collect()
}

/// Normalise an admonition to `(type, body markup)`. adoctra emits a modern
/// `<aside class="admonition" data-type="TYPE">…<p>BODY</p></aside>`; asciidoctor
/// a `<div class="admonitionblock TYPE"><table>…<td class="content">BODY</td>`.
/// We pin the admonition *type* and the body's inline markup; the surrounding
/// structure (aside vs table, the title/icon cell) is adoctra's deliberate
/// divergence and is not compared.
fn admonition(html: &str) -> (String, String, String) {
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

    // The label: asciidoctor's icon-cell title, adoctra's `admonition-title`.
    let label = [
        "<td class=\"icon\">\n<div class=\"title\">",
        "<p class=\"admonition-title\">",
    ]
    .iter()
    .find_map(|open| {
        let start = body.find(open)? + open.len();
        let end = body[start..].find("</")? + start;
        Some(body[start..end].to_string())
    })
    .unwrap_or_default();

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
    (ty, label, normalize_container(content, true))
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
            // Structural wrappers — drop them. `figure`/`figcaption` are adoctra's
            // quote/verse wrappers (asciidoctor uses `div`s), `aside` is adoctra's
            // sidebar wrapper, `section` its section wrapper and `nav` its TOC
            // (asciidoctor `div`s), so both collapse alike.
            "div" | "figure" | "figcaption" | "aside" | "section" | "nav" => {}
            "p" if strip_p => {} // admonition/quote body paragraph wrapper — drop it
            "pre" if !inner.starts_with('/') => out.push_str("<pre>"),
            _ => out.push_str(tag),
        }
        rest = &rest[gt + 1..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalise a sidebar block to its children's markup in document order. Like
/// [`example_body`]: asciidoctor nests `<div class="sidebarblock"><div
/// class="content">` with a `<div class="paragraph">` around each child; adoctra
/// emits a single `<aside class="sidebarblock">` with no inner wrappers. Stripping
/// every structural `<div>`/`<aside>` (and rewriting `<pre …>` to bare) collapses
/// both to the same shape — the children (`<p>`/`<pre>`/inline markup), which must
/// match.
fn sidebar_body(html: &str) -> String {
    let body = html.split("<body>").nth(1).unwrap_or(html);
    let body = body.split("</body>").next().unwrap_or(body);
    let at = body
        .find("sidebarblock")
        .expect("no sidebar block in output");
    let open = body[..at].rfind('<').expect("malformed sidebar block tag");
    normalize_container(&body[open..], false)
}

/// Normalise an open block to its children's markup in document order. Like
/// [`sidebar_body`]: asciidoctor nests `<div class="openblock"><div
/// class="content">` with a `<div class="paragraph">` around each child; adoctra
/// emits a single `<div class="openblock">` with no inner wrappers. Stripping
/// every structural `<div>` (and rewriting `<pre …>` to bare) collapses both to
/// the same shape — the children (`<p>`/`<pre>`/inline markup), which must match.
fn open_body(html: &str) -> String {
    let body = html.split("<body>").nth(1).unwrap_or(html);
    let body = body.split("</body>").next().unwrap_or(body);
    let at = body.find("openblock").expect("no open block in output");
    let open = body[..at].rfind('<').expect("malformed open block tag");
    normalize_container(&body[open..], false)
}

/// Normalise a quote or verse block to its quoted content plus attribution markup.
/// adoctra emits the HTML5 `<figure class="quoteblock|verseblock">` /
/// `<blockquote>`|`<pre>` / `<figcaption>` / `<cite>` idiom; asciidoctor nests
/// `<div class="quoteblock|verseblock">` / `<blockquote>`|`<pre class="content">` /
/// `<div class="attribution">` / `<cite>`. Stripping the structural wrappers
/// (`<div>`/`<figure>`/`<figcaption>`/`<p>`) and rewriting `<pre …>` to bare
/// collapses both to the same shape — the quoted text/markup, the em dash, the
/// author and the `<cite>` — which must match. (Whitespace is collapsed, so the
/// verse line-break *preservation* is pinned by the unit tests, not here.)
fn quote_verse_body(html: &str) -> String {
    let body = html.split("<body>").nth(1).unwrap_or(html);
    let body = body.split("</body>").next().unwrap_or(body);
    let at = body
        .find("quoteblock")
        .or_else(|| body.find("verseblock"))
        .expect("no quote/verse block in output");
    let open = body[..at].rfind('<').expect("malformed quote/verse tag");
    normalize_container(&body[open..], true)
}

/// Normalise a passthrough block's raw output. asciidoctor's `:pass` context and
/// adoctra both emit the body *completely raw* — no escaping and no wrapper
/// element at all — so unlike the container helpers this strips *nothing*: the
/// literal tags and text are the content and must match verbatim. We restrict to
/// <body> (adoctra inlines a stylesheet in <head>; asciidoctor's `-s` fragment
/// has no <body> and falls through unchanged) and only collapse whitespace.
fn passthrough_body(html: &str) -> String {
    let body = html.split("<body>").nth(1).unwrap_or(html);
    let body = body.split("</body>").next().unwrap_or(body);
    body.split_whitespace().collect::<Vec<_>>().join(" ")
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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            flat_list(&ours),
            flat_list(&expected),
            "HTML list structure diverges from asciidoctor for `{name}`"
        );
    }
}

/// Description lists as (tag, text) pairs: each `<dt>` term and `<dd>`
/// description with tags stripped and whitespace collapsed. Ruby wraps the
/// `<dl>` in a `div.dlist` and its descriptions in `<p>`; adoctra keeps the
/// `<dl>` alone.
fn flat_dlist(html: &str) -> Vec<(String, String)> {
    let body = html.split("<body>").nth(1).unwrap_or(html);
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(at) = ["<dt", "<dd"].iter().filter_map(|m| rest.find(m)).min() {
        let tag = rest[at + 1..at + 3].to_owned();
        let open_end = at + rest[at..].find('>').unwrap() + 1;
        let close = format!("</{tag}>");
        let end = open_end + rest[open_end..].find(&close).unwrap();
        // A description ends at its own close; a nested list's terms follow.
        let inner = &rest[open_end..end];
        let inner = inner.split("<dl").next().unwrap();
        let text = strip_tags(inner)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        out.push((tag, text));
        rest = &rest[open_end..];
    }
    out
}

#[test]
fn html_description_lists_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/dlists");
    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("golden/dlists dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let expected = fs::read_to_string(path.with_extension("expected.html")).unwrap();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let doc = adoctra_core::parse(&fs::read_to_string(&path).unwrap()).document;
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();
        assert_eq!(
            flat_dlist(&ours),
            flat_dlist(&expected),
            "HTML description list diverges from asciidoctor for `{name}`"
        );
        checked += 1;
    }
    assert!(checked > 0, "no golden fixtures found in {dir:?}");
}

/// Callouts: the conum markup inside the first `<pre>` matches Ruby's, and
/// the callout list items match its `colist` items.
#[test]
fn html_callouts_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/callouts");
    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("golden/callouts dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let expected = fs::read_to_string(path.with_extension("expected.html")).unwrap();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let doc = adoctra_core::parse(&fs::read_to_string(&path).unwrap()).document;
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();
        assert_eq!(
            pre_inner(&ours),
            pre_inner(&expected),
            "callout marks diverge for `{name}`"
        );
        assert_eq!(
            flat_list(&ours),
            flat_list(&expected),
            "callout list diverges for `{name}`"
        );
        checked += 1;
    }
    assert!(checked > 0, "no golden fixtures found in {dir:?}");
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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            pre_inner(&ours),
            pre_inner(&expected),
            "HTML literal-block content diverges from asciidoctor for `{name}`"
        );
    }
}

#[test]
fn html_passthrough_blocks_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/passthrough");

    for entry in fs::read_dir(&dir).expect("golden/passthrough dir is missing") {
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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            passthrough_body(&ours),
            passthrough_body(&expected),
            "HTML passthrough-block output diverges from asciidoctor for `{name}`"
        );
    }
}

#[test]
fn html_breaks_match_asciidoctor() {
    // Ruby renders thematic and page breaks without wrappers or metadata, so
    // the whole body compares like a passthrough.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/breaks");

    for entry in fs::read_dir(&dir).expect("golden/breaks dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let expected_path = path.with_extension("expected.html");
        if !expected_path.exists() {
            continue;
        }

        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let adoc = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(&expected_path).unwrap();

        let doc = adoctra_core::parse(&adoc).document;
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            passthrough_body(&ours),
            passthrough_body(&expected),
            "HTML break output diverges from asciidoctor for `{name}`"
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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            example_body(&ours),
            example_body(&expected),
            "HTML example-block content diverges from asciidoctor for `{name}`"
        );
    }
}

#[test]
fn html_sidebar_blocks_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/sidebars");

    for entry in fs::read_dir(&dir).expect("golden/sidebars dir is missing") {
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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            sidebar_body(&ours),
            sidebar_body(&expected),
            "HTML sidebar-block content diverges from asciidoctor for `{name}`"
        );
    }
}

#[test]
fn html_open_blocks_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/opens");

    for entry in fs::read_dir(&dir).expect("golden/opens dir is missing") {
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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            open_body(&ours),
            open_body(&expected),
            "HTML open-block content diverges from asciidoctor for `{name}`"
        );
    }
}

/// The whole body with structural wrappers removed: the TOC lists, images,
/// headings and paragraphs in order. Fixtures have no document title (asciidoctor's
/// reference omits it), so both sides start with the same content.
fn toc_body(html: &str) -> String {
    let body = html.split("<body>").nth(1).unwrap_or(html);
    let body = body.split("</body>").next().unwrap_or(body);
    normalize_container(body, false)
}

#[test]
fn html_tocs_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/toc");
    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("golden/toc dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let expected_path = path.with_extension("expected.html");
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let adoc = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(&expected_path).unwrap();
        let doc = adoctra_core::parse(&adoc).document;
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();
        assert_eq!(
            toc_body(&ours),
            toc_body(&expected),
            "HTML TOC diverges from asciidoctor for `{name}`"
        );
        checked += 1;
    }
    assert!(checked > 0, "no golden fixtures found in {dir:?}");
}

/// Images: asciidoctor's `imageblock`/`content`/`title` divs and adoctra's
/// `<figure>`/`<figcaption>` both reduce to the image (and link) tags and the
/// captioned title.
#[test]
fn html_images_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/images");
    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("golden/images dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let expected_path = path.with_extension("expected.html");
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let adoc = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(&expected_path).unwrap();
        let doc = adoctra_core::parse(&adoc).document;
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();
        assert_eq!(
            toc_body(&ours),
            toc_body(&expected),
            "HTML image diverges from asciidoctor for `{name}`"
        );
        checked += 1;
    }
    assert!(checked > 0, "no golden fixtures found in {dir:?}");
}

/// Video and audio blocks: asciidoctor's `videoblock`/`audioblock` divs with a
/// leading title and a `content` wrapper, and adoctra's `<figure>` with a
/// leading `<figcaption>`, reduce to the title and the player tags; Ruby's
/// obsolete `frameborder="0"` on embedded players is stylesheet presentation.
#[test]
fn html_media_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/media");
    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("golden/media dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let expected_path = path.with_extension("expected.html");
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let adoc = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(&expected_path).unwrap();
        let doc = adoctra_core::parse(&adoc).document;
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();
        assert_eq!(
            toc_body(&ours),
            toc_body(&expected.replace(" frameborder=\"0\"", "")),
            "HTML media diverges from asciidoctor for `{name}`"
        );
        checked += 1;
    }
    assert!(checked > 0, "no golden fixtures found in {dir:?}");
}

/// Tables: asciidoctor's `<table class="tableblock">` markup is already
/// semantic, so adoctra emits it unchanged and the body matches line for line.
#[test]
fn html_tables_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/tables");
    let lines = |html: &str| -> Vec<String> {
        let body = html.split("<body>").nth(1).unwrap_or(html);
        let body = body.split("</body>").next().unwrap_or(body);
        body.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("golden/tables dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let expected_path = path.with_extension("expected.html");
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let adoc = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(&expected_path).unwrap();
        let doc = adoctra_core::parse(&adoc).document;
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();
        assert_eq!(
            lines(&ours),
            lines(&expected),
            "HTML table diverges from asciidoctor for `{name}`"
        );
        checked += 1;
    }
    assert!(checked > 0, "no golden fixtures found in {dir:?}");
}

/// Footnotes: Ruby's markers and footnotes list are adoctra's markup; the
/// paragraph wrappers reduce as for the other body fixtures.
#[test]
fn html_footnotes_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/footnotes");
    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("golden/footnotes dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let expected_path = path.with_extension("expected.html");
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let adoc = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(&expected_path).unwrap();
        let doc = adoctra_core::parse(&adoc).document;
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();
        assert_eq!(
            toc_body(&ours),
            toc_body(&expected),
            "HTML footnotes diverge from asciidoctor for `{name}`"
        );
        checked += 1;
    }
    assert!(checked > 0, "no golden fixtures found in {dir:?}");
}

#[test]
fn html_block_titles_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/titles");

    for entry in fs::read_dir(&dir).expect("golden/titles dir is missing") {
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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            block_titles(&ours),
            block_titles(&expected),
            "HTML block titles diverge from asciidoctor for `{name}`"
        );
        assert_eq!(
            xref_texts(&ours),
            xref_texts(&expected),
            "HTML cross-reference labels diverge from asciidoctor for `{name}`"
        );
    }
}

#[test]
fn html_quote_blocks_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/quotes");

    for entry in fs::read_dir(&dir).expect("golden/quotes dir is missing") {
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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            quote_verse_body(&ours),
            quote_verse_body(&expected),
            "HTML quote-block content diverges from asciidoctor for `{name}`"
        );
    }
}

#[test]
fn html_verse_blocks_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/verses");

    for entry in fs::read_dir(&dir).expect("golden/verses dir is missing") {
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
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();

        assert_eq!(
            quote_verse_body(&ours),
            quote_verse_body(&expected),
            "HTML verse-block content diverges from asciidoctor for `{name}`"
        );
    }
}

#[test]
fn offset_heading_levels_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("leveloffset-attributes.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("leveloffset-attributes.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    let headings = |html: &str| -> Vec<String> {
        html.lines()
            .map(str::trim)
            .filter(|line| {
                line.starts_with("<h") && line.as_bytes().get(2).is_some_and(u8::is_ascii_digit)
            })
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(headings(&ours), headings(&expected));
    assert_eq!(headings(&ours).len(), 3);
}

/// The section structure of rendered HTML: `sectN`/preamble containers (Ruby's
/// `<div>` or adoctra's `<section>`, with any extra classes), their ends, and
/// every heading and paragraph with whitespace collapsed. Ruby's `sectionbody`
/// and other wrappers are ignored.
fn section_outline(html: &str) -> Vec<String> {
    let body = html.split("<body>").nth(1).unwrap_or(html);
    let mut out = Vec::new();
    let mut stack = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find('<') {
        let close = rest[open..].find('>').unwrap() + open;
        let tag = &rest[open + 1..close];
        let name = tag.split([' ', '/']).find(|s| !s.is_empty()).unwrap_or("");
        let after = &rest[close + 1..];
        if let Some(closing) = tag.strip_prefix('/') {
            if closing == "div" || closing == "section" {
                if let Some(Some(label)) = stack.pop() {
                    out.push(format!("end {label}"));
                }
            }
        } else if name == "div" || name == "section" {
            let class = tag
                .split("class=\"")
                .nth(1)
                .map(|c| c.split('"').next().unwrap());
            let label = if tag.contains("id=\"preamble\"") {
                Some("preamble".to_owned())
            } else {
                class
                    .filter(|c| c.starts_with("sect") && !c.starts_with("sectionbody"))
                    .map(str::to_owned)
            };
            if let Some(label) = &label {
                out.push(label.clone());
            }
            stack.push(label);
        } else if name == "p" || (name.len() == 2 && name.starts_with('h')) {
            let end = after.find(&format!("</{name}>")).unwrap();
            let text = after[..end]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            out.push(format!("<{tag}>{text}"));
        }
        rest = after;
    }
    out
}

#[test]
fn section_hierarchy_and_preamble_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("sections.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("sections.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    // The reference omits the document title (no `showtitle`).
    let mut outline = section_outline(&ours);
    assert_eq!(outline.remove(0), "<h1>Doc");
    assert_eq!(outline, section_outline(&expected));
    assert!(outline.contains(&"sect1 special".to_owned()), "{outline:?}");
    assert!(
        outline.contains(&"<p>Para == Not a title".to_owned()),
        "{outline:?}"
    );
}

#[test]
fn setext_body_sections_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("section-setext.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("section-setext.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    // Ruby reports the same skipped level for `Part` (line 21).
    let messages: Vec<_> = result.diagnostics.iter().map(|d| &*d.message).collect();
    assert_eq!(
        messages,
        ["section title out of sequence: expected level 2, got level 4"]
    );
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    let headings = |html: &str| -> Vec<String> {
        html.lines()
            .map(str::trim)
            .filter(|line| {
                line.starts_with("<h")
                    && line
                        .as_bytes()
                        .get(2)
                        .is_some_and(|b| (b'2'..=b'6').contains(b))
            })
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(headings(&ours), headings(&expected));
    assert_eq!(headings(&ours).len(), 6);
    // The resolved xref borrows the setext title; underlines never leak as text.
    assert!(ours.contains(r##"<a href="#_deep">Deep</a>"##), "{ours}");
    assert!(expected.contains(r##"<a href="#_deep">Deep</a>"##));
    assert!(!ours.contains("~~~~") && !ours.contains("++++"), "{ours}");
}

#[test]
fn setext_document_title_and_metadata_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("header-setext.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("header-setext.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    let heading = |html: &str| {
        html.lines()
            .find(|line| line.starts_with("<h1>"))
            .unwrap()
            .to_owned()
    };
    assert_eq!(heading(&ours), heading(&expected));
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert_eq!(heading(&ours), "<h1><strong>Документ</strong></h1>");
}

#[test]
fn doctitle_override_and_attribute_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("header-doctitle.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("header-doctitle.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.document.header.as_ref().unwrap().title, "Исходный");
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    let heading = |html: &str| {
        html.lines()
            .find(|line| line.starts_with("<h1>"))
            .unwrap()
            .to_owned()
    };
    assert_eq!(heading(&ours), heading(&expected));
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
}

#[test]
fn indexed_author_names_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("header-indexed-authors.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("header-indexed-authors.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert_eq!(
        result.document.header.unwrap().authors[1].name,
        "Иван Петров"
    );
}

#[test]
fn author_markup_and_literal_values_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("header-author-markup.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("header-author-markup.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert!(ours.contains("<strong>Анна</strong> Смирнова"));
    assert!(ours.contains("&lt;b&gt;Literal&lt;/b&gt; Name"));
}

#[test]
fn attribute_entities_and_escapes_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("attribute-entities.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("attribute-entities.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert!(ours.contains("<h1>Entities &copy; &#65; &#x1F642;</h1>"));
    assert!(ours.contains("<p>&amp;copy;</p>"));
    assert!(ours.contains("<strong>&copy;</strong>"));
}

#[test]
fn shared_replacements_match_reference_including_raw_and_escaped_entities() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("shared-replacements.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("shared-replacements.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert!(ours.contains("<h1>&#169; &copy;</h1>"));
    assert!(ours.contains("<p>(C) &amp;copy;</p>"));
}

#[test]
fn pass_quotes_preserve_semantic_formatting_and_ordered_substitutions() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("pass-quotes.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("pass-quotes.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert!(ours.contains("<strong><em>Ada</em></strong>"));
    assert!(ours.contains("<strong>_Ada_</strong>"));
    assert!(ours.contains("&lt;strong&gt;A &amp; B&lt;/strong&gt;"));
}

#[test]
fn pass_replacements_match_reference_with_ordered_protection_and_expansion() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("pass-replacements.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("pass-replacements.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert!(ours.contains("<p>&#169; -></p>"));
    assert!(ours.contains("<p>&lt;b&gt;&#174;&lt;/b&gt; &#8594;</p>"));
    assert!(ours.contains("<p>(C)</p>"));
}

#[test]
fn section_ids_strip_entities_and_raw_tags_and_preserve_heading_and_link_markup() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("section-id-entities.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("section-id-entities.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    let headings = |html: &str| -> Vec<String> {
        html.lines()
            .filter(|line| line.starts_with("<h2"))
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(headings(&ours), headings(&expected));
    assert_eq!(headings(&ours).len(), 5);
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert!(ours.contains("<h2 id=\"\">&#169; &#8594;</h2>"));
    assert!(ours.contains("<a href=\"#_alpha\"><b>Alpha</b> &copy;</a>"));
}

#[test]
fn quote_attributes_match_reference_and_ordered_protection() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("pass-quote-attributes.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("pass-quote-attributes.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert!(ours.contains("<strong id=\"anchor\" class=\"red\">Ж&amp;</strong>"));
    assert!(ours.contains("<span class=\"\">empty role</span>"));
    assert!(ours.contains("&lt;strong id=\"id\" class=\"red\"&gt;literal&lt;/strong&gt;"));
}

#[test]
fn ordered_inline_quotes_match_reference_with_metadata_and_escapes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("inline-quotes.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("inline-quotes.expected.html")).unwrap();
    let actual = String::from_utf8(
        HtmlBackend::embedded()
            .convert(&adoctra_core::parse(&source).document)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(paragraph_inner(&actual), paragraph_inner(&expected));
}

#[test]
fn direct_link_macros_follow_ordered_substitutions() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("direct-link-macros.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("direct-link-macros.expected.html")).unwrap();
    let doc = adoctra_core::parse(&source).document;
    let actual = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();
    assert_eq!(paragraph_inner(&actual), paragraph_inner(&expected));
}

#[test]
fn post_attribute_link_macros_match_reference_across_text_and_reference_parts() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("post-attribute-macros.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("post-attribute-macros.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert!(ours.contains("<a href=\"page\" class=\"red\" title=\"Title\">Ж&amp;</a>"));
    assert!(ours.contains("<a href=\"page\"><strong>Bold</strong></a>"));
}

#[test]
fn pass_link_macros_match_reference_and_preserve_ordered_protection() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("pass-link-macros.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("pass-link-macros.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty());
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert!(ours.contains("<a href=\"page\" id=\"anchor\" class=\"red\" title=\"Title\" target=\"_blank\" rel=\"nofollow noopener\"><strong>Ж&amp;</strong></a>"));
    assert!(ours.contains("<a href=\"#_alpha\">Alpha</a>"));
    assert!(ours.contains("<h2 id=\"_alpha\"><a href=\"page\">Alpha</a></h2>"));
}

/// Manual pages: the NAME section (in Ruby's header, in adoctra a section),
/// the body's section titles and paragraphs, in order. The references are
/// embedded without the document title, which adoctra always renders.
#[test]
fn html_manual_pages_match_asciidoctor() {
    fn elements(html: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = html;
        while let Some(start) = rest.find('<') {
            rest = &rest[start..];
            let tag = ["h2", "h3", "p"].into_iter().find(|tag| {
                rest[1..].starts_with(tag)
                    && matches!(rest.as_bytes().get(1 + tag.len()), Some(b'>' | b' '))
            });
            let Some(tag) = tag else {
                rest = &rest[1..];
                continue;
            };
            let close = format!("</{tag}>");
            let end = rest.find(&close).unwrap() + close.len();
            found.push(rest[..end].split_whitespace().collect::<Vec<_>>().join(" "));
            rest = &rest[end..];
        }
        found
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/manpage");
    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("golden/manpage dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let adoc = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(path.with_extension("expected.html")).unwrap();
        let doc = adoctra_core::parse(&adoc).document;
        let ours = String::from_utf8(HtmlBackend::embedded().convert(&doc).unwrap()).unwrap();
        assert!(ours.contains(" Manual Page</h1>"), "{ours}");
        assert!(!elements(&expected).is_empty());
        assert_eq!(
            elements(&ours),
            elements(&expected),
            "HTML manual page diverges from asciidoctor for `{name}`"
        );
        checked += 1;
    }
    assert!(checked > 0, "no golden fixtures found in {dir:?}");
}

#[test]
fn section_id_policy_matches_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let source = fs::read_to_string(dir.join("section-id-policy.adoc")).unwrap();
    let expected = fs::read_to_string(dir.join("section-id-policy.expected.html")).unwrap();
    let result = adoctra_core::parse(&source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let ours =
        String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap();
    // Headings, TOC entries and resolved references carry the same ids.
    let ids = |html: &str| -> Vec<String> {
        html.lines()
            .filter(|line| {
                line.starts_with("<h2") || line.starts_with("<h3") || line.starts_with("<li>")
            })
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(ids(&ours), ids(&expected));
    assert_eq!(ids(&ours).len(), 11);
    assert_eq!(paragraph_inner(&ours), paragraph_inner(&expected));
    assert!(ours.contains("<h2 id=\"hello-world-2\">Hello World -</h2>"));
}

#[test]
fn html_section_titles_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/sections");
    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("golden/sections dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let source = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(path.with_extension("expected.html")).unwrap();
        let ours = String::from_utf8(
            HtmlBackend::embedded()
                .convert(&adoctra_core::parse(&source).document)
                .unwrap(),
        )
        .unwrap();
        // Section and floating titles (anchors, links and numbers) line by line.
        let headings = |html: &str| -> Vec<String> {
            html.lines()
                .filter(|line| {
                    line.len() > 2
                        && line.starts_with("<h")
                        && line.as_bytes()[2].is_ascii_digit()
                        && !line.starts_with("<h1>")
                })
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(
            headings(&ours),
            headings(&expected),
            "section titles diverge from asciidoctor for `{name}`"
        );
        assert!(!headings(&ours).is_empty(), "{name}");
        checked += 1;
    }
    assert!(checked > 0, "no golden fixtures found in {dir:?}");
}

/// The document shell of a standalone document: the head without the
/// stylesheet, X-UA-Compatible and generator, the header, footnotes and
/// footer, with the content replaced by a marker. adoctra's semantic
/// `<header>`/`<main>`/`<footer>`/`<nav>` map to Ruby's divs and its
/// escaped quotes in `content` attributes to Ruby's literal ones.
fn document_shell(html: &str) -> Vec<String> {
    let mut html = html.to_owned();
    if let (Some(start), Some(end)) = (html.find("<style>\n"), html.find("</style>\n")) {
        html.replace_range(start..end + "</style>\n".len(), "");
    }
    let lines: Vec<String> = html
        .lines()
        // adoctra wraps a manual page's NAME section in a `sect1` section
        // where Ruby's has a `sectionbody` div after the title.
        .filter(|line| {
            !line.starts_with("<meta http-equiv")
                && !line.starts_with("<meta name=\"generator\"")
                && *line != "<section class=\"sect1\">"
                && *line != "<div class=\"sectionbody\">"
        })
        .map(|line| {
            let line = line
                .replace("<header id=\"header\"", "<div id=\"header\"")
                .replace("</header>", "</div>")
                .replace("<main id=\"content\"", "<div id=\"content\"")
                .replace("</main>", "</div>")
                .replace("<footer id=\"footer\"", "<div id=\"footer\"")
                .replace("</footer>", "</div>")
                .replace("<nav id=\"toc\"", "<div id=\"toc\"")
                .replace("</nav>", "</div>")
                .replace("</section>", "</div>");
            if line.starts_with("<meta ") {
                line.replace("&quot;", "\"")
            } else {
                line
            }
        })
        .collect();
    let Some(start) = lines
        .iter()
        .position(|l| l.starts_with("<div id=\"content\""))
    else {
        return lines;
    };
    let end = lines
        .iter()
        .position(|l| {
            l.starts_with("<div id=\"footnotes\"")
                || l.starts_with("<div id=\"footer\"")
                || l == "</body>"
        })
        .unwrap();
    let mut shell = lines[..start].to_vec();
    shell.push("<content/>".to_owned());
    shell.extend_from_slice(&lines[end..]);
    shell
}

#[test]
fn html_standalone_documents_match_asciidoctor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/document");
    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("golden/document dir is missing") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("adoc") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let source = fs::read_to_string(&path).unwrap();
        let expected = fs::read_to_string(path.with_extension("expected.html")).unwrap();
        // The reference converts with `reproducible`, which hides the
        // last-update line adoctra omits without date attributes.
        let ours = String::from_utf8(
            HtmlBackend::new()
                .convert(&adoctra_core::parse(&source).document)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            document_shell(&ours),
            document_shell(&expected),
            "document shell diverges from asciidoctor for `{name}`"
        );
        checked += 1;
    }
    assert!(checked > 0, "no golden fixtures found in {dir:?}");
}
