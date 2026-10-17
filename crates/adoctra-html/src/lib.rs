//! HTML5 backend.

use adoctra_core::ast::{Attribution, Block, BlockKind, Inline};
use adoctra_core::{Backend, Document, Result};

/// adoctra's reference stylesheet — a modern, self-contained theme (custom
/// properties, `color-mix()`, SVG-mask icons). [`HtmlBackend::convert`] inlines
/// it into the document `<head>` so output is styled out of the box; consumers
/// that want their own styling can ignore it and read the semantic markup.
pub const STYLESHEET: &str = include_str!("../assets/adoctra.css");

#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlBackend;

impl HtmlBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Backend for HtmlBackend {
    fn name(&self) -> &'static str {
        "html"
    }

    fn extension(&self) -> &'static str {
        "html"
    }

    fn convert(&self, doc: &Document) -> Result<Vec<u8>> {
        let mut out = String::new();
        out.push_str("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<style>\n");
        out.push_str(STYLESHEET);
        out.push_str("</style>\n</head>\n<body>\n");
        for (i, block) in doc.blocks.iter().enumerate() {
            match &block.kind {
                BlockKind::Heading { level, inlines } if i == 0 && doc.has_title => {
                    render_heading(block, *level, inlines, false, &mut out);
                }
                _ => render_block(block, &mut out),
            }
        }
        out.push_str("</body>\n</html>\n");
        Ok(out.into_bytes())
    }
}

fn render_block(block: &Block, out: &mut String) {
    // A block's anchor (an explicit `[[id]]`/`[#id]` or a section's auto-generated
    // id) renders as an `id` attribute on the block's element; `None` → none.
    // `[.role]` shorthand roles render as a `class` (appended to any base class
    // such as a listing's); asciidoctor puts both on a wrapping `<div>`, but
    // adoctra emits no wrapper, so they go on the inner element (as the id
    // already does).
    let id = id_attr(&block.id);
    match &block.kind {
        BlockKind::Heading { level, inlines } => render_heading(block, *level, inlines, true, out),
        BlockKind::Paragraph { inlines } => {
            let class = class_attr(&[], &block.roles);
            render_title(&block.title, &block.caption, out);
            out.push_str(&format!("<p{id}{class}>"));
            render_inlines(inlines, out);
            out.push_str("</p>\n");
        }
        BlockKind::Listing { text } => {
            render_title(&block.title, &block.caption, out);
            // A `[source]`-style listing renders as asciidoctor's highlight form:
            // `<pre class="highlight"><code class="language-LANG" data-lang="LANG">`,
            // with the language taken from the `language` attribute that core
            // folded out of `[source,LANG]`. Without `source` it stays a plain
            // listing. asciidoctor hangs roles/id on a wrapping `<div>`; adoctra
            // has no wrapper, so they go on the `<pre>` (roles appended to the base
            // class), as elsewhere.
            if block.attrs.style.as_deref() == Some("source") {
                let class = class_attr(&["highlight"], &block.roles);
                out.push_str(&format!("<pre{class}{id}>"));
                match block.attrs.named.get("language") {
                    Some(lang) => {
                        let lang = escape(lang);
                        out.push_str(&format!(
                            "<code class=\"language-{lang}\" data-lang=\"{lang}\">"
                        ));
                    }
                    None => out.push_str("<code>"),
                }
                out.push_str(&escape(text));
                out.push_str("</code></pre>\n");
            } else {
                let class = class_attr(&["listing"], &block.roles);
                out.push_str(&format!("<pre{class}{id}>"));
                out.push_str(&escape(text));
                out.push_str("</pre>\n");
            }
        }
        BlockKind::Literal { text } => {
            // A literal block: verbatim `<pre>`, like a plain listing but with the
            // `literalblock` base class. asciidoctor wraps it in a `<div>`; adoctra
            // hangs the class/id/roles on the `<pre>` itself, as for a listing.
            render_title(&block.title, &block.caption, out);
            let class = class_attr(&["literalblock"], &block.roles);
            out.push_str(&format!("<pre{class}{id}>"));
            out.push_str(&escape(text));
            out.push_str("</pre>\n");
        }
        BlockKind::Passthrough { text } => {
            // Passthrough: the body is emitted *completely raw* — no escaping and
            // no wrapper element at all (asciidoctor's `:pass` context). Unlike
            // every other block, the id/roles are dropped: there is no element to
            // hang them on, matching asciidoctor. Its title is not rendered.
            out.push_str(text);
            out.push('\n');
        }
        BlockKind::List { ordered, items } => {
            let tag = if *ordered { "ol" } else { "ul" };
            let class = class_attr(&[], &block.roles);
            render_title(&block.title, &block.caption, out);
            out.push_str(&format!("<{tag}{id}{class}>\n"));
            for item in items {
                out.push_str("<li>");
                render_inlines(&item.inlines, out);
                // Nested blocks (a sublist, or a `+`-attached paragraph/listing)
                // render inside the <li>, after the item's principal text. Each
                // starts on its own line, matching asciidoctor's structure and
                // keeping the principal text from running into the block's text.
                for child in &item.blocks {
                    out.push('\n');
                    render_block(child, out);
                }
                out.push_str("</li>\n");
            }
            out.push_str(&format!("</{tag}>\n"));
        }
        BlockKind::Admonition {
            kind,
            label,
            blocks,
        } => {
            // A modern, semantic admonition: a single `<aside>` carrying the type
            // (`data-type`) and an accessible label, then a title and the rendered
            // child blocks. No table/icon-font scaffolding (asciidoctor's) — the
            // styling is the reference stylesheet's job. As elsewhere there is no
            // wrapping `<div>`: id/roles hang on the `<aside>` itself (roles append
            // to `admonition`). The paragraph/label forms carry a single child
            // paragraph, so their body renders as `<p>…</p>` exactly as before.
            // The label is core's resolved `caption` / `<kind>-caption` value; an
            // unset caption leaves an empty title, as in asciidoctor.
            let class = class_attr(&["admonition"], &block.roles);
            let ty = kind.as_str();
            let mut label_html = String::new();
            render_parts(label, &mut label_html);
            let text = adoctra_core::attributes::parts_text(label);
            let aria = if text.is_empty() {
                String::new()
            } else {
                format!(" aria-label=\"{}\"", escape(&text).replace('"', "&quot;"))
            };
            out.push_str(&format!("<aside{id}{class} data-type=\"{ty}\"{aria}>\n"));
            out.push_str(&format!("<p class=\"admonition-title\">{label_html}</p>\n"));
            // A `.Title` on the admonition is the block's *own* title (distinct from
            // the `Note`/`Tip`/… label), rendered above the body — matching where
            // asciidoctor places it in the content cell.
            render_title(&block.title, &block.caption, out);
            for child in blocks {
                render_block(child, out);
            }
            out.push_str("</aside>\n");
        }
        BlockKind::Example { blocks } => {
            // A parsed container: one `<div class="exampleblock">` wrapping the
            // rendered children. asciidoctor nests `<div class="content">` inside;
            // adoctra keeps the no-wrapper convention — a single element carries
            // the class plus the block's id/roles, children render straight in.
            let class = class_attr(&["exampleblock"], &block.roles);
            out.push_str(&format!("<div{class}{id}>\n"));
            render_title(&block.title, &block.caption, out);
            for child in blocks {
                render_block(child, out);
            }
            out.push_str("</div>\n");
        }
        BlockKind::Sidebar { blocks } => {
            // A parsed container rendered as the semantic HTML5 `<aside>` — a sidebar
            // is tangential content — carrying `sidebarblock` plus the block's
            // id/roles. asciidoctor nests `<div class="sidebarblock"><div
            // class="content">`; adoctra keeps the no-wrapper convention (one
            // element), and the `.Title` renders just inside it, as for an example.
            let class = class_attr(&["sidebarblock"], &block.roles);
            out.push_str(&format!("<aside{class}{id}>\n"));
            render_title(&block.title, &block.caption, out);
            for child in blocks {
                render_block(child, out);
            }
            out.push_str("</aside>\n");
        }
        BlockKind::Quote {
            attribution,
            blocks,
        } => {
            // A modern, semantic quote: a `<figure class="quoteblock">` wrapping a
            // `<blockquote>` of the rendered children and an optional
            // `<figcaption>` attribution. asciidoctor nests
            // `quoteblock`/`blockquote`/`attribution` `<div>`s; adoctra keeps the
            // no-wrapper convention and the HTML5 figure/blockquote/figcaption/cite
            // idiom — id/roles hang on the `<figure>`.
            let class = class_attr(&["quoteblock"], &block.roles);
            out.push_str(&format!("<figure{class}{id}>\n"));
            render_title(&block.title, &block.caption, out);
            out.push_str("<blockquote>\n");
            for child in blocks {
                render_block(child, out);
            }
            out.push_str("</blockquote>\n");
            render_attribution(attribution, out);
            out.push_str("</figure>\n");
        }
        BlockKind::Verse {
            attribution,
            inlines,
        } => {
            // A verse: like a quote, but the body is verbatim — rendered in a
            // `<pre>` so the source line breaks survive — while inline
            // substitutions still apply (the scanned `inlines` render their
            // markup). No newline after `<pre>` (it is whitespace-significant).
            let class = class_attr(&["verseblock"], &block.roles);
            out.push_str(&format!("<figure{class}{id}>\n"));
            render_title(&block.title, &block.caption, out);
            out.push_str("<pre class=\"verse\">");
            render_inlines(inlines, out);
            out.push_str("</pre>\n");
            render_attribution(attribution, out);
            out.push_str("</figure>\n");
        }
        BlockKind::Open { blocks } => {
            // A parsed container with no semantic styling of its own (a `--` block,
            // or `[open]` on a paragraph). asciidoctor nests
            // `<div class="openblock"><div class="content">`; adoctra keeps the
            // no-wrapper convention — a single `<div class="openblock">` carries the
            // id/roles, with the `.Title` and children rendered straight inside. A
            // *container* style on a `--` block masquerades it into another kind in
            // the parser, so this arm only ever sees a genuine open block.
            let class = class_attr(&["openblock"], &block.roles);
            out.push_str(&format!("<div{class}{id}>\n"));
            render_title(&block.title, &block.caption, out);
            for child in blocks {
                render_block(child, out);
            }
            out.push_str("</div>\n");
        }
    }
}

