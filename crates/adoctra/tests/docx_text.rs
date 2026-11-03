//! Text differential between the DOCX and HTML backends.
//!
//! Every input of the HTML golden corpus (`crates/adoctra-html/tests/golden`)
//! is converted by both backends from the same parse. Their words are
//! compared as multisets (layout order differs: footnotes, captions): words
//! the HTML shows but the DOCX lacks, and words only the DOCX has. Ruby has
//! no DOCX converter, so this pins the DOCX content to the HTML backend's,
//! which the golden tests pin to Ruby.
//!
//! Known differences are frozen in `docx_text.baseline` (one line per input
//! that differs); any drift fails, including a difference that disappears.
//! Regenerate after reviewing the change with
//! `ADOCTRA_DOCX_TEXT_REGENERATE=1 cargo test -p adoctra --all-features --test docx_text`.
#![cfg(all(feature = "html", feature = "docx"))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use adoctra::Backend;

fn inputs(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            inputs(&path, out);
        } else if path.extension().is_some_and(|e| e == "adoc") {
            out.push(path);
        }
    }
}

/// The visible text of HTML: tags dropped (with `script`/`style` content;
/// block-level tags separate words, inline ones do not), character
/// references decoded.
fn html_text(html: &str) -> String {
    const INLINE: &[&str] = &[
        "a", "abbr", "b", "bdi", "cite", "code", "del", "dfn", "em", "i", "img", "input", "ins",
        "kbd", "mark", "q", "s", "samp", "small", "span", "strong", "sub", "sup", "time", "u",
        "var", "wbr",
    ];
    // References decode within each text chunk, like a browser (`&gt</a>;`
    // is `>` then `;`).
    let mut text = String::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        text.push_str(&decode(&rest[..open]));
        let tag = &rest[open..];
        let end = tag_end(tag);
        let name: String = tag[1..end]
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let mut after = &tag[end..];
        if name == "script" || name == "style" {
            if let Some(close) = after.find(&format!("</{name}")) {
                after = &after[close..];
                after = &after[tag_end(after)..];
            }
        }
        if !INLINE.contains(&name.as_str()) {
            text.push(' ');
        }
        rest = after;
    }
    text.push_str(&decode(rest));
    text
}

/// The index after the `>` closing the tag at the start of `s`, outside
/// quoted attribute values.
fn tag_end(s: &str) -> usize {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if q == c => quote = None,
            (None, '>') => return i + 1,
            _ => {}
        }
    }
    s.len()
}

fn decode(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        // A browser reads `&amp`, `&lt`, `&gt`, `&quot` without `;` too.
        if let Some((len, ch)) = [("&amp", '&'), ("&lt", '<'), ("&gt", '>'), ("&quot", '"')]
            .iter()
            .find(|(name, _)| tail == *name)
            .map(|(name, ch)| (name.len(), *ch))
        {
            out.push(ch);
            rest = &tail[len..];
            continue;
        }
        let end = tail.find(';').filter(|e| *e <= 10);
        let decoded = end.and_then(|end| {
            let name = &tail[1..end];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                "copy" => Some('©'),
                "reg" => Some('®'),
                "trade" => Some('™'),
                "mdash" => Some('\u{2014}'),
                "ndash" => Some('\u{2013}'),
                "hellip" => Some('\u{2026}'),
                "lsquo" => Some('\u{2018}'),
                "rsquo" => Some('\u{2019}'),
                "ldquo" => Some('\u{201c}'),
                "rdquo" => Some('\u{201d}'),
                "laquo" => Some('«'),
                "raquo" => Some('»'),
                "rarr" => Some('\u{2192}'),
                "larr" => Some('\u{2190}'),
                "rArr" => Some('\u{21d2}'),
                "lArr" => Some('\u{21d0}'),
                _ => name
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            }?;
            Some((ch, end + 1))
        });
        match decoded {
            Some((ch, len)) => {
                out.push(ch);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn words(text: &str) -> BTreeMap<String, i64> {
    let mut counts = BTreeMap::new();
    for word in text
        .split(|c: char| c.is_whitespace())
        .filter(|w| !w.is_empty())
    {
        *counts.entry(word.to_string()).or_insert(0) += 1;
    }
    counts
}

/// `missing: …` (HTML only) and `extra: …` (DOCX only), or `None` when the
/// words agree.
fn difference(html: &str, docx: &str) -> Option<String> {
    let html = words(html);
    let docx = words(docx);
    let side = |a: &BTreeMap<String, i64>, b: &BTreeMap<String, i64>| {
        let mut out = Vec::new();
        for (word, n) in a {
            for _ in 0..(n - b.get(word).copied().unwrap_or(0)).max(0) {
                out.push(word.clone());
            }
        }
        out.join(" ")
    };
    let missing = side(&html, &docx);
    let extra = side(&docx, &html);
    (!missing.is_empty() || !extra.is_empty())
        .then(|| format!("missing: {missing}\textra: {extra}"))
}

#[test]
fn docx_text_matches_html_text() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../adoctra-html/tests/golden");
    let mut files = Vec::new();
    inputs(&root, &mut files);
    assert!(files.len() > 200, "golden corpus not found");
    let mut report = String::new();
    for path in &files {
        let source = std::fs::read_to_string(path).unwrap();
        let doc = adoctra::parse(&source).document;
        let html = adoctra::html::HtmlBackend::embedded()
            .convert(&doc)
            .unwrap();
        let docx = adoctra::docx::DocxBackend::new().convert(&doc).unwrap();
        let docx_text = adoctra::docx::extract_text(&docx).unwrap();
        let html_text = html_text(&String::from_utf8(html).unwrap());
        if let Some(diff) = difference(&html_text, &docx_text) {
            let name = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            report.push_str(&format!("{name}\t{diff}\n"));
        }
    }
    let baseline = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/docx_text.baseline");
    if std::env::var_os("ADOCTRA_DOCX_TEXT_REGENERATE").is_some() {
        std::fs::write(&baseline, &report).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&baseline).unwrap_or_default();
    if expected != report {
        let old: Vec<&str> = expected.lines().collect();
        let new: Vec<&str> = report.lines().collect();
        let gone: Vec<_> = old.iter().filter(|l| !new.contains(l)).collect();
        let added: Vec<_> = new.iter().filter(|l| !old.contains(l)).collect();
        panic!("DOCX/HTML text differences drifted.\nno longer:\n{gone:#?}\nnew:\n{added:#?}");
    }
}

#[test]
fn words_compare_as_multisets() {
    assert_eq!(difference("a b b", "b a b"), None);
    assert_eq!(
        difference("a b c", "a c d d"),
        Some("missing: b\textra: d d".to_string())
    );
    assert_eq!(
        html_text("<p>a&amp;<em>b</em></p><style>x{}</style>c"),
        " a&b  c"
    );
    assert_eq!(html_text(r#"x<img alt="a>b">y"#), "xy");
}