/// A heading element. A discrete (`[discrete]`/`[float]`) heading carries its
/// style as a class, like asciidoctor's floating title; a level-0 heading that
/// is not the document title is a part (`sect0`).
fn render_heading(block: &Block, level: u8, inlines: &[Inline], body: bool, out: &mut String) {
    let h = level.saturating_add(1).min(6);
    let base = match block.attrs.style.as_deref() {
        Some(style @ ("discrete" | "float")) => Some(style),
        _ if body && level == 0 => Some("sect0"),
        _ => None,
    };
    let class = class_attr(base.as_slice(), &block.roles);
    render_title(&block.title, &block.caption, out);
    out.push_str(&format!("<h{h}{}{class}>", id_attr(&block.id)));
    render_inlines(inlines, out);
    out.push_str(&format!("</h{h}>\n"));
}

/// Emit a quote/verse [`Attribution`] as a `<figcaption>` (nothing when absent).
/// Mirrors asciidoctor's content — an em dash before the author, the cited work in
/// a `<cite>` — but as a semantic `<figcaption>` rather than a `<div
/// class="attribution">`. The em dash is emitted only with an author, so a
/// citetitle-only attribution renders just the `<cite>` (matching asciidoctor).
fn render_attribution(attribution: &Option<Attribution>, out: &mut String) {
    let Some(attr) = attribution else {
        return;
    };
    out.push_str("<figcaption>");
    if let Some(author) = &attr.author {
        out.push_str("&#8212; ");
        render_inlines(author, out);
        if attr.citetitle.is_some() {
            out.push_str("<br>\n");
        }
    }
    if let Some(cite) = &attr.citetitle {
        out.push_str("<cite>");
        render_inlines(cite, out);
        out.push_str("</cite>");
    }
    out.push_str("</figcaption>\n");
}

/// Emit a block's `.Title` as `<div class="title">…</div>` when present (nothing
/// when absent). asciidoctor wraps a titled block in a `<div>` and puts the title
/// inside it as `<div class="title">`; adoctra has no wrapper, so for most blocks
/// the title is a sibling rendered immediately before the block's element, and for
/// the container blocks (example/admonition) it is emitted just inside their
/// element. The title text is parsed inlines, so its markup renders.
///
/// A resolved `caption` (e.g. `"Example 1. "` on a numbered example block) is
/// prepended verbatim inside the same `<div>`, exactly like asciidoctor's
/// `#{caption}#{title}`. It is `None` for every block but a numbered example, so
/// titles elsewhere are unaffected.
fn render_title(title: &Option<Vec<Inline>>, caption: &Option<String>, out: &mut String) {
    if let Some(inlines) = title {
        out.push_str("<div class=\"title\">");
        if let Some(caption) = caption {
            out.push_str(&escape(caption));
        }
        render_inlines(inlines, out);
        out.push_str("</div>\n");
    }
}

fn render_inlines(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text { value, .. } => out.push_str(&escape_text(value)),
            Inline::Raw { value, .. } => out.push_str(value),
            Inline::Strong { children, .. } => {
                out.push_str("<strong>");
                render_inlines(children, out);
                out.push_str("</strong>");
            }
            Inline::Emphasis { children, .. } => {
                out.push_str("<em>");
                render_inlines(children, out);
                out.push_str("</em>");
            }
            Inline::Monospace { children, .. } => {
                out.push_str("<code>");
                render_inlines(children, out);
                out.push_str("</code>");
            }
            Inline::Mark { children, .. } => {
                out.push_str("<mark>");
                render_inlines(children, out);
                out.push_str("</mark>");
            }
            Inline::Superscript { children, .. } => {
                out.push_str("<sup>");
                render_inlines(children, out);
                out.push_str("</sup>");
            }
            Inline::Subscript { children, .. } => {
                out.push_str("<sub>");
                render_inlines(children, out);
                out.push_str("</sub>");
            }
            // A bare link (no explicit text) gets `class="bare"`, matching
            // asciidoctor; the href and display text are otherwise independent.
            Inline::Link {
                target,
                children,
                bare,
                ..
            } => {
                out.push_str("<a href=\"");
                out.push_str(&escape(target));
                if *bare {
                    out.push_str("\" class=\"bare");
                }
                out.push_str("\">");
                render_inlines(children, out);
                out.push_str("</a>");
            }
            // A cross-reference renders as an internal link `href="#id"`. The
            // display text (explicit reftext, or the core-generated `[id]`) is in
            // `children`; no `class="bare"` (that is a link-only concern).
            Inline::CrossReference {
                target, children, ..
            } => {
                out.push_str("<a href=\"#");
                out.push_str(&escape(target));
                out.push_str("\">");
                render_inlines(children, out);
                out.push_str("</a>");
            }
            // An inline anchor renders as an empty `<a id="…">` at this point, like
            // asciidoctor. The reftext (if any) is *not* shown here — it only feeds
            // a textless cross-reference's display text during resolution.
            Inline::Anchor { id, .. } => {
                out.push_str("<a id=\"");
                out.push_str(&escape(id));
                out.push_str("\"></a>");
            }
            // Resolved in core: emit the value, or fall back to the literal
            // `{name}` for a missing attribute (asciidoctor's default).
            Inline::AttributeRef { value, parts, .. } if !parts.is_empty() && value.is_some() => {
                render_parts(parts, out);
            }
            Inline::Substituted { parts, .. } => render_parts(parts, out),
            Inline::AttributeRef { name, value, .. } => match value {
                Some(v) => out.push_str(&escape_text(v)),
                None => out.push_str(&escape(&format!("{{{name}}}"))),
            },
        }
    }
}

fn attribute_format_tag(format: adoctra_core::AttributeFormat) -> &'static str {
    use adoctra_core::AttributeFormat::*;
    match format {
        Strong => "strong",
        Emphasis => "em",
        Monospace => "code",
        Mark => "mark",
        Superscript => "sup",
        Subscript => "sub",
        Span => "span",
    }
}

/// An ` id="…"` attribute (with a leading space) for a block that carries an
/// anchor, or the empty string. The id is escaped as an HTML attribute value.
fn id_attr(id: &Option<String>) -> String {
    match id {
        Some(id) => format!(" id=\"{}\"", escape(id)),
        None => String::new(),
    }
}

/// A ` class="…"` attribute (with a leading space) joining a block's static
/// `base` classes (e.g. `listing`) with its `[.role]` shorthand roles, or the
/// empty string when there are none. Classes are space-separated in order and
/// escaped as an attribute value.
fn class_attr(base: &[&str], roles: &[String]) -> String {
    if base.is_empty() && roles.is_empty() {
        return String::new();
    }
    let classes = base
        .iter()
        .copied()
        .chain(roles.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    format!(" class=\"{}\"", escape(&classes))
}

/// Output-encoding for HTML. Deliberately a *backend* concern: the shared model
/// keeps text literal, and each backend escapes for its own format. Used for
/// verbatim text (listing blocks) and link targets, where typographic
/// characters are not remapped.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Output-encoding for resolved *inline text*. Beyond the usual `&<>` escaping,
/// the typographic characters core produces for replacements (`(C)` -> `©`,
/// `--` -> `—`, `...` -> `…`, the arrows, …) are emitted as numeric character
/// references, matching asciidoctor's HTML byte-for-byte. Choosing entities
/// over the literal UTF-8 char is an HTML-encoding decision, so it lives here in
/// the backend rather than in the format-neutral core.
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{a9}' => out.push_str("&#169;"),
            '\u{ae}' => out.push_str("&#174;"),
            '\u{2122}' => out.push_str("&#8482;"),
            '\u{2009}' => out.push_str("&#8201;"),
            '\u{2014}' => out.push_str("&#8212;"),
            '\u{200b}' => out.push_str("&#8203;"),
            '\u{2026}' => out.push_str("&#8230;"),
            '\u{2018}' => out.push_str("&#8216;"),
            '\u{2019}' => out.push_str("&#8217;"),
            '\u{201c}' => out.push_str("&#8220;"),
            '\u{201d}' => out.push_str("&#8221;"),
            '\u{2192}' => out.push_str("&#8594;"),
            '\u{21d2}' => out.push_str("&#8658;"),
            '\u{2190}' => out.push_str("&#8592;"),
            '\u{21d0}' => out.push_str("&#8656;"),
            _ => out.push(ch),
        }
    }
    out
}

fn render_parts(parts: &[adoctra_core::AttributePart], out: &mut String) {
    for part in parts {
        match part {
            adoctra_core::AttributePart::Text(text) => out.push_str(&escape_text(text)),
            adoctra_core::AttributePart::Raw(text) => out.push_str(text),
            adoctra_core::AttributePart::OpenLink(link) => {
                out.push_str("<a href=\"");
                out.push_str(&link.target);
                out.push('"');
                for (key, value) in [
                    ("id", &link.id),
                    ("class", &link.role),
                    ("title", &link.title),
                    ("target", &link.window),
                    ("rel", &link.rel),
                ] {
                    if let Some(value) = value {
                        out.push(' ');
                        out.push_str(key);
                        out.push_str("=\"");
                        out.push_str(value);
                        out.push('"');
                    }
                }
                out.push('>');
            }
            adoctra_core::AttributePart::CloseLink => out.push_str("</a>"),
            adoctra_core::AttributePart::LineBreak => out.push_str("<br>"),
            adoctra_core::AttributePart::Open(format) => {
                out.push('<');
                out.push_str(attribute_format_tag(*format));
                out.push('>');
            }
            adoctra_core::AttributePart::OpenWithAttributes { format, id, role } => {
                out.push('<');
                out.push_str(attribute_format_tag(*format));
                // Quote substitution already retains lexical protection
                // in these values; escaping again would double entities.
                if let Some(id) = id {
                    out.push_str(" id=\"");
                    out.push_str(id);
                    out.push('"');
                }
                if let Some(role) = role {
                    out.push_str(" class=\"");
                    out.push_str(role);
                    out.push('"');
                }
                out.push('>');
            }
            adoctra_core::AttributePart::Close(format) => {
                out.push_str("</");
                out.push_str(attribute_format_tag(*format));
                out.push('>');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(src: &str) -> String {
        let doc = adoctra_core::parse(src).document;
        String::from_utf8(HtmlBackend.convert(&doc).unwrap()).unwrap()
    }

    #[test]
    fn block_reftext_preserves_early_values_and_uses_only_cqr() {
        let html = render("[[here,*{counter:n}* \\{n} link:p[{counter:n}] <x>]]\nBody {counter:n}\n\n<<here>> / <<here>>");
        assert!(html.contains("Body 3</p>"), "{html}");
        assert_eq!(
            html.matches(r##"<a href="#here"><strong>1</strong> {n} link:p[2] &lt;x&gt;</a>"##)
                .count(),
            2,
            "{html}"
        );
        let empty = render(":attribute-missing: drop-line\n\n[[here,{missing}]]\nBody\n\n<<here>>");
        assert!(empty.contains(r##"<a href="#here">[here]</a>"##), "{empty}");
        let header = render("[[here,<b>{counter:n}</b>]]\n= H\n\n{reftext}\n\n<<here>>");
        assert!(header.contains("<p><b>1</b></p>"), "{header}");
        assert!(
            header.contains(r##"<a href="#here">&lt;b&gt;1&lt;/b&gt;</a>"##),
            "{header}"
        );
    }

    #[test]
    fn xref_labels_use_titles_and_reftext_formatted_per_reference() {
        let html = render("See <<t>>.\n\n[[t]]\n.Ж *T* https://x.org[site]\n----\nx\n----\n\n[[b,[.r\\{x}]#R#]]\npara\n\n:x: 1\n\n<<b>>\n\n:x: 2\n\n<<b>> <<none>>");
        // A borrowed title keeps formatting but drops link tags (DropAnchorRx).
        assert!(
            html.contains(r##"<a href="#t">Ж <strong>T</strong> site</a>"##),
            "{html}"
        );
        assert!(
            html.contains(r##"<a href="#b"><span class="r1">R</span></a>"##),
            "{html}"
        );
        assert!(
            html.contains(r##"<a href="#b"><span class="r2">R</span></a>"##),
            "{html}"
        );
        assert!(html.contains(r##"<a href="#none">[none]</a>"##), "{html}");
    }

    #[test]
    fn stateful_block_titles_render_shared_counters_and_skip_passthrough_titles() {
        let html = render(".A {counter:example-number}\n====\nBody {counter:example-number}\n====\n\n[[here]]\n.B {counter:example-number}\n====\nBody {counter:example-number}\n====");
        assert!(html.contains("Example 1. A 2</div>"), "{html}");
        assert!(html.contains("Example 2. B 3</div>"), "{html}");
        assert_eq!(html.matches("<p>Body 3</p>").count(), 2, "{html}");
        for anchor in ["", "[[here]]\n"] {
            let html = render(&format!(
                "{anchor}.Hidden {{set:x:yes}}\n[pass]\n<b>Raw</b>\n\n== S {{x}}\n\n{{x}}"
            ));
            assert!(!html.contains("Hidden"), "{html}");
            assert!(html.contains("<b>Raw</b>"), "{html}");
            assert!(html.contains("<p>{x}</p>"), "{html}");
            assert!(
                html.contains(if anchor.is_empty() {
                    ">S {x}</h2>"
                } else {
                    ">S yes</h2>"
                }),
                "{html}"
            );
        }
    }

    #[test]
    fn stateful_headings_render_cached_sections_and_conversion_time_doctitle() {
        let html = render("= H {counter:n}\n\n== S {counter:n}\n\n<<_s_2>> {counter:n}\n\n== More {counter:n}\n\n{counter:n}");
        assert!(html.contains("<h1>H 2</h1>"), "{html}");
        assert!(html.contains(r#"<h2 id="_s_2">S 2</h2>"#), "{html}");
        assert!(html.contains(r##"<a href="#_s_2">S 2</a> 3"##), "{html}");
        assert!(html.contains(r#"<h2 id="_more_3">More 3</h2>"#), "{html}");
        assert!(html.contains("<p>4</p>"), "{html}");
    }

    #[test]
    fn stateful_body_renders_counters_late_passes_and_replayed_declarations() {
        let html = render(":n: 1\n\n{counter:n} pass:a[{counter:n}] {n}\n\n:n: 1\n\n{counter:n} {set:x:<b>X</b>}{x}\n\nlink:pass:a[{counter:n}][]");
        assert!(html.contains("<p>2 3 2</p>"), "{html}");
        assert!(html.contains("<p>2 &lt;b&gt;X&lt;/b&gt;</p>"), "{html}");
        assert!(html.contains(r#"<a href="3" class="bare">4</a>"#), "{html}");
    }

    #[test]
    fn stateful_definitions_render_neutral_parts_and_body_replay() {
        let html = render(":v: pass:q,a[[.{counter:n}]*{counter:n}*]\n:l: pass:n[link:pass:a[{counter:n}][]]\n\n{v} {l} {n}\n\n:w: {counter:n}\n\n{w} {n}");
        assert!(html.contains(r#"<strong class="1">2</strong>"#), "{html}");
        assert!(
            html.contains(r#"<a href="3" class="bare">4</a> 4"#),
            "{html}"
        );
        assert!(html.contains("<p>5 4</p>"), "{html}");
    }

    #[test]
    fn repeated_link_normal_renders_protected_text_raw_parts_and_breaks() {
        let html = render(":v: pass:[+*bold*+]\n:r: pass:[+++<b>raw</b>+++]\n\nlink:https://e['{v}',role=x] link:https://e['{r}',role=x] link:https://e['end +',role=x]");
        assert!(html.contains(r#"class="x">*bold*</a>"#), "{html}");
        assert!(html.contains(r#"class="x"><b>raw</b></a>"#), "{html}");
        assert!(html.contains(r#"class="x">end<br></a>"#), "{html}");
    }

    #[test]
    fn metadata_missing_policies_preserve_enclosing_formatting_and_link_exceptions() {
        let html = render(":attribute-missing: drop-line\n\nBefore [.red{missing}]*Text* link:https://e['\\{missing}',role=x] link:https://e[Label,title='\\{missing}'] after");
        assert!(html.contains("Before <strong>Text</strong>"), "{html}");
        assert!(html.contains("class=\"bare x\">https://e</a>"), "{html}");
        assert!(
            html.contains("title=\"{missing}\">Label</a> after"),
            "{html}"
        );
    }

    #[test]
    fn ordinary_post_replacements_render_attribute_breaks_and_local_options() {
        let html = render(":v: one + \\\n two\n\n{v}\n\n[%hardbreaks]\nЖ\nБ +\n\nthree\nfour");
        assert!(html.contains("<p>one<br> two</p>"), "{html}");
        assert!(html.contains("<p>Ж<br> Б +</p>"), "{html}");
        assert!(html.contains("<p>three four</p>"), "{html}");
        let html = render("[verse%hardbreaks]\n____\nЖ\nБ +\n____");
        assert!(html.contains("Ж<br>\nБ +"), "{html}");
    }

    #[test]
    fn shared_attribute_stage_renders_ordered_protection_and_definition_escapes() {
        let html = render(":x: Value\n:v: {cpp} \\{x} {x\\}\n:protected: pass:a,c[{cpp}]\n:raw: pass:c,a[{cpp}]\n\n{v} / {protected} / {raw} / pass:a,c[{cpp}]");
        assert!(
            html.contains("C&#43;&#43; {x} {x} / C&#43;&#43; / C&#43;&#43; / C&amp;#43;&amp;#43;"),
            "{html}"
        );
        let html = render(":attribute-missing: drop-line\n:v: before {missing} after\n\nFirst {v} pass:a[before {missing} after] last");
        assert!(html.contains("<p>First   last</p>"), "{html}");
    }

    #[test]
    fn ordinary_attributes_render_escapes_intrinsic_entities_and_missing_policies() {
        let html = render(r"{cpp} \{cpp} {missing} {amp}copy; {asterisk}literal{asterisk}");
        assert!(
            html.contains("C&#43;&#43; {cpp} {missing} &copy; *literal*"),
            "{html}"
        );
        let html = render(":attribute-missing: drop-line\n\nfirst\ndiscard {missing}\nlast");
        assert!(html.contains("<p>first last</p>"), "{html}");
        assert!(!html.contains("discard"), "{html}");
    }

    #[test]
    fn renders_heading_and_paragraph() {
        let html = render("= Title\n\nHi *there*");
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<strong>there</strong>"));
    }

    #[test]
    fn resolves_attribute_reference_to_its_value() {
        let html = render(":product: Adoctra\n\nUse {product}.");
        assert!(html.contains("<p>Use Adoctra.</p>"), "got: {html}");
    }

    #[test]
    fn missing_attribute_falls_back_to_literal() {
        let html = render("Use {nope} here.");
        assert!(html.contains("Use {nope} here."), "got: {html}");
    }

    #[test]
    fn renders_monospace_mark_super_and_subscript() {
        let html = render("A `code`, #hot#, E=mc^2^, H~2~O.");
        assert!(html.contains("<code>code</code>"), "got: {html}");
        assert!(html.contains("<mark>hot</mark>"), "got: {html}");
        assert!(html.contains("mc<sup>2</sup>"), "got: {html}");
        assert!(html.contains("H<sub>2</sub>O"), "got: {html}");
    }

    #[test]
    fn monospace_renders_nested_markup() {
        let html = render("Run `*bold* code`.");
        assert!(
            html.contains("<code><strong>bold</strong> code</code>"),
            "got: {html}"
        );
    }

    #[test]
    fn renders_link_macro_with_text() {
        let html = render("See link:https://example.org[the site].");
        assert!(
            html.contains("<a href=\"https://example.org\">the site</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn bare_autolink_gets_the_bare_class() {
        let html = render("Bare https://example.org now.");
        assert!(
            html.contains("<a href=\"https://example.org\" class=\"bare\">https://example.org</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn link_text_keeps_inline_markup() {
        let html = render("link:https://h[*bold* text]");
        assert!(
            html.contains("<a href=\"https://h\"><strong>bold</strong> text</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn mailto_macro_renders_scheme_in_href() {
        let html = render("Mail mailto:me@x.io[me].");
        assert!(
            html.contains("<a href=\"mailto:me@x.io\">me</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn xref_with_text_renders_internal_link() {
        let html = render("See <<sec,Section Two>>.");
        assert!(
            html.contains("<a href=\"#sec\">Section Two</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn textless_xref_to_unknown_id_renders_bracketed_id() {
        // An *unresolved* textless `<<id>>` falls back to `[id]` (still inside
        // the anchor), matching asciidoctor.
        let html = render("See <<sec>>.");
        assert!(html.contains("<a href=\"#sec\">[sec]</a>"), "got: {html}");
    }

    #[test]
    fn xref_macro_renders_internal_link() {
        let html = render("See xref:sec[Mac].");
        assert!(html.contains("<a href=\"#sec\">Mac</a>"), "got: {html}");
    }

    #[test]
    fn inline_anchor_renders_empty_a_with_id() {
        // An inline `[[id]]` renders as an empty `<a id>` in place; the reftext is
        // not shown here (it only feeds a textless xref). Matches asciidoctor.
        let html = render("See the [[point,the Important Part]]here.");
        assert!(
            html.contains("See the <a id=\"point\"></a>here."),
            "got: {html}"
        );
    }

    #[test]
    fn anchor_macro_renders_empty_a_with_id() {
        let html = render("See anchor:point[] the part.");
        assert!(
            html.contains("See <a id=\"point\"></a> the part."),
            "got: {html}"
        );
    }

    #[test]
    fn xref_resolves_to_an_inline_anchor() {
        // `<<point>>` to an inline `[[point,…]]` renders the anchor's reftext.
        let html = render("Go to <<point>>.\n\nSee the [[point,the Important Part]]here.");
        assert!(
            html.contains("<a href=\"#point\">the Important Part</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn section_heading_renders_with_auto_id() {
        let html = render("== Hello World\n\nbody");
        assert!(
            html.contains("<h2 id=\"_hello_world\">Hello World</h2>"),
            "got: {html}"
        );
    }

    #[test]
    fn doctitle_override_renders_once_and_resolves_forward_references() {
        let html = render("= Original\n:doctitle: See <<_target>>\n\n== Target");
        assert!(
            html.contains("<h1>See <a href=\"#_target\">Target</a></h1>"),
            "{html}"
        );
        assert_eq!(html.matches("<h1>").count(), 1);
        assert!(!html.contains("Original"));
        let html = render(":doctitle: *Only*\n\nBody");
        assert!(html.contains("<h1><strong>Only</strong></h1>"), "{html}");
    }

    #[test]
    fn doctitle_has_no_id() {
        // The level-0 document title is not a section, so it carries no id.
        let html = render("= Doc Title\n\nbody");
        assert!(html.contains("<h1>Doc Title</h1>"), "got: {html}");
    }

    #[test]
    fn resolved_xref_renders_section_title() {
        let html = render("== Section Two\n\nSee <<_section_two>>.");
        assert!(
            html.contains("<a href=\"#_section_two\">Section Two</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn text_is_html_escaped_by_the_backend() {
        // The model keeps text literal; escaping is the backend's job.
        let html = render("1 < 2 & 3");
        assert!(html.contains("1 &lt; 2 &amp; 3"), "got: {html}");
    }

    #[test]
    fn renders_unordered_list() {
        let html = render("* one\n* two");
        assert!(html.contains("<ul>"), "got: {html}");
        assert!(html.contains("<li>one</li>"), "got: {html}");
        assert!(html.contains("<li>two</li>"), "got: {html}");
        assert!(html.contains("</ul>"), "got: {html}");
    }

    #[test]
    fn dash_list_also_renders_ul() {
        let html = render("- a\n- b");
        assert!(html.contains("<ul>"), "got: {html}");
    }

    #[test]
    fn renders_ordered_list() {
        let html = render(". first\n. second");
        assert!(html.contains("<ol>"), "got: {html}");
        assert!(html.contains("<li>first</li>"), "got: {html}");
        assert!(html.contains("</ol>"), "got: {html}");
    }

    #[test]
    fn explicit_numbered_list_renders_as_ol() {
        // `1.`/`2.` reuse the ordered-list node, so they render like `.`.
        let html = render("1. first\n2. second");
        assert!(html.contains("<ol>"), "got: {html}");
        assert!(html.contains("<li>first</li>"), "got: {html}");
        assert!(html.contains("</ol>"), "got: {html}");
    }

    #[test]
    fn list_item_keeps_inline_markup() {
        let html = render("* a *b*");
        assert!(
            html.contains("<li>a <strong>b</strong></li>"),
            "got: {html}"
        );
    }

    #[test]
    fn nested_list_renders_inside_parent_li() {
        let html = render("* one\n** a\n** b\n* two");
        // Two <ul> levels; the nested one opens inside the first item.
        assert_eq!(html.matches("<ul>").count(), 2, "got: {html}");
        // A nested block opens on its own line, after the item's principal text.
        assert!(html.contains("<li>one\n<ul>"), "got: {html}");
        assert!(html.contains("<li>a</li>"), "got: {html}");
        assert!(html.contains("<li>b</li>"), "got: {html}");
        assert!(html.contains("<li>two</li>"), "got: {html}");
    }

    #[test]
    fn nested_ordered_under_unordered() {
        // A unique marker nests across kinds: `. b` becomes an <ol> in the <li>.
        let html = render("* a\n. b");
        assert!(html.contains("<ul>"), "got: {html}");
        assert!(html.contains("<ol>"), "got: {html}");
        assert!(html.contains("<li>a\n<ol>"), "got: {html}");
        assert!(html.contains("<li>b</li>"), "got: {html}");
    }

    #[test]
    fn continuation_paragraph_renders_inside_li() {
        // `+` attaches a paragraph block; it renders inside the <li>, after the
        // item's principal text (the same recursion that renders a sublist).
        let html = render("* one\n+\nattached\n* two");
        assert!(html.contains("<li>one\n<p>attached</p>"), "got: {html}");
        assert!(html.contains("<li>two</li>"), "got: {html}");
    }

    #[test]
    fn continuation_listing_renders_inside_li() {
        let html = render("* one\n+\n----\ncode\n----");
        assert!(
            html.contains("<li>one\n<pre class=\"listing\">code</pre>"),
            "got: {html}"
        );
    }

    #[test]
    fn multi_line_item_text_folds_into_one_li() {
        // A wrapped line folds into the item's text (one <li>, no extra item).
        let html = render("* one\n  two");
        assert_eq!(html.matches("<li>").count(), 1, "got: {html}");
        assert!(html.contains("two</li>"), "got: {html}");
    }

    #[test]
    fn typographic_replacements_render_as_numeric_entities() {
        // The HTML backend encodes the resolved typographic chars as numeric
        // character references, matching asciidoctor.
        let html = render("(C) (R) (TM)");
        assert!(html.contains("<p>&#169; &#174; &#8482;</p>"), "got: {html}");

        let html = render("A -- B and word--word");
        assert!(
            html.contains("<p>A&#8201;&#8212;&#8201;B and word&#8212;&#8203;word</p>"),
            "got: {html}"
        );

        let html = render("Wait... it's done");
        assert!(
            html.contains("<p>Wait&#8230;&#8203; it&#8217;s done</p>"),
            "got: {html}"
        );

        // Arrows, including the `<=>` -> `<` + `⇒` precedence (the `<` then
        // escapes to `&lt;`).
        let html = render("-> => <- <= and <=>");
        assert!(
            html.contains("<p>&#8594; &#8658; &#8592; &#8656; and &lt;&#8658;</p>"),
            "got: {html}"
        );
    }

    #[test]
    fn escaped_replacement_renders_literally() {
        let html = render("escaped \\(C) and \\-> here");
        assert!(
            html.contains("<p>escaped (C) and -&gt; here</p>"),
            "got: {html}"
        );
    }

    #[test]
    fn anchored_paragraph_renders_id_attribute() {
        let html = render("[[pa]]\nHello.");
        assert!(html.contains("<p id=\"pa\">Hello.</p>"), "got: {html}");
    }

    #[test]
    fn xref_to_anchor_reftext_renders_reftext() {
        let html = render("[[pa,The Para]]\nHi.\n\nSee <<pa>>.");
        assert!(html.contains("<a href=\"#pa\">The Para</a>"), "got: {html}");
    }

    #[test]
    fn explicit_anchor_on_section_overrides_auto_id() {
        // `[[custom]]` replaces the auto id `_real_title` on the <h2>.
        let html = render("[[custom]]\n== Real Title\n\nbody");
        assert!(
            html.contains("<h2 id=\"custom\">Real Title</h2>"),
            "got: {html}"
        );
    }

    #[test]
    fn shorthand_id_and_role_render_on_paragraph() {
        let html = render("[#intro.lead]\nHello.");
        assert!(
            html.contains("<p id=\"intro\" class=\"lead\">Hello.</p>"),
            "got: {html}"
        );
    }

    #[test]
    fn shorthand_role_only_renders_class_without_id() {
        let html = render("[.lead.big]\nHello.");
        assert!(
            html.contains("<p class=\"lead big\">Hello.</p>"),
            "got: {html}"
        );
    }

    #[test]
    fn shorthand_id_only_renders_id_without_class() {
        let html = render("[#intro]\nHello.");
        assert!(html.contains("<p id=\"intro\">Hello.</p>"), "got: {html}");
    }

    #[test]
    fn shorthand_role_appends_to_listing_base_class() {
        let html = render("[.numbered]\n----\nx = 1\n----");
        assert!(
            html.contains("<pre class=\"listing numbered\">"),
            "got: {html}"
        );
    }

    #[test]
    fn shorthand_role_on_section_renders_class_on_heading() {
        let html = render("[.notice]\n== Hello World\n\nbody");
        assert!(
            html.contains("<h2 id=\"_hello_world\" class=\"notice\">Hello World</h2>"),
            "got: {html}"
        );
    }

    #[test]
    fn shorthand_role_renders_on_list() {
        let html = render("[.compact]\n* one\n* two");
        assert!(html.contains("<ul class=\"compact\">"), "got: {html}");
    }

    #[test]
    fn source_listing_renders_highlight_code_with_language() {
        let html = render("[source,ruby]\n----\nputs 1\n----");
        assert!(
            html.contains(
                "<pre class=\"highlight\"><code class=\"language-ruby\" data-lang=\"ruby\">puts 1</code></pre>"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn source_listing_without_language_omits_code_class() {
        let html = render("[source]\n----\nputs 1\n----");
        assert!(
            html.contains("<pre class=\"highlight\"><code>puts 1</code></pre>"),
            "got: {html}"
        );
    }

    #[test]
    fn source_listing_role_appends_to_highlight_class() {
        let html = render("[source,ruby,role=numbered]\n----\nx\n----");
        assert!(
            html.contains("<pre class=\"highlight numbered\"><code class=\"language-ruby\""),
            "got: {html}"
        );
    }

    #[test]
    fn source_listing_id_renders_on_pre() {
        let html = render("[source#snip,ruby]\n----\nx\n----");
        assert!(
            html.contains("<pre class=\"highlight\" id=\"snip\"><code class=\"language-ruby\""),
            "got: {html}"
        );
    }

    #[test]
    fn listing_without_source_style_is_unchanged() {
        let html = render("----\nx = 1\n----");
        assert!(
            html.contains("<pre class=\"listing\">x = 1</pre>"),
            "got: {html}"
        );
    }

    #[test]
    fn delimited_literal_renders_literalblock_pre() {
        let html = render("....\nverbatim *not bold*\n....");
        assert!(
            html.contains("<pre class=\"literalblock\">verbatim *not bold*</pre>"),
            "got: {html}"
        );
    }

    #[test]
    fn literal_style_paragraph_renders_verbatim_and_escaped() {
        // `[literal]` paragraph: markup stays literal, HTML specials are escaped.
        let html = render("[literal]\na < b & *c*");
        assert!(
            html.contains("<pre class=\"literalblock\">a &lt; b &amp; *c*</pre>"),
            "got: {html}"
        );
    }

    #[test]
    fn literal_id_and_role_render_on_pre() {
        let html = render("[literal#lit.special]\nverbatim");
        assert!(
            html.contains("<pre class=\"literalblock special\" id=\"lit\">verbatim</pre>"),
            "got: {html}"
        );
    }

    #[test]
    fn delimited_passthrough_renders_raw_unescaped_unwrapped() {
        // `++++` emits its body completely raw: no escaping (`<`/`&` survive), no
        // inline subs (`*c*` stays literal) and no wrapper element (no `<pre>`).
        let html = render("++++\n<div>a < b & *c*</div>\n++++");
        assert!(html.contains("<div>a < b & *c*</div>"), "got: {html}");
        // No verbatim `<pre>` wrapper was emitted for the passthrough body.
        assert!(!html.contains("<pre"), "got: {html}");
    }

    #[test]
    fn pass_style_paragraph_renders_raw() {
        let html = render("[pass]\n<em>a</em> & b *c*");
        assert!(html.contains("<em>a</em> & b *c*"), "got: {html}");
    }

    #[test]
    fn passthrough_drops_id_and_roles() {
        // asciidoctor's `:pass` context has no element to hang id/roles on, so they
        // are dropped on output — unlike every other block.
        let html = render("[#myid.myrole]\n++++\n<p>raw</p>\n++++");
        assert!(html.contains("<p>raw</p>"), "got: {html}");
        assert!(!html.contains("myid"), "id leaked: {html}");
        assert!(!html.contains("myrole"), "role leaked: {html}");
    }

    #[test]
    fn note_style_renders_semantic_aside() {
        // `[NOTE]` → a semantic <aside>; inline markup in the body is processed.
        let html = render("[NOTE]\nThis is a *note*.");
        assert!(
            html.contains("<aside class=\"admonition\" data-type=\"note\" aria-label=\"Note\">"),
            "got: {html}"
        );
        assert!(
            html.contains("<p class=\"admonition-title\">Note</p>"),
            "got: {html}"
        );
        assert!(
            html.contains("<p>This is a <strong>note</strong>.</p>"),
            "got: {html}"
        );
    }

    #[test]
    fn label_form_renders_like_style_form() {
        // `NOTE: ` produces the same admonition; the label is not in the body.
        let html = render("NOTE: This is a *note*.");
        assert!(
            html.contains("<aside class=\"admonition\" data-type=\"note\" aria-label=\"Note\">"),
            "got: {html}"
        );
        assert!(
            html.contains("<p>This is a <strong>note</strong>.</p>"),
            "got: {html}"
        );
        assert!(!html.contains("NOTE:"), "label leaked into body: {html}");
    }

    #[test]
    fn all_admonition_types_render_with_type_and_label() {
        for (src, ty, label) in [
            ("[NOTE]\nx", "note", "Note"),
            ("[TIP]\nx", "tip", "Tip"),
            ("[IMPORTANT]\nx", "important", "Important"),
            ("[WARNING]\nx", "warning", "Warning"),
            ("[CAUTION]\nx", "caution", "Caution"),
        ] {
            let html = render(src);
            assert!(
                html.contains(&format!("data-type=\"{ty}\" aria-label=\"{label}\"")),
                "got: {html}"
            );
            assert!(
                html.contains(&format!("<p class=\"admonition-title\">{label}</p>")),
                "got: {html}"
            );
        }
    }

    #[test]
    fn admonition_id_and_role_render_on_aside() {
        let html = render("[NOTE#warn.box]\ntext");
        assert!(
            html.contains(
                "<aside id=\"warn\" class=\"admonition box\" data-type=\"note\" aria-label=\"Note\">"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn delimited_admonition_renders_block_body() {
        // `[NOTE]` on a `====` block: one semantic <aside> with the title, then the
        // rendered child blocks (two paragraphs here) — no exampleblock wrapper.
        let html = render("[NOTE]\n====\nFirst.\n\nSecond.\n====");
        assert!(
            html.contains("<aside class=\"admonition\" data-type=\"note\" aria-label=\"Note\">"),
            "got: {html}"
        );
        assert!(
            html.contains("<p class=\"admonition-title\">Note</p>"),
            "got: {html}"
        );
        assert!(html.contains("<p>First.</p>"), "got: {html}");
        assert!(html.contains("<p>Second.</p>"), "got: {html}");
        // The `[NOTE]` style replaced the example container — no exampleblock
        // wrapper survives (the stylesheet still mentions `.exampleblock`, so check
        // for the element, not the substring).
        assert!(!html.contains("<div class=\"exampleblock\""), "got: {html}");
    }

    #[test]
    fn delimited_admonition_id_and_roles_on_aside() {
        // As in the paragraph form, id/roles hang on the <aside> itself.
        let html = render("[NOTE#warn.box]\n====\ntext\n====");
        assert!(
            html.contains(
                "<aside id=\"warn\" class=\"admonition box\" data-type=\"note\" aria-label=\"Note\">"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn reference_stylesheet_is_embedded_in_head() {
        // The converter inlines the reference stylesheet so output is styled out
        // of the box.
        let html = render("Hello.");
        assert!(html.contains("<meta charset=\"utf-8\">"), "got: {html}");
        assert!(html.contains("<style>"), "got: {html}");
        assert!(html.contains(".admonition"), "stylesheet missing: {html}");
    }

    #[test]
    fn example_block_renders_single_wrapper_with_children() {
        // One `<div class="exampleblock">` wrapping the rendered children (no
        // nested `.content`), the body parsed normally (`*world*` → `<strong>`).
        let html = render("====\nHello *world*.\n====");
        assert!(
            html.contains(
                "<div class=\"exampleblock\">\n<p>Hello <strong>world</strong>.</p>\n</div>"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn example_block_id_and_roles_on_wrapper() {
        // id/roles hang on the `exampleblock` div (no extra wrapper), as elsewhere.
        let html = render("[#ex.box]\n====\nhi\n====");
        assert!(
            html.contains("<div class=\"exampleblock box\" id=\"ex\">"),
            "got: {html}"
        );
    }

    #[test]
    fn example_style_paragraph_renders_as_example() {
        let html = render("[example]\nJust text.");
        assert!(
            html.contains("<div class=\"exampleblock\">\n<p>Just text.</p>\n</div>"),
            "got: {html}"
        );
    }

    #[test]
    fn example_stylesheet_rule_is_shipped() {
        let html = render("Hello.");
        assert!(html.contains(".exampleblock"), "stylesheet missing: {html}");
    }

    #[test]
    fn sidebar_block_renders_aside_with_children() {
        // One semantic `<aside class="sidebarblock">` wrapping the rendered children
        // (no nested `.content`), the body parsed normally (`*world*` → `<strong>`).
        let html = render("****\nHello *world*.\n****");
        assert!(
            html.contains(
                "<aside class=\"sidebarblock\">\n<p>Hello <strong>world</strong>.</p>\n</aside>"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn sidebar_block_id_and_roles_on_aside() {
        // id/roles hang on the `sidebarblock` aside (no extra wrapper), as elsewhere.
        let html = render("[#sb.box]\n****\nhi\n****");
        assert!(
            html.contains("<aside class=\"sidebarblock box\" id=\"sb\">"),
            "got: {html}"
        );
    }

    #[test]
    fn sidebar_style_paragraph_renders_as_sidebar() {
        let html = render("[sidebar]\nJust text.");
        assert!(
            html.contains("<aside class=\"sidebarblock\">\n<p>Just text.</p>\n</aside>"),
            "got: {html}"
        );
    }

    #[test]
    fn sidebar_title_renders_inside_aside() {
        // The `.Title` goes just inside the `<aside>`, before the body (sidebars are
        // not caption-numbered, so the title carries no `Example N.` prefix).
        let html = render(".My note\n****\nhi\n****");
        assert!(
            html.contains(
                "<aside class=\"sidebarblock\">\n<div class=\"title\">My note</div>\n<p>hi</p>\n</aside>"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn sidebar_stylesheet_rule_is_shipped() {
        let html = render("Hello.");
        assert!(html.contains(".sidebarblock"), "stylesheet missing: {html}");
    }

    #[test]
    fn open_block_renders_div_with_children() {
        // One `<div class="openblock">` wrapping the rendered children (no nested
        // `.content`), the body parsed normally (`*world*` → `<strong>`).
        let html = render("--\nHello *world*.\n--");
        assert!(
            html.contains(
                "<div class=\"openblock\">\n<p>Hello <strong>world</strong>.</p>\n</div>"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn open_block_id_and_roles_on_div() {
        let html = render("[#o.box]\n--\nhi\n--");
        assert!(
            html.contains("<div class=\"openblock box\" id=\"o\">"),
            "got: {html}"
        );
    }

    #[test]
    fn open_style_paragraph_renders_as_open() {
        let html = render("[open]\nJust text.");
        assert!(
            html.contains("<div class=\"openblock\">\n<p>Just text.</p>\n</div>"),
            "got: {html}"
        );
    }

    #[test]
    fn quote_style_on_open_renders_as_quote_not_openblock() {
        // The masquerade is a parser concern; by render time a `[quote]` on `--` is a
        // genuine quote block, so it renders the `quoteblock` figure, not an openblock.
        let html = render("[quote]\n--\nMasqueraded.\n--");
        assert!(
            html.contains("<figure class=\"quoteblock\">"),
            "got: {html}"
        );
        // No openblock *element* is rendered (the shipped stylesheet still defines a
        // `.openblock` rule, so we check the rendered tag, not the CSS).
        assert!(!html.contains("<div class=\"openblock\""), "got: {html}");
    }

    #[test]
    fn open_stylesheet_rule_is_shipped() {
        let html = render("Hello.");
        assert!(html.contains(".openblock"), "stylesheet missing: {html}");
    }

    #[test]
    fn paragraph_title_renders_before_the_element() {
        // adoctra has no wrapping `<div class="paragraph">`, so the title is a
        // sibling `<div class="title">` rendered immediately before the `<p>`.
        let html = render(".A caption\nBody.");
        assert!(
            html.contains("<div class=\"title\">A caption</div>\n<p>Body.</p>"),
            "got: {html}"
        );
    }

    #[test]
    fn block_title_renders_inline_markup() {
        let html = render(".A *bold* caption\nBody.");
        assert!(
            html.contains("<div class=\"title\">A <strong>bold</strong> caption</div>"),
            "got: {html}"
        );
    }

    #[test]
    fn listing_title_renders_before_pre() {
        let html = render(".Code\n----\nx\n----");
        assert!(
            html.contains("<div class=\"title\">Code</div>\n<pre"),
            "got: {html}"
        );
    }

    #[test]
    fn example_title_renders_inside_the_wrapper() {
        // The title goes just inside `<div class="exampleblock">`, before the
        // children (matching asciidoctor's content ordering). A titled example is
        // numbered, so the title carries the `Example 1. ` caption prefix.
        let html = render(".Demo\n====\nhi\n====");
        assert!(
            html.contains(
                "<div class=\"exampleblock\">\n<div class=\"title\">Example 1. Demo</div>\n<p>hi</p>\n</div>"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn admonition_title_renders_after_the_label() {
        // A `.Title` on an admonition is the block's own title (distinct from the
        // `Note` label), rendered just below the label.
        let html = render(".My note\n[NOTE]\nbody");
        assert!(
            html.contains(
                "<p class=\"admonition-title\">Note</p>\n<div class=\"title\">My note</div>"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn untitled_block_emits_no_title_div() {
        let html = render("Just body.");
        assert!(!html.contains("class=\"title\""), "got: {html}");
    }

    #[test]
    fn block_title_stylesheet_rule_is_shipped() {
        let html = render("Hello.");
        assert!(html.contains(".title {"), "stylesheet missing: {html}");
    }

    #[test]
    fn titled_example_caption_prefixes_the_title() {
        // The resolved `"Example 1. "` caption renders verbatim inside the title
        // div, before the title text (asciidoctor's `#{caption}#{title}`).
        let html = render(".First\n====\none\n====\n\n.Second\n====\ntwo\n====");
        assert!(
            html.contains("<div class=\"title\">Example 1. First</div>"),
            "got: {html}"
        );
        assert!(
            html.contains("<div class=\"title\">Example 2. Second</div>"),
            "got: {html}"
        );
    }

    #[test]
    fn untitled_example_renders_no_caption() {
        // No title → no caption (and the counter is untouched, asserted in core).
        let html = render("====\nbody\n====");
        assert!(!html.contains("class=\"title\""), "got: {html}");
    }

    #[test]
    fn titled_listing_renders_bare_title_without_caption() {
        // Only example blocks are numbered; a titled listing keeps its bare title.
        let html = render(".Code\n----\nputs 1\n----");
        assert!(
            html.contains("<div class=\"title\">Code</div>"),
            "got: {html}"
        );
    }

    // --- quote & verse blocks ---

    #[test]
    fn quote_block_renders_figure_blockquote_and_attribution() {
        let html = render("[quote, Albert Einstein, Theory]\n____\nA *bold* quote.\n____");
        assert!(
            html.contains("<figure class=\"quoteblock\">"),
            "got: {html}"
        );
        assert!(
            html.contains("<blockquote>\n<p>A <strong>bold</strong> quote.</p>\n</blockquote>"),
            "got: {html}"
        );
        assert!(
            html.contains(
                "<figcaption>&#8212; Albert Einstein<br>\n<cite>Theory</cite></figcaption>"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn quote_without_attribution_has_no_figcaption() {
        let html = render("____\nx\n____");
        assert!(
            html.contains("<figure class=\"quoteblock\">"),
            "got: {html}"
        );
        assert!(!html.contains("<figcaption>"), "got: {html}");
    }

    #[test]
    fn quote_id_and_roles_on_figure() {
        let html = render("[#q.box]\n____\nx\n____");
        assert!(
            html.contains("<figure class=\"quoteblock box\" id=\"q\">"),
            "got: {html}"
        );
    }

    #[test]
    fn quote_citetitle_only_omits_the_em_dash() {
        // `[quote, , Fog]` — no author, so no em dash, just the `<cite>`.
        let html = render("[quote, , Fog]\n____\nx\n____");
        assert!(
            html.contains("<figcaption><cite>Fog</cite></figcaption>"),
            "got: {html}"
        );
    }

    #[test]
    fn quote_attribution_uses_ruby_field_keys() {
        // A named field occupies the author slot; a quoted empty author keeps
        // the em dash, as asciidoctor renders it.
        let html = render("[quote, role=x, Fog]\n____\nx\n____");
        assert!(
            html.contains("<figcaption><cite>Fog</cite></figcaption>"),
            "got: {html}"
        );
        let html = render("[quote,\"\",W]\nx");
        assert!(
            html.contains("<figcaption>&#8212; <br>\n<cite>W</cite></figcaption>"),
            "got: {html}"
        );
    }

    #[test]
    fn unquoted_attribution_is_raw_and_single_quoted_is_substituted() {
        // asciidoctor emits an unquoted value as is; attribute values keep the
        // protection from their definition; single quotes run normal subs.
        let html = render(":who: A<b>\n\n[quote, *Ann* & <i>x</i> {who}, '*W* & (C)']\nx");
        assert!(
            html.contains(
                "<figcaption>&#8212; *Ann* & <i>x</i> A&lt;b&gt;<br>\n<cite><strong>W</strong> &amp; &#169;</cite></figcaption>"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn list_titles_and_reftext_render_like_dot_titles_and_anchors() {
        let html = render("[#p,title=*T* & U,reftext='*R*']\npara\n\n<<p>>");
        assert!(
            html.contains("<div class=\"title\"><strong>T</strong> &amp; U</div>"),
            "got: {html}"
        );
        assert!(
            html.contains("<a href=\"#p\"><strong>R</strong></a>"),
            "got: {html}"
        );
        // A block title is carried past a section heading.
        let html = render(".T\n== S\n\npara");
        assert!(
            html.contains("<h2 id=\"_s\">S</h2>\n<div class=\"title\">T</div>"),
            "got: {html}"
        );
    }

    #[test]
    fn cataloged_inline_anchors_resolve_and_escaped_anchors_are_text() {
        let html = render(
            "p\n\nA [[a,*R* {counter:n}]] \\[[b,S]]\n\n[NOTE]\nC [[c,T]]\n\n<<a>> <<b>> <<c>>",
        );
        assert!(html.contains("A <a id=\"a\"></a> [[b,S]]"), "got: {html}");
        assert!(
            html.contains("<a href=\"#a\"><strong>R</strong> 1</a> <a href=\"#b\">[b]</a> <a href=\"#c\">[c]</a>"),
            "got: {html}"
        );
    }

    #[test]
    fn admonition_labels_and_heading_classes_follow_core() {
        // A `caption` attribute is emitted raw, as in asciidoctor; aria-label escapes.
        let html =
            render("[NOTE,caption=\"Say \\\"hi\\\" & <go>\"]\nx\n\n:tip-caption!:\n\nTIP: y");
        assert!(
            html.contains("aria-label=\"Say &quot;hi&quot; &amp; &lt;go&gt;\">\n<p class=\"admonition-title\">Say \"hi\" & <go></p>"),
            "got: {html}"
        );
        assert!(
            html.contains("data-type=\"tip\">\n<p class=\"admonition-title\"></p>"),
            "got: {html}"
        );
        let html = render("= Doc\n\n[float#f.r]\n=== F\n\n= Part");
        assert!(html.contains("<h1>Doc</h1>"), "got: {html}");
        assert!(
            html.contains("<h3 id=\"f\" class=\"float r\">F</h3>"),
            "got: {html}"
        );
        assert!(
            html.contains("<h1 id=\"_part\" class=\"sect0\">Part</h1>"),
            "got: {html}"
        );
    }

    #[test]
    fn verse_block_renders_pre_preserving_newlines() {
        let html = render("[verse]\n____\nThe fog comes\non little cat feet.\n____");
        assert!(
            html.contains("<figure class=\"verseblock\">"),
            "got: {html}"
        );
        assert!(
            html.contains("<pre class=\"verse\">The fog comes\non little cat feet.</pre>"),
            "got: {html}"
        );
    }

    #[test]
    fn verse_renders_attribution_figcaption() {
        let html = render("[verse, Carl Sandburg, Fog]\n____\nx\n____");
        assert!(
            html.contains("<figcaption>&#8212; Carl Sandburg<br>\n<cite>Fog</cite></figcaption>"),
            "got: {html}"
        );
    }

    #[test]
    fn quote_and_verse_stylesheet_rules_are_shipped() {
        let html = render("Hello.");
        assert!(
            html.contains(".quoteblock"),
            "stylesheet missing quote: {html}"
        );
        assert!(
            html.contains(".verseblock"),
            "stylesheet missing verse: {html}"
        );
    }

    // --- verbatim masquerades on a `--` open block ---

    #[test]
    fn source_style_on_open_renders_highlight_like_listing() {
        // `[source,ruby]` on a `--` block renders the same source highlight form as
        // the `----` listing: verbatim, no inline subs.
        let html = render("[source,ruby]\n--\nputs *x*\n--");
        assert!(
            html.contains(
                "<pre class=\"highlight\"><code class=\"language-ruby\" data-lang=\"ruby\">puts *x*</code></pre>"
            ),
            "got: {html}"
        );
    }

    #[test]
    fn literal_style_on_open_renders_literalblock() {
        let html = render("[literal]\n--\nverbatim *not bold*\n--");
        assert!(
            html.contains("<pre class=\"literalblock\">verbatim *not bold*</pre>"),
            "got: {html}"
        );
    }

    #[test]
    fn verse_style_on_open_renders_verseblock_preserving_newlines() {
        // `[verse, …]` on a `--` block renders the verseblock idiom: inline subs
        // apply (`*fog*` → `<strong>`), the line break survives, attribution shows.
        let html = render("[verse, Carl Sandburg, Fog]\n--\nThe *fog* comes\non feet.\n--");
        assert!(
            html.contains("<pre class=\"verse\">The <strong>fog</strong> comes\non feet.</pre>"),
            "got: {html}"
        );
        assert!(
            html.contains("<figcaption>&#8212; Carl Sandburg<br>\n<cite>Fog</cite></figcaption>"),
            "got: {html}"
        );
    }
}
