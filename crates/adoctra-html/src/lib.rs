//! HTML5 backend.

use adoctra_core::ast::{Attribution, Block, BlockKind, Inline};
use adoctra_core::{Backend, Document, Result};

mod math;

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
        out.push_str("</style>\n");
        icon_font(&doc.attributes, &mut out);
        math::configuration(&doc.final_attributes, &doc.attributes, &mut out);
        out.push_str("</head>\n<body>\n");
        let cx = Cx {
            doc,
            book: doc.attributes.get("doctype") == Some("book"),
        };
        let attributes = &doc.attributes;
        // Ruby's embedded document puts an automatic TOC after the title. Like
        // its converter, every TOC reads the header attributes except the
        // `toc::[]` macro, which reads them at its position.
        let auto_toc = cx.has_sections()
            && attributes.get("toc").is_some()
            && !matches!(attributes.get("toc-placement"), Some("macro" | "preamble"));
        let title = usize::from(doc.has_title);
        if doc.manpage.is_some() && !doc.has_title {
            render_manpage_header(doc, &mut out);
        }
        for (i, block) in doc.blocks.iter().enumerate() {
            if i == title && auto_toc {
                render_toc(&cx, attributes, "toc", None, None, &mut out);
            }
            match &block.kind {
                BlockKind::Heading { .. } if i == 0 && doc.manpage.is_some() && doc.has_title => {
                    render_manpage_header(doc, &mut out);
                }
                BlockKind::Heading { level, inlines } if i == 0 && doc.has_title => {
                    render_heading(block, *level, inlines, false, &block.roles, &mut out);
                }
                // Like Ruby, a partintro renders only as a child of a book part.
                BlockKind::Section { level: 0, blocks } if cx.book => {
                    for child in blocks {
                        match &child.kind {
                            BlockKind::Open { blocks } if is_style(child, "partintro") => {
                                render_open(child, blocks, &cx, &mut out)
                            }
                            _ => render_block(child, &cx, &mut out),
                        }
                    }
                }
                // Ruby excludes an abstract directly in a book (no preamble).
                BlockKind::Open { .. } if cx.book && is_style(block, "abstract") => {}
                // asciidoctor's `<div id="preamble">` (without its `sectionbody`),
                // ending with the TOC when `toc-placement` is `preamble`.
                BlockKind::Preamble { blocks } => {
                    out.push_str("<section id=\"preamble\">\n");
                    for child in blocks {
                        render_block(child, &cx, &mut out);
                    }
                    if cx.has_sections()
                        && attributes.get("toc").is_some()
                        && attributes.get("toc-placement") == Some("preamble")
                    {
                        let class = attributes.get("toc-class").unwrap_or("toc");
                        render_toc(&cx, attributes, class, None, None, &mut out);
                    }
                    out.push_str("</section>\n");
                }
                _ => render_block(block, &cx, &mut out),
            }
        }
        if doc.blocks.len() == title && auto_toc {
            render_toc(&cx, attributes, "toc", None, None, &mut out);
        }
        render_footnotes(&doc.footnotes, attributes, &mut out);
        out.push_str("</body>\n</html>\n");
        Ok(out.into_bytes())
    }
}

/// Ruby's manual page header (`convert_embedded`): the document title with
/// ` Manual Page` (without a title, the first section's, like Ruby's
/// `doctitle`), then the NAME section from the header attributes when it has
/// a purpose (`generate_manname_section`), before an automatic TOC. adoctra
/// renders that section like the others.
fn render_manpage_header(doc: &Document, out: &mut String) {
    let first_section = doc.blocks.iter().find_map(|block| match &block.kind {
        BlockKind::Section { blocks, .. } => match blocks.first().map(|b| &b.kind) {
            Some(BlockKind::Heading { inlines, .. }) => Some(inlines),
            _ => None,
        },
        _ => None,
    });
    let title = doc.blocks.first().filter(|_| doc.has_title);
    let (id, class) = title.map_or_else(Default::default, |block| {
        (id_attr(&block.id), class_attr(&[], &block.roles))
    });
    out.push_str(&format!("<h1{id}{class}>"));
    let inlines = match title.map(|block| &block.kind) {
        Some(BlockKind::Heading { inlines, .. }) => Some(inlines),
        _ => first_section,
    };
    if let Some(inlines) = inlines {
        render_inlines(inlines, out);
    }
    out.push_str(" Manual Page</h1>\n");
    let attributes = &doc.attributes;
    let (Some(manpage), Some(_)) = (&doc.manpage, attributes.get("manpurpose")) else {
        return;
    };
    let section = manpage
        .name_section
        .as_ref()
        .filter(|_| manpage.section_title)
        .map(|section| &section.heading.kind);
    let mut name_title = match section {
        Some(BlockKind::Heading { inlines, .. }) => {
            let mut title = String::new();
            render_inlines(inlines, &mut title);
            title
        }
        _ => attribute_html(attributes, "manname-title").unwrap_or("Name".into()),
    };
    // Ruby upcases the label when the first section's converted title is
    // uppercase; adoctra leaves markup and character references alone.
    if let Some(inlines) = first_section {
        let mut converted = String::new();
        render_inlines(inlines, &mut converted);
        if converted == converted.to_uppercase() {
            name_title = upcase_text(&name_title);
        }
    }
    let id = id_attr(&attributes.get("manname-id").map(str::to_owned));
    out.push_str(&format!(
        "<section class=\"sect1\">\n<h2{id}>{name_title}</h2>\n<p>{} - {}</p>\n</section>\n",
        manpage.names.join(", "),
        attribute_html(attributes, "manpurpose").unwrap_or_default(),
    ));
}

/// An attribute value as HTML: its parts, or escaped text.
fn attribute_html(attributes: &adoctra_core::Attributes, key: &str) -> Option<String> {
    let value = attributes.get(key)?;
    let mut out = String::new();
    match attributes.raw_parts(key) {
        Some(parts) => render_parts(parts, &mut out),
        None => out.push_str(&escape_text(value)),
    }
    Some(out)
}

/// Uppercase the text of an HTML fragment outside tags and character
/// references.
fn upcase_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(ch) = rest.chars().next() {
        let end = match ch {
            '<' => rest.find('>').map(|end| end + 1),
            '&' => rest
                .find(';')
                .filter(|&end| {
                    rest[1..end]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '#')
                })
                .map(|end| end + 1),
            _ => None,
        };
        match end {
            Some(end) => {
                out.push_str(&rest[..end]);
                rest = &rest[end..];
            }
            None => {
                out.extend(ch.to_uppercase());
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    out
}

/// What a block needs from its document while rendering.
struct Cx<'a> {
    doc: &'a Document,
    book: bool,
}

impl Cx<'_> {
    /// Ruby's `Document#sections?`.
    fn has_sections(&self) -> bool {
        self.doc
            .blocks
            .iter()
            .any(|block| matches!(block.kind, BlockKind::Section { .. }))
    }
}

fn render_block(block: &Block, cx: &Cx, out: &mut String) {
    // A block's anchor (an explicit `[[id]]`/`[#id]` or a section's auto-generated
    // id) renders as an `id` attribute on the block's element; `None` → none.
    // `[.role]` shorthand roles render as a `class` (appended to any base class
    // such as a listing's); asciidoctor puts both on a wrapping `<div>`, but
    // adoctra emits no wrapper, so they go on the inner element (as the id
    // already does).
    let id = id_attr(&block.id);
    match &block.kind {
        BlockKind::Heading { level, inlines } => {
            render_heading(block, *level, inlines, true, &block.roles, out)
        }
        BlockKind::Paragraph { inlines } => {
            let class = class_attr(&[], &block.roles);
            render_title(&block.title, &block.caption, out);
            out.push_str(&format!("<p{id}{class}>"));
            render_inlines(inlines, out);
            out.push_str("</p>\n");
        }
        BlockKind::Listing {
            text,
            callouts,
            inlines,
        } => {
            render_title(&block.title, &block.caption, out);
            // A `[source]`-style listing renders as asciidoctor's highlight form:
            // `<pre class="highlight"><code class="language-LANG" data-lang="LANG">`,
            // with the language taken from the `language` attribute that core
            // folded out of `[source,LANG]`. Without `source` it stays a plain
            // listing. asciidoctor hangs roles/id on a wrapping `<div>`; adoctra
            // has no wrapper, so they go on the `<pre>` (roles appended to the base
            // class), as elsewhere.
            if block.attrs.style.as_deref() == Some("source") {
                let class = class_attr(&["highlight", nowrap(block)], &block.roles);
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
                render_verbatim(text, callouts, inlines, out);
                out.push_str("</code></pre>\n");
            } else {
                let class = class_attr(&["listing", nowrap(block)], &block.roles);
                out.push_str(&format!("<pre{class}{id}>"));
                render_verbatim(text, callouts, inlines, out);
                out.push_str("</pre>\n");
            }
        }
        BlockKind::Literal {
            text,
            callouts,
            inlines,
        } => {
            // A literal block: verbatim `<pre>`, like a plain listing but with the
            // `literalblock` base class. asciidoctor wraps it in a `<div>`; adoctra
            // hangs the class/id/roles on the `<pre>` itself, as for a listing.
            render_title(&block.title, &block.caption, out);
            let class = class_attr(&["literalblock", nowrap(block)], &block.roles);
            out.push_str(&format!("<pre{class}{id}>"));
            render_verbatim(text, callouts, inlines, out);
            out.push_str("</pre>\n");
        }
        BlockKind::CalloutList { items } => {
            // Ruby's `div.colist` without its wrapper: an ordered list whose
            // items explain the callout marks. With `icons` Ruby lays out the
            // number icon beside the text in a table; adoctra keeps the list
            // and the stylesheet lays out the same icon and text columns.
            render_title(&block.title, &block.caption, out);
            // Core captured the `icons` mode where the list converts.
            let icons = items.first().and_then(|entry| entry.icons.as_deref());
            let base: &[&str] = if icons.is_some() {
                &["colist", "arabic", "icons"]
            } else {
                &["colist", "arabic"]
            };
            let class = class_attr(base, &block.roles);
            out.push_str(&format!("<ol{id}{class}>\n"));
            for (index, entry) in items.iter().enumerate() {
                let number = index + 1;
                out.push_str("<li>");
                if let Some(icons) = icons {
                    out.push_str("<span class=\"colist-icon\">");
                    if icons == "font" {
                        out.push_str(&format!(
                            "<i class=\"conum\" data-value=\"{number}\"></i><b>{number}</b>"
                        ));
                    } else {
                        out.push_str(&format!(
                            "<img src=\"{}\" alt=\"{number}\">",
                            attr_value(&entry.src)
                        ));
                    }
                    out.push_str("</span><div>");
                }
                render_inlines(&entry.item.inlines, out);
                for child in &entry.item.blocks {
                    out.push('\n');
                    render_block(child, cx, out);
                }
                if icons.is_some() {
                    out.push_str("</div>");
                }
                out.push_str("</li>\n");
            }
            out.push_str("</ol>\n");
        }
        BlockKind::Stem { style, inlines } => {
            let class = class_attr(&["stemblock"], &block.roles);
            out.push_str(&format!("<div{id}{class}>\n"));
            render_title(&block.title, &block.caption, out);
            out.push_str("<div class=\"content\">\n");
            let mut equation = String::new();
            render_inlines(inlines, &mut equation);
            math::equation(*style, &equation, out);
            out.push_str("\n</div>\n</div>\n");
        }
        BlockKind::Passthrough { text, inlines } => {
            // Passthrough: the body is emitted *completely raw* — no escaping and
            // no wrapper element at all (asciidoctor's `:pass` context). Unlike
            // every other block, the id/roles are dropped: there is no element to
            // hang them on, matching asciidoctor. Its title is not rendered.
            // Custom substitutions emit their result, still unwrapped.
            match inlines {
                Some(inlines) => render_inlines(inlines, out),
                None => out.push_str(text),
            }
            out.push('\n');
        }
        BlockKind::Image { src, alt, .. } => {
            // A semantic `<figure class="imageblock">` holding the image (inside
            // an `<a class="image">` with `link`) and a trailing `<figcaption>`
            // for the captioned title. asciidoctor nests a `content` div and
            // ends with `<div class="title">`; `float`/`align`/roles are classes
            // as in Ruby's `convert_image` (secure mode: always an `<img>`).
            let named = &block.attrs.named;
            let align = named.get("align").map(|align| format!("text-{align}"));
            let mut base = vec!["imageblock"];
            base.extend(named.get("float").map(String::as_str));
            base.extend(align.as_deref());
            let class = class_attr(&base, &block.roles);
            out.push_str(&format!("<figure{id}{class}>\n"));
            let mut img = format!("<img src=\"{}\" alt=\"", attr_value(src));
            for part in alt {
                match part {
                    adoctra_core::AttributePart::Raw(raw) => {
                        img.push_str(&raw.replace('"', "&quot;"))
                    }
                    part => img.push_str(&attr_value(&adoctra_core::attributes::parts_text(
                        std::slice::from_ref(part),
                    ))),
                }
            }
            img.push('"');
            for key in ["width", "height"] {
                if let Some(value) = named.get(key) {
                    img.push_str(&format!(" {key}=\"{}\"", attr_value(value)));
                }
            }
            img.push('>');
            match named.get("link") {
                Some(link) => out.push_str(&format!(
                    "<a class=\"image\" href=\"{}\"{}>{img}</a>\n",
                    attr_value(link),
                    link_constraints(&block.attrs)
                )),
                None => {
                    out.push_str(&img);
                    out.push('\n');
                }
            }
            if let Some(title) = &block.title {
                out.push_str("<figcaption class=\"title\">");
                if let Some(caption) = &block.caption {
                    out.push_str(&escape(caption));
                }
                render_inlines(title, out);
                out.push_str("</figcaption>\n");
            }
            out.push_str("</figure>\n");
        }
        BlockKind::Table(table) => render_table(block, table, cx, out),
        BlockKind::Video {
            src,
            poster,
            embedded,
            ..
        } => {
            // A semantic `<figure class="videoblock">` with the title first as
            // a `<figcaption>` (Ruby's leading title `<div>`), then the player:
            // an `<iframe>` for YouTube/Vimeo, else a `<video>` with Ruby's
            // fallback text. asciidoctor nests a `content` div and adds the
            // obsolete `frameborder="0"`, which the stylesheet replaces.
            let named = &block.attrs.named;
            let option = |name: &str| block.attrs.options.iter().any(|o| o == name);
            let align = named.get("align").map(|align| format!("text-{align}"));
            let mut base = vec!["videoblock"];
            base.extend(named.get("float").map(String::as_str));
            base.extend(align.as_deref());
            let class = class_attr(&base, &block.roles);
            out.push_str(&format!("<figure{id}{class}>\n"));
            render_media_title(&block.title, out);
            let mut size = String::new();
            for key in ["width", "height"] {
                if let Some(value) = named.get(key) {
                    size.push_str(&format!(" {key}=\"{}\"", attr_value(value)));
                }
            }
            if *embedded {
                let fullscreen = if option("nofullscreen") {
                    ""
                } else {
                    " allowfullscreen"
                };
                out.push_str(&format!(
                    "<iframe{size} src=\"{}\"{fullscreen}></iframe>\n",
                    attr_value(src)
                ));
            } else {
                out.push_str(&format!("<video src=\"{}\"{size}", attr_value(src)));
                if let Some(poster) = poster {
                    out.push_str(&format!(" poster=\"{}\"", attr_value(poster)));
                }
                for (name, on) in [
                    ("autoplay", option("autoplay")),
                    ("muted", option("muted")),
                    ("controls", !option("nocontrols")),
                    ("loop", option("loop")),
                ] {
                    if on {
                        out.push(' ');
                        out.push_str(name);
                    }
                }
                if let Some(preload) = named.get("preload").filter(|p| !p.is_empty()) {
                    out.push_str(&format!(" preload=\"{}\"", attr_value(preload)));
                }
                out.push_str(">\nYour browser does not support the video tag.\n</video>\n");
            }
            out.push_str("</figure>\n");
        }
        BlockKind::Audio { src, .. } => {
            // Like a video, without float/align classes (Ruby's `convert_audio`).
            let option = |name: &str| block.attrs.options.iter().any(|o| o == name);
            let class = class_attr(&["audioblock"], &block.roles);
            out.push_str(&format!("<figure{id}{class}>\n"));
            render_media_title(&block.title, out);
            out.push_str(&format!("<audio src=\"{}\"", attr_value(src)));
            for (name, on) in [
                ("autoplay", option("autoplay")),
                ("controls", !option("nocontrols")),
                ("loop", option("loop")),
            ] {
                if on {
                    out.push(' ');
                    out.push_str(name);
                }
            }
            out.push_str(">\nYour browser does not support the audio tag.\n</audio>\n</figure>\n");
        }
        BlockKind::List { ordered, items } => {
            let tag = if *ordered { "ol" } else { "ul" };
            let option = |name: &str| {
                block.attrs.options.iter().any(|o| o == name)
                    || block.attrs.named.contains_key(&format!("{name}-option"))
            };
            let checklist = !*ordered && option("checklist");
            // Ruby's list class: the style (`checklist` for a checklist), then
            // roles; an ordered style other than arabic also sets `type`.
            let style = if checklist {
                Some("checklist")
            } else {
                block.attrs.style.as_deref()
            };
            let class = class_attr(&style.into_iter().collect::<Vec<_>>(), &block.roles);
            let mut extra = String::new();
            // Ruby's implicit style from a numbered or lettered marker is a
            // symbol without a `type` keyword.
            let marker_style = items
                .first()
                .and_then(|item| item.marker.as_deref())
                .filter(|marker| !marker.starts_with('.'))
                .map(|marker| match marker {
                    "a." => "loweralpha",
                    "A." => "upperalpha",
                    "i)" => "lowerroman",
                    "I)" => "upperroman",
                    _ => "arabic",
                });
            if *ordered {
                let keyword = match style.filter(|s| Some(*s) != marker_style) {
                    Some("loweralpha") => Some("a"),
                    Some("lowerroman") => Some("i"),
                    Some("upperalpha") => Some("A"),
                    Some("upperroman") => Some("I"),
                    _ => None,
                };
                if let Some(keyword) = keyword {
                    extra.push_str(&format!(" type=\"{keyword}\""));
                }
                if let Some(start) = block.attrs.named.get("start") {
                    extra.push_str(&format!(" start=\"{}\"", attr_value(start)));
                }
                if option("reversed") {
                    extra.push_str(" reversed");
                }
            }
            let (checked, unchecked) = if option("interactive") {
                (
                    "<input type=\"checkbox\" data-item-complete=\"1\" checked> ",
                    "<input type=\"checkbox\" data-item-complete=\"0\"> ",
                )
            } else if cx.doc.attributes.get("icons") == Some("font") {
                (
                    "<i class=\"fa fa-check-square-o\"></i> ",
                    "<i class=\"fa fa-square-o\"></i> ",
                )
            } else {
                ("&#10003; ", "&#10063; ")
            };
            render_title(&block.title, &block.caption, out);
            out.push_str(&format!("<{tag}{id}{class}{extra}>\n"));
            for item in items {
                out.push_str("<li>");
                match item.checkbox.filter(|_| checklist) {
                    Some(true) => out.push_str(checked),
                    Some(false) => out.push_str(unchecked),
                    None => {}
                }
                render_inlines(&item.inlines, out);
                // Nested blocks (a sublist, or a `+`-attached paragraph/listing)
                // render inside the <li>, after the item's principal text. Each
                // starts on its own line, matching asciidoctor's structure and
                // keeping the principal text from running into the block's text.
                for child in &item.blocks {
                    out.push('\n');
                    render_block(child, cx, out);
                }
                out.push_str("</li>\n");
            }
            out.push_str(&format!("</{tag}>\n"));
        }
        BlockKind::DescriptionList { items } => render_dlist(block, items, cx, out),
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
            // to `admonition`). The paragraph/label forms are simple content:
            // their single child paragraph renders as bare text, like Ruby.
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
            render_body(block, blocks, cx, out);
            out.push_str("</aside>\n");
        }
        BlockKind::Example { blocks } if has_option(block, "collapsible") => {
            // Ruby's collapsible example: `<details>` (open with `%open`) whose
            // `<summary>` is the uncaptioned title, else `Details` verbatim.
            let class = class_attr(&[], &block.roles);
            let open = if has_option(block, "open") {
                " open"
            } else {
                ""
            };
            out.push_str(&format!(
                "<details{id}{class}{open}>\n<summary class=\"title\">"
            ));
            match &block.title {
                Some(title) => render_inlines(title, out),
                None => out.push_str("Details"),
            }
            out.push_str("</summary>\n");
            render_body(block, blocks, cx, out);
            out.push_str("</details>\n");
        }
        BlockKind::Example { blocks } => {
            // A parsed container: one `<div class="exampleblock">` wrapping the
            // rendered children. asciidoctor nests `<div class="content">` inside;
            // adoctra keeps the no-wrapper convention — a single element carries
            // the class plus the block's id/roles, children render straight in.
            let class = class_attr(&["exampleblock"], &block.roles);
            out.push_str(&format!("<div{class}{id}>\n"));
            render_title(&block.title, &block.caption, out);
            render_body(block, blocks, cx, out);
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
            render_body(block, blocks, cx, out);
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
            render_body(block, blocks, cx, out);
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
        BlockKind::Section { level: 0, blocks } => {
            // Like Ruby, a level-0 section (a part) has no wrapper: its `sect0`
            // title (carrying the roles) is followed by its content.
            for child in blocks {
                render_block(child, cx, out);
            }
        }
        BlockKind::Section { level, blocks } => {
            // A semantic `<section class="sectN">` holding the title and content.
            // asciidoctor uses `<div class="sectN">` plus a `sectionbody` div for
            // level 1; adoctra keeps one element. The section roles (stored on its
            // title) hang on the `<section>`, the id on the heading, as in Ruby.
            let (heading, content) = blocks.split_first().expect("section title");
            let base = format!("sect{level}");
            out.push_str(&format!(
                "<section{}>\n",
                class_attr(&[base.as_str()], &heading.roles)
            ));
            if let BlockKind::Heading { level, inlines } = &heading.kind {
                render_heading(heading, *level, inlines, true, &[], out);
            }
            for child in content {
                render_block(child, cx, out);
            }
            out.push_str("</section>\n");
        }
        BlockKind::Preamble { blocks } => {
            // asciidoctor's `<div id="preamble">` (without its `sectionbody`).
            out.push_str("<section id=\"preamble\">\n");
            for child in blocks {
                render_block(child, cx, out);
            }
            out.push_str("</section>\n");
        }
        // A partintro outside a book part is excluded (see `convert`).
        BlockKind::Open { .. } if is_style(block, "partintro") => {}
        BlockKind::Open { blocks } if is_style(block, "abstract") => {
            // Ruby renders an abstract as a quote block with an `abstract` class;
            // adoctra uses the same figure/blockquote idiom as quotes.
            let class = class_attr(&["quoteblock", "abstract"], &block.roles);
            out.push_str(&format!("<figure{class}{id}>\n"));
            render_title(&block.title, &block.caption, out);
            out.push_str("<blockquote>\n");
            render_body(block, blocks, cx, out);
            out.push_str("</blockquote>\n</figure>\n");
        }
        BlockKind::Open { blocks } => render_open(block, blocks, cx, out),
        // Ruby's `convert_thematic_break` / `convert_page_break` ignore the
        // block's id, roles and title.
        BlockKind::ThematicBreak => out.push_str("<hr>\n"),
        BlockKind::PageBreak => out.push_str("<div style=\"page-break-after: always;\"></div>\n"),
        BlockKind::Toc => {
            // Ruby's `convert_toc`: only with `toc-placement` macro (the header
            // value `:toc: macro`), at the attributes of its position.
            let attributes = &block.attribute_context;
            if attributes.get("toc-placement") != Some("macro")
                || !cx.has_sections()
                || attributes.get("toc").is_none()
            {
                out.push_str("<!-- toc disabled -->\n");
                return;
            }
            let role = block.roles.join(" ");
            let class = if role.is_empty() {
                attributes.get("toc-class").unwrap_or("toc")
            } else {
                role.as_str()
            };
            let levels = block.attrs.named.get("levels").map(|v| ruby_to_i(v));
            let id = block.id.as_deref().unwrap_or("toc");
            let title = block.title.as_deref().map(|title| {
                let mut html = String::new();
                render_inlines(title, &mut html);
                html
            });
            render_toc(cx, attributes, class, Some((id, title)), levels, out);
        }
    }
}

/// A table of contents: Ruby's automatic/preamble TOC (`macro` is `None`) or
/// the `toc::[]` block (its id and converted title). adoctra uses a `<nav>`
/// where Ruby uses a `<div>`.
fn render_toc(
    cx: &Cx,
    attributes: &adoctra_core::Attributes,
    class: &str,
    macro_: Option<(&str, Option<String>)>,
    levels: Option<i64>,
    out: &mut String,
) {
    let (id, title_attrs, title) = match macro_ {
        Some((id, title)) => (
            id,
            format!(" id=\"{}title\" class=\"title\"", escape(id)),
            title,
        ),
        None => ("toc", " id=\"toctitle\"".to_string(), None),
    };
    out.push_str(&format!(
        "<nav id=\"{}\" class=\"{}\">\n<div{title_attrs}>",
        escape(id),
        escape(class)
    ));
    match title {
        Some(title) => out.push_str(&title),
        None => match attributes.raw_parts("toc-title") {
            Some(parts) => render_parts(parts, out),
            None => out.push_str(&escape_text(
                attributes.get("toc-title").unwrap_or_default(),
            )),
        },
    }
    out.push_str("</div>\n");
    let levels = levels.unwrap_or_else(|| attributes.get("toclevels").map_or(2, ruby_to_i));
    render_outline(&cx.doc.blocks, levels, out);
    out.push_str("</nav>\n");
}

/// Ruby's `convert_outline`: a `sectlevelN` list of links to the sections
/// among `blocks`, nested while a section's level is below `toclevels`.
/// Entry text is the heading's number caption and title with every `<a>` tag
/// removed (Ruby's `DropAnchorRx`). Returns whether there were sections.
fn render_outline(blocks: &[Block], toclevels: i64, out: &mut String) -> bool {
    let mut sections = blocks.iter().filter_map(|block| match &block.kind {
        BlockKind::Section { level, blocks } => Some((*level, blocks)),
        _ => None,
    });
    let Some((first, _)) = sections.clone().next() else {
        return false;
    };
    out.push_str(&format!("<ul class=\"sectlevel{first}\">\n"));
    for (level, blocks) in sections.by_ref() {
        let heading = &blocks[0];
        let mut title = String::new();
        if let Some(caption) = &heading.caption {
            title.push_str(&escape(caption));
        }
        if let BlockKind::Heading { inlines, .. } = &heading.kind {
            render_inlines(inlines, &mut title);
        }
        let href = escape(heading.id.as_deref().unwrap_or_default());
        out.push_str(&format!(
            "<li><a href=\"#{href}\">{}</a>",
            drop_anchors(&title)
        ));
        let mut children = String::new();
        if i64::from(level) < toclevels && render_outline(&blocks[1..], toclevels, &mut children) {
            out.push('\n');
            out.push_str(&children);
        }
        out.push_str("</li>\n");
    }
    out.push_str("</ul>\n");
    true
}

/// Remove `<a …>` and `</a>` tags, keeping their content, like Ruby's
/// `DropAnchorRx` (`<(?:a\b[^>]*|/a)>`).
fn drop_anchors(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let tag = &rest[at + 1..];
        let open = tag.strip_prefix('a').filter(|after| {
            !after
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        });
        let end = match (open, tag.starts_with("/a>")) {
            (Some(after), _) => after.find('>').map(|i| at + 2 + i + 1),
            (None, true) => Some(at + 4),
            _ => None,
        };
        match end {
            Some(end) => rest = &rest[end..],
            None => {
                out.push('<');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A description list. The default and styled forms keep Ruby's semantic
/// `<dl>` (without its wrapper `<div>`, so id, style and roles hang on the
/// list); `horizontal` is a `<dl class="hdlist">` laid out by the stylesheet
/// instead of Ruby's layout table, and `qanda` Ruby's ordered list of questions.
fn render_dlist(
    block: &Block,
    items: &[adoctra_core::ast::DescriptionListItem],
    cx: &Cx,
    out: &mut String,
) {
    let id = id_attr(&block.id);
    let style = block.attrs.style.as_deref();
    let description = |item: &adoctra_core::ast::ListItem, out: &mut String| {
        if !item.inlines.is_empty() {
            out.push_str("<p>");
            render_inlines(&item.inlines, out);
            out.push_str("</p>\n");
        }
        for child in &item.blocks {
            render_block(child, cx, out);
        }
    };
    render_title(&block.title, &block.caption, out);
    match style {
        Some("qanda") => {
            let class = class_attr(&["qanda"], &block.roles);
            out.push_str(&format!("<ol{id}{class}>\n"));
            for item in items {
                out.push_str("<li>\n");
                for term in &item.terms {
                    out.push_str("<p><em>");
                    render_inlines(&term.inlines, out);
                    out.push_str("</em></p>\n");
                }
                if let Some(dd) = &item.description {
                    description(dd, out);
                }
                out.push_str("</li>\n");
            }
            out.push_str("</ol>\n");
        }
        Some("horizontal") => {
            let class = class_attr(&["hdlist"], &block.roles);
            let named = &block.attrs.named;
            let width = |key: &str| {
                named
                    .get(key)
                    .map(|w| format!("{}%", w.trim_end_matches('%')))
            };
            let style_attr = match (width("labelwidth"), width("itemwidth")) {
                (None, None) => String::new(),
                (label, item) => format!(
                    " style=\"grid-template-columns: {} {};\"",
                    attr_value(&label.unwrap_or_else(|| "auto".into())),
                    attr_value(&item.unwrap_or_else(|| "1fr".into()))
                ),
            };
            let strong = block.attrs.options.iter().any(|o| o == "strong")
                || named.contains_key("strong-option");
            out.push_str(&format!("<dl{id}{class}{style_attr}>\n"));
            for item in items {
                for term in &item.terms {
                    out.push_str(if strong {
                        "<dt class=\"strong\">"
                    } else {
                        "<dt>"
                    });
                    render_inlines(&term.inlines, out);
                    out.push_str("</dt>\n");
                }
                out.push_str("<dd>\n");
                if let Some(dd) = &item.description {
                    description(dd, out);
                }
                out.push_str("</dd>\n");
            }
            out.push_str("</dl>\n");
        }
        style => {
            let class = class_attr(&style.into_iter().collect::<Vec<_>>(), &block.roles);
            let dt_class = if style.is_none() {
                " class=\"hdlist1\""
            } else {
                ""
            };
            out.push_str(&format!("<dl{id}{class}>\n"));
            for item in items {
                for term in &item.terms {
                    out.push_str(&format!("<dt{dt_class}>"));
                    render_inlines(&term.inlines, out);
                    out.push_str("</dt>\n");
                }
                if let Some(dd) = &item.description {
                    out.push_str("<dd>\n");
                    description(dd, out);
                    out.push_str("</dd>\n");
                }
            }
            out.push_str("</dl>\n");
        }
    }
}

/// Ruby's `convert_document` head: with font icons, the icon font stylesheet
/// from the header attributes — Font Awesome from the CDN (or `iconfont-cdn`)
/// unless `iconfont-remote` is unset, else `<iconfont-name>.css` under
/// `stylesdir`.
fn icon_font(attributes: &adoctra_core::Attributes, out: &mut String) {
    if attributes.get("icons") != Some("font") {
        return;
    }
    let href = if attributes.get("iconfont-remote").is_some() {
        match attributes.get("iconfont-cdn") {
            Some(cdn) => cdn.to_owned(),
            None => {
                let scheme = attributes.get("asset-uri-scheme").unwrap_or("https");
                let colon = if scheme.is_empty() { "" } else { ":" };
                format!("{scheme}{colon}//cdnjs.cloudflare.com/ajax/libs/font-awesome/4.7.0/css/font-awesome.min.css")
            }
        }
    } else {
        let name = attributes.get("iconfont-name").unwrap_or("font-awesome");
        adoctra_core::normalize_web_path(&format!("{name}.css"), attributes.get("stylesdir"), false)
    };
    out.push_str(&format!(
        "<link rel=\"stylesheet\" href=\"{}\">\n",
        attr_value(&href)
    ));
}

/// A callout mark as Ruby's `convert_inline_callout`, with the `icons` mode
/// core captured where the block converts. `guard` is already HTML.
fn render_conum(
    number: &str,
    guard: Option<&str>,
    xml: bool,
    icons: Option<&str>,
    src: &str,
    out: &mut String,
) {
    let n = escape(number);
    match icons {
        Some("font") => out.push_str(&format!(
            "<i class=\"conum\" data-value=\"{n}\"></i><b>({n})</b>"
        )),
        Some(_) => out.push_str(&format!("<img src=\"{}\" alt=\"{n}\">", attr_value(src))),
        None if xml => out.push_str(&format!("&lt;!--<b class=\"conum\">({n})</b>--&gt;")),
        None => {
            out.push_str(guard.unwrap_or_default());
            out.push_str(&format!("<b class=\"conum\">({n})</b>"));
        }
    }
}

/// A callout mark from custom block substitutions; its guard keeps the
/// lexical spelling of the substituted text.
fn render_callout(callout: &adoctra_core::InlineCallout, out: &mut String) {
    render_conum(
        &callout.number,
        callout.guard.as_deref(),
        callout.xml,
        callout.icons.as_deref(),
        &callout.src,
        out,
    );
}

/// Verbatim text with its callout marks: the guard kept before
/// `<b class="conum">(N)</b>` (an `<!--N-->` mark keeps its comment), or a
/// font/image icon with `icons`.
/// Content converted with custom substitutions renders instead.
fn render_verbatim(
    text: &str,
    callouts: &[adoctra_core::ast::Callout],
    inlines: &Option<Vec<Inline>>,
    out: &mut String,
) {
    if let Some(inlines) = inlines {
        render_inlines(inlines, out);
        return;
    }
    let mut copied = 0;
    for callout in callouts {
        out.push_str(&escape(&text[copied..callout.start]));
        let guard = callout.guard.as_deref().map(escape);
        render_conum(
            &callout.number,
            guard.as_deref(),
            callout.xml,
            callout.icons.as_deref(),
            &callout.src,
            out,
        );
        copied = callout.end;
    }
    out.push_str(&escape(&text[copied..]));
}

/// Ruby's `convert_table`: a `<table class="tableblock">` is already the
/// semantic element, so adoctra keeps its markup (frame/grid/stripes classes,
/// a `<caption>`, the column widths in a `<colgroup>`) and styles it in the
/// reference stylesheet.
fn render_table(block: &Block, table: &adoctra_core::ast::Table, cx: &Cx, out: &mut String) {
    use adoctra_core::ast::{CellContent, CellStyle};
    let frame = format!("frame-{}", table.frame);
    let grid = format!("grid-{}", table.grid);
    let stripes = table.stripes.as_ref().map(|s| format!("stripes-{s}"));
    let mut base = vec!["tableblock", frame.as_str(), grid.as_str()];
    base.extend(stripes.as_deref());
    let autowidth = block.attrs.options.iter().any(|o| o == "autowidth")
        || block.attrs.named.contains_key("autowidth-option");
    let mut style = String::new();
    if autowidth && !block.attrs.named.contains_key("width") {
        base.push("fit-content");
    } else if table.width == 100 {
        base.push("stretch");
    } else {
        style = format!(" style=\"width: {}%;\"", table.width);
    }
    base.extend(block.attrs.named.get("float").map(String::as_str));
    out.push_str(&format!(
        "<table{}{}{style}>\n",
        id_attr(&block.id),
        class_attr(&base, &block.roles)
    ));
    if let Some(title) = &block.title {
        out.push_str("<caption class=\"title\">");
        if let Some(caption) = &block.caption {
            out.push_str(&escape(caption));
        }
        render_inlines(title, out);
        out.push_str("</caption>\n");
    }
    if table.cells().next().is_some() {
        out.push_str("<colgroup>\n");
        for column in &table.columns {
            if autowidth || column.autowidth {
                out.push_str("<col>\n");
            } else {
                out.push_str(&format!("<col style=\"width: {}%;\">\n", column.width));
            }
        }
        out.push_str("</colgroup>\n");
    }
    for (section, rows) in [
        ("head", &table.head),
        ("body", &table.body),
        ("foot", &table.foot),
    ] {
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("<t{section}>\n"));
        for row in rows {
            out.push_str("<tr>\n");
            for cell in row {
                let head = section == "head";
                let tag = if head || cell.style == Some(CellStyle::Header) {
                    "th"
                } else {
                    "td"
                };
                out.push_str(&format!(
                    "<{tag} class=\"tableblock halign-{} valign-{}\"",
                    cell.halign.as_str(),
                    cell.valign.as_str()
                ));
                if let Some(colspan) = cell.colspan {
                    out.push_str(&format!(" colspan=\"{colspan}\""));
                }
                if let Some(rowspan) = cell.rowspan {
                    out.push_str(&format!(" rowspan=\"{rowspan}\""));
                }
                if let Some(color) = &cell.background {
                    out.push_str(&format!(
                        " style=\"background-color: {};\"",
                        attr_value(color)
                    ));
                }
                out.push('>');
                match &cell.content {
                    // The nested document's blocks (Ruby's embedded inner
                    // document).
                    CellContent::Blocks { blocks, footnotes } => {
                        out.push_str("<div class=\"content\">");
                        for child in blocks {
                            render_block(child, cx, out);
                        }
                        render_footnotes(footnotes, &cx.doc.attributes, out);
                        out.push_str("</div>");
                    }
                    CellContent::Literal(text) => {
                        out.push_str("<div class=\"literal\"><pre>");
                        out.push_str(&escape(text));
                        out.push_str("</pre></div>");
                    }
                    CellContent::Paragraphs(paragraphs) if head => {
                        for inlines in paragraphs {
                            render_inlines(inlines, out);
                        }
                    }
                    CellContent::Paragraphs(paragraphs) => match cell.style {
                        Some(CellStyle::Literal) => {
                            out.push_str("<div class=\"literal\"><pre>");
                            for inlines in paragraphs {
                                render_inlines(inlines, out);
                            }
                            out.push_str("</pre></div>");
                        }
                        style => {
                            let (open, close) = match style {
                                Some(CellStyle::Strong) => ("<strong>", "</strong>"),
                                Some(CellStyle::Emphasis) => ("<em>", "</em>"),
                                Some(CellStyle::Monospaced) => ("<code>", "</code>"),
                                _ => ("", ""),
                            };
                            for (i, inlines) in paragraphs.iter().enumerate() {
                                if i > 0 {
                                    out.push('\n');
                                }
                                out.push_str("<p class=\"tableblock\">");
                                out.push_str(open);
                                render_inlines(inlines, out);
                                out.push_str(close);
                                out.push_str("</p>");
                            }
                        }
                    },
                }
                out.push_str(&format!("</{tag}>\n"));
            }
            out.push_str("</tr>\n");
        }
        out.push_str(&format!("</t{section}>\n"));
    }
    out.push_str("</table>\n");
}

/// Ruby's `String#to_i`: an optional sign and leading digits, else 0.
fn ruby_to_i(value: &str) -> i64 {
    let value = value.trim_start();
    let (sign, digits) = match value.as_bytes().first() {
        Some(b'-') => (-1, &value[1..]),
        Some(b'+') => (1, &value[1..]),
        _ => (1, value),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    match digits[..end].parse::<i64>() {
        Ok(n) => sign * n,
        Err(_) if end > 0 => sign * i64::MAX,
        Err(_) => 0,
    }
}

fn is_style(block: &Block, style: &str) -> bool {
    block.attrs.style.as_deref() == Some(style)
}

/// A parsed container with no semantic styling of its own (a `--` block, or
/// `[open]` on a paragraph). asciidoctor nests
/// `<div class="openblock"><div class="content">`; adoctra keeps the
/// no-wrapper convention — a single `<div class="openblock">` carries the
/// id/roles, with the `.Title` and children rendered straight inside. A
/// *container* style on a `--` block masquerades it into another kind in the
/// parser; any other style (`partintro`, an unknown one) is a class, as in Ruby.
fn render_open(block: &Block, blocks: &[Block], cx: &Cx, out: &mut String) {
    let style = block
        .attrs
        .style
        .as_deref()
        .filter(|style| *style != "open");
    let base: Vec<&str> = std::iter::once("openblock").chain(style).collect();
    let class = class_attr(&base, &block.roles);
    out.push_str(&format!("<div{class}{}>\n", id_attr(&block.id)));
    render_title(&block.title, &block.caption, out);
    render_body(block, blocks, cx, out);
    out.push_str("</div>\n");
}

/// A container's content: Ruby's simple content (a styled paragraph form)
/// writes the paragraph's inlines without a paragraph element, like Ruby's
/// `content` of a simple block; otherwise the child blocks render in order.
fn render_body(block: &Block, blocks: &[Block], cx: &Cx, out: &mut String) {
    if block.simple {
        if let [Block {
            kind: BlockKind::Paragraph { inlines },
            ..
        }] = blocks
        {
            render_inlines(inlines, out);
            out.push('\n');
            return;
        }
    }
    for child in blocks {
        render_block(child, cx, out);
    }
}

/// A heading element. A discrete (`[discrete]`/`[float]`) heading carries its
/// style as a class, like asciidoctor's floating title; a level-0 heading that
/// is not the document title is a part (`sect0`).
fn render_heading(
    block: &Block,
    level: u8,
    inlines: &[Inline],
    body: bool,
    roles: &[String],
    out: &mut String,
) {
    let h = level.saturating_add(1).min(6);
    let base = match block.attrs.style.as_deref() {
        Some(style @ ("discrete" | "float")) => Some(style),
        _ if body && level == 0 => Some("sect0"),
        _ => None,
    };
    let class = class_attr(base.as_slice(), roles);
    // A heading's caption is its section number (Ruby's `convert_section`).
    render_title(&block.title, &None, out);
    out.push_str(&format!("<h{h}{}{class}>", id_attr(&block.id)));
    let mut title = String::new();
    if let Some(caption) = &block.caption {
        title.push_str(&escape(caption));
    }
    render_inlines(inlines, &mut title);
    // Like Ruby's `convert_section` (not its floating titles or the document
    // title), a section with an id wraps its numbered title in a `sectlinks`
    // link and adds a `sectanchors` anchor before it (after with `after`),
    // reading the attributes at the section.
    if let (true, Some(id)) = (
        body && base != Some("discrete") && base != Some("float"),
        &block.id,
    ) {
        let attributes = &block.attribute_context;
        let href = escape(id);
        if attributes.get("sectlinks").is_some() {
            title = format!("<a class=\"link\" href=\"#{href}\">{title}</a>");
        }
        match attributes.get("sectanchors") {
            Some("after") => title.push_str(&format!("<a class=\"anchor\" href=\"#{href}\"></a>")),
            Some(_) => title.insert_str(0, &format!("<a class=\"anchor\" href=\"#{href}\"></a>")),
            None => {}
        }
    }
    out.push_str(&title);
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
    // A credit split inside an element continues after the `cite`.
    render_inlines(&attr.trailing, out);
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
            // A cross-reference renders as a link to its resolved target
            // (`#id`, or another document's path). The display text (explicit
            // reftext, or the core-generated label) is in `children`; no
            // `class="bare"` (that is a link-only concern). Link constraints
            // only exist on interdocument references.
            Inline::CrossReference { xref, children, .. } => {
                out.push_str("<a href=\"");
                out.push_str(&escape(&xref.target));
                out.push('"');
                for (key, value) in [
                    ("class", &xref.role),
                    ("target", &xref.window),
                    ("rel", &xref.rel),
                ] {
                    if let Some(value) = value {
                        out.push_str(&format!(" {key}=\""));
                        out.push_str(&escape(value));
                        out.push('"');
                    }
                }
                out.push('>');
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
            Inline::Image { image, .. } => render_inline_image(image, out),
            Inline::Footnote { footnote, .. } => render_footnote(footnote, out),
            Inline::Ui { ui, .. } => render_ui(ui, out),
            Inline::IndexTerm { term, .. } => render_index_term(term, out),
            Inline::Stem { stem, .. } => render_stem(stem, out),
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

/// Whether a block has an option (`%name`, `options=name` or `name-option`).
fn has_option(block: &Block, name: &str) -> bool {
    block.attrs.options.iter().any(|o| o == name)
        || block.attrs.named.contains_key(&format!("{name}-option"))
}

/// Ruby's `nowrap` class of a listing or literal `<pre>`: with `%nowrap` or
/// without the (default) `prewrap` attribute at the block.
fn nowrap(block: &Block) -> &'static str {
    if has_option(block, "nowrap") || block.attribute_context.get("prewrap").is_none() {
        "nowrap"
    } else {
        ""
    }
}

/// A ` class="…"` attribute (with a leading space) joining a block's static
/// `base` classes (e.g. `listing`; empty ones are skipped) with its `[.role]` shorthand roles, or the
/// empty string when there are none. Classes are space-separated in order and
/// escaped as an attribute value.
fn class_attr(base: &[&str], roles: &[String]) -> String {
    let classes = base
        .iter()
        .copied()
        .filter(|class| !class.is_empty())
        .chain(roles.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    if classes.is_empty() {
        return String::new();
    }
    format!(" class=\"{}\"", escape(&classes))
}

/// The leading `<figcaption>` of a video or audio figure: its title, never
/// captioned (Ruby numbers only listing/example/figure/table captions).
fn render_media_title(title: &Option<Vec<Inline>>, out: &mut String) {
    if let Some(title) = title {
        out.push_str("<figcaption class=\"title\">");
        render_inlines(title, out);
        out.push_str("</figcaption>\n");
    }
}

/// An escaped attribute value (Ruby's `encode_attribute_value` after special
/// characters).
fn attr_value(s: &str) -> String {
    escape(s).replace('"', "&quot;")
}

/// Ruby's `append_link_constraint_attrs`: `window` becomes the target, with
/// `noopener` for `_blank` (or the option) and `nofollow` from the option.
fn link_constraints(attrs: &adoctra_core::ast::BlockAttrs) -> String {
    let window = attrs.named.get("window").map(|window| attr_value(window));
    constraints(window.as_deref(), &attrs.options)
}

/// The constraint attributes for an already encoded `window`.
fn constraints(window: Option<&str>, options: &[String]) -> String {
    let option = |name: &str| options.iter().any(|o| o == name);
    let rel = option("nofollow").then_some("nofollow");
    match window {
        Some(window) => {
            let mut out = format!(" target=\"{window}\"");
            if window == "_blank" || option("noopener") {
                match rel {
                    Some(rel) => out.push_str(&format!(" rel=\"{rel} noopener\"")),
                    None => out.push_str(" rel=\"noopener\""),
                }
            }
            out
        }
        None => rel.map(|rel| format!(" rel=\"{rel}\"")).unwrap_or_default(),
    }
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

/// Ruby's `convert_inline_image`: a `<span class="image|icon …">` holding an
/// `<img>` (an icon font `<i>` or the `[alt]` text for icons), inside an
/// `<a class="image">` with `link`. Lexical values are inserted verbatim like
/// Ruby's; the alt is its lexical HTML with quotes encoded.
fn render_inline_image(image: &adoctra_core::InlineImage, out: &mut String) {
    let attr = |key: &str| image.attributes.get(key);
    let mut alt = String::new();
    render_parts(&image.alt, &mut alt);
    let inner = match (image.icon, image.icons.as_deref()) {
        (true, Some("font")) => {
            let mut class = format!("fa fa-{}", image.target);
            if let Some(size) = attr("size") {
                class.push_str(&format!(" fa-{size}"));
            }
            if let Some(flip) = attr("flip") {
                class.push_str(&format!(" fa-flip-{flip}"));
            } else if let Some(rotate) = attr("rotate") {
                class.push_str(&format!(" fa-rotate-{rotate}"));
            }
            let title = attr("title")
                .map(|title| format!(" title=\"{title}\""))
                .unwrap_or_default();
            format!("<i class=\"{class}\"{title}></i>")
        }
        (true, None) => format!("[{alt}&#93;"),
        _ => {
            let mut img = format!(
                "<img src=\"{}\" alt=\"{}\"",
                image.src,
                alt.replace('"', "&quot;")
            );
            for key in ["width", "height", "title"] {
                if let Some(value) = attr(key) {
                    img.push_str(&format!(" {key}=\"{value}\""));
                }
            }
            img.push('>');
            img
        }
    };
    out.push_str(&format!("<span class=\"{}\">", image.classes()));
    match attr("link") {
        Some(link) => out.push_str(&format!(
            "<a class=\"image\" href=\"{link}\"{}>{inner}</a>",
            constraints(attr("window").map(String::as_str), &image.options)
        )),
        None => out.push_str(&inner),
    }
    out.push_str("</span>");
}

/// Ruby's `convert_inline_footnote`: the numbered marker linking to the
/// footnote (a definition is also the back-link target), or the unresolved
/// `[id]`. The content renders in the footnotes list.
fn render_footnote(footnote: &adoctra_core::Footnote, out: &mut String) {
    let index = escape(footnote.index.as_deref().unwrap_or_default());
    match footnote.kind {
        adoctra_core::FootnoteKind::Definition => {
            out.push_str("<sup class=\"footnote\"");
            if let Some(id) = &footnote.id {
                out.push_str(&format!(" id=\"_footnote_{}\"", attr_value(id)));
            }
            out.push_str(&format!(">[<a id=\"_footnoteref_{index}\" class=\"footnote\" href=\"#_footnotedef_{index}\" title=\"View footnote.\">{index}</a>]</sup>"));
        }
        adoctra_core::FootnoteKind::Reference => out.push_str(&format!("<sup class=\"footnoteref\">[<a class=\"footnote\" href=\"#_footnotedef_{index}\" title=\"View footnote.\">{index}</a>]</sup>")),
        adoctra_core::FootnoteKind::Unresolved => out.push_str(&format!(
            "<sup class=\"footnoteref red\" title=\"Unresolved footnote reference.\">[{}]</sup>",
            escape(footnote.id.as_deref().unwrap_or_default())
        )),
    }
}

/// Ruby's inline math delimiters. Parts already encode the selected subs;
/// the backend escapes protected text and emits explicitly raw parts as-is.
fn render_stem(stem: &adoctra_core::Stem, out: &mut String) {
    let (open, close) = stem.style.delimiters();
    out.push_str(open);
    render_parts(&stem.parts, out);
    out.push_str(close);
}

/// Ruby's `convert_inline_indexterm`: a flow term shows its text, a
/// concealed term nothing.
fn render_index_term(term: &adoctra_core::IndexTerm, out: &mut String) {
    if let Some(text) = &term.text {
        render_parts(text, out);
    }
}

/// Ruby's `convert_inline_kbd`, `convert_inline_button` and
/// `convert_inline_menu` markup.
fn render_ui(ui: &adoctra_core::UiMacro, out: &mut String) {
    use adoctra_core::UiMacro;
    match ui {
        UiMacro::Keyboard { keys } => {
            if keys.len() > 1 {
                out.push_str("<span class=\"keyseq\">");
            }
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push('+');
                }
                out.push_str("<kbd>");
                render_parts(key, out);
                out.push_str("</kbd>");
            }
            if keys.len() > 1 {
                out.push_str("</span>");
            }
        }
        UiMacro::Button { label } => {
            out.push_str("<b class=\"button\">");
            render_parts(label, out);
            out.push_str("</b>");
        }
        UiMacro::Menu {
            menu,
            submenus,
            item,
            font,
        } => {
            if submenus.is_empty() && item.is_none() {
                out.push_str("<b class=\"menuref\">");
                render_parts(menu, out);
                out.push_str("</b>");
                return;
            }
            let caret = UiMacro::caret(*font);
            out.push_str("<span class=\"menuseq\"><b class=\"menu\">");
            render_parts(menu, out);
            out.push_str("</b>");
            for submenu in submenus {
                out.push_str(caret);
                out.push_str("<b class=\"submenu\">");
                render_parts(submenu, out);
                out.push_str("</b>");
            }
            out.push_str(caret);
            out.push_str("<b class=\"menuitem\">");
            render_parts(item.as_deref().unwrap_or_default(), out);
            out.push_str("</b></span>");
        }
    }
}

/// Ruby's footnotes list after a document's content (unless `nofootnotes`).
fn render_footnotes(
    footnotes: &[adoctra_core::ast::FootnoteDefinition],
    attributes: &adoctra_core::Attributes,
    out: &mut String,
) {
    if footnotes.is_empty() || attributes.get("nofootnotes").is_some() {
        return;
    }
    out.push_str("<div id=\"footnotes\">\n<hr>\n");
    for footnote in footnotes {
        let index = escape(&footnote.index);
        out.push_str(&format!(
            "<div class=\"footnote\" id=\"_footnotedef_{index}\">\n<a href=\"#_footnoteref_{index}\">{index}</a>. "
        ));
        render_parts(&footnote.content, out);
        out.push_str("\n</div>\n");
    }
    out.push_str("</div>\n");
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
            adoctra_core::AttributePart::CloseLink | adoctra_core::AttributePart::CloseXref => {
                out.push_str("</a>")
            }
            // Lexical values keep their entity protection, like link fields.
            adoctra_core::AttributePart::OpenXref(xref) => {
                out.push_str("<a href=\"");
                out.push_str(&xref.target);
                out.push('"');
                for (key, value) in [
                    ("class", &xref.role),
                    ("target", &xref.window),
                    ("rel", &xref.rel),
                ] {
                    if let Some(value) = value {
                        out.push_str(&format!(" {key}=\"{value}\""));
                    }
                }
                out.push('>');
            }
            adoctra_core::AttributePart::LineBreak => out.push_str("<br>"),
            adoctra_core::AttributePart::Image(image) => render_inline_image(image, out),
            adoctra_core::AttributePart::Footnote(footnote) => render_footnote(footnote, out),
            adoctra_core::AttributePart::Ui(ui) => render_ui(ui, out),
            adoctra_core::AttributePart::IndexTerm(term) => render_index_term(term, out),
            adoctra_core::AttributePart::Stem(stem) => render_stem(stem, out),
            adoctra_core::AttributePart::Callout(callout) => render_callout(callout, out),
            adoctra_core::AttributePart::Anchor(id) => {
                out.push_str("<a id=\"");
                out.push_str(id);
                out.push_str("\"></a>");
            }
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
    fn builtin_backend_and_doctype_attributes_resolve() {
        let html = render(
            "= T\n:doctype: book\n\nifdef::backend-html5-doctype-book[{backend}/{doctype}]\nifdef::doctype-article[article]\n",
        );
        assert!(html.contains("<p>html5/book</p>"), "{html}");
        assert!(!html.contains("article"), "{html}");
    }

    #[test]
    fn section_ids_follow_the_id_policy() {
        let html = render(
            "= Doc\n:toc:\n:sectids!:\n\n== First\n\n:sectids:\n:idprefix: s-\n:idseparator:\n\n\
             == Two Words\n\n== Two Words\n\n<<s-twowords2>>\n",
        );
        // Like Ruby, a section without an id links its TOC entry to `#`.
        assert!(html.contains("<li><a href=\"#\">First</a></li>"), "{html}");
        assert!(
            html.contains("<li><a href=\"#s-twowords2\">Two Words</a></li>"),
            "{html}"
        );
        assert!(html.contains("<h2>First</h2>"), "{html}");
        assert!(
            html.contains("<h2 id=\"s-twowords\">Two Words</h2>"),
            "{html}"
        );
        assert!(
            html.contains("<a href=\"#s-twowords2\">Two Words</a>"),
            "{html}"
        );
    }

    #[test]
    fn manual_pages_render_the_name_section_after_the_title() {
        let html = render(
            "= git-foo(1)\n:doctype: manpage\n:toc:\n\n== NAME\n\n\
             git-foo, git-bar - does <b>it</b> & {doctype}\n\n== SYNOPSIS\n\n*git foo*\n",
        );
        assert!(
            html.contains(
                "<body>\n<h1>git-foo(1) Manual Page</h1>\n<section class=\"sect1\">\n\
                 <h2 id=\"_name\">NAME</h2>\n<p>git-foo, git-bar - does <b>it</b> & manpage</p>\n\
                 </section>\n<nav id=\"toc\" class=\"toc\">"
            ),
            "{html}"
        );
        // The TOC lists the body's sections only.
        assert_eq!(html.matches("href=\"#_name\"").count(), 0, "{html}");
        assert!(
            html.contains("<h2 id=\"_synopsis\">SYNOPSIS</h2>"),
            "{html}"
        );
    }

    #[test]
    fn manual_page_name_titles_follow_the_first_section() {
        // Uppercase when the first section's converted title is. Unlike
        // Ruby, markup and character references keep their case.
        let html = render(
            "= x(1)\n:doctype: manpage\n\n[[t]]\n== Name of <<t>> &amp;\n\nx - y\n\n== DESC\n",
        );
        assert!(
            html.contains("<h2 id=\"_name_of_t\">NAME OF <a href=\"#t\">[T]</a> &amp;</h2>"),
            "{html}"
        );
        let html = render("= x(1)\n:doctype: manpage\n\n== Name\n\nx - y\n\n== Desc\n");
        assert!(html.contains("<h2 id=\"_name\">Name</h2>"), "{html}");
        // Header attributes name the page; the definition protected the value.
        let html = render(
            "= x(1)\n:doctype: manpage\n:manname: z\n:manpurpose: <b>p</b>\n:manname-title: Nom\n",
        );
        assert!(
            html.contains("<section class=\"sect1\">\n<h2>Nom</h2>\n<p>z - &lt;b&gt;p&lt;/b&gt;</p>\n</section>"),
            "{html}"
        );
        // Without a title, Ruby's doctitle is the first section's title.
        let html = render(":doctype: manpage\n\n== NAME\n\nx - y\n\n== Synopsis\n");
        assert!(html.contains("<body>\n<h1>Synopsis Manual Page</h1>\n<section class=\"sect1\">\n<h2 id=\"_name\">NAME</h2>"), "{html}");
        // A nonconforming NAME section stays in the body.
        let html = render("= x(1)\n:doctype: manpage\n\n== NAME\n\nx y\n");
        assert!(html.contains("<h1>x(1) Manual Page</h1>\n<section class=\"sect1\">\n<h2 id=\"_name_2\">NAME</h2>\n<p>x y</p>"), "{html}");
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
        assert!(html.contains("<ol class=\"arabic\">"), "got: {html}");
        assert!(html.contains("<li>first</li>"), "got: {html}");
        assert!(html.contains("</ol>"), "got: {html}");
    }

    #[test]
    fn explicit_numbered_list_renders_as_ol() {
        // `1.`/`2.` reuse the ordered-list node, so they render like `.`.
        let html = render("1. first\n2. second");
        assert!(html.contains("<ol class=\"arabic\">"), "got: {html}");
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
        assert!(html.contains("<li>a\n<ol class=\"arabic\">"), "got: {html}");
        assert!(html.contains("<li>b</li>"), "got: {html}");
    }

    #[test]
    fn callouts_render_conums_and_callout_lists() {
        let html = render(
            "[source,ruby]\n----\nx # <1> <2>\ny <!--3-->\n----\n[.r]\n<1> one\n<2> two\n<3> three",
        );
        assert!(html.contains("x # <b class=\"conum\">(1)</b> <b class=\"conum\">(2)</b>\ny &lt;!--<b class=\"conum\">(3)</b>--&gt;</code></pre>"), "{html}");
        assert!(
            html.contains(
                "<ol class=\"colist arabic r\">\n<li>one</li>\n<li>two</li>\n<li>three</li>\n</ol>"
            ),
            "{html}"
        );
        let html = render(":icons: font\n\n----\nx // <1>\n----");
        assert!(
            html.contains("x <i class=\"conum\" data-value=\"1\"></i><b>(1)</b>"),
            "{html}"
        );
        // Like Ruby, the callout list shows the same icons beside its text.
        let html = render(":icons: font\n\n----\nx // <1>\n----\n<1> one\n+\nmore");
        assert!(html.contains("<ol class=\"colist arabic icons\">\n<li><span class=\"colist-icon\"><i class=\"conum\" data-value=\"1\"></i><b>1</b></span><div>one\n<p>more</p>\n</div></li>"), "{html}");
        let html = render(":icons:\n\n....\nx <1>\n....\n<1> one");
        assert!(
            html.contains("x <img src=\"./images/icons/callouts/1.png\" alt=\"1\">"),
            "{html}"
        );
        assert!(html.contains("<li><span class=\"colist-icon\"><img src=\"./images/icons/callouts/1.png\" alt=\"1\"></span><div>one</div></li>"), "{html}");
        // Each block takes the `icons` mode where it converts, like Ruby.
        let html = render("----\na <1>\n----\n<1> one\n\n:icons: font\n\n----\nb <1>\n----\n<1> two\n\n:icons!:\n\n----\nc <1>\n----\n<1> three");
        assert!(html.contains("<pre class=\"listing\">a <b class=\"conum\">(1)</b></pre>\n<ol class=\"colist arabic\">"), "{html}");
        assert!(html.contains("<pre class=\"listing\">b <i class=\"conum\" data-value=\"1\"></i><b>(1)</b></pre>\n<ol class=\"colist arabic icons\">"), "{html}");
        assert!(html.contains("<pre class=\"listing\">c <b class=\"conum\">(1)</b></pre>\n<ol class=\"colist arabic\">\n<li>three</li>"), "{html}");
        // Image icons sit on the middle of the line, like Ruby's `img` rule.
        assert!(STYLESHEET.contains("pre img { vertical-align: middle; }"));
        // As Asciidoctor's stylesheet: a plain mark is not drawn as a badge.
        assert!(STYLESHEET.contains(".conum[data-value]::after { content: attr(data-value); }"));
        assert!(STYLESHEET.contains(".conum"));
    }

    #[test]
    fn block_subs_render_selected_substitutions() {
        let html = render(":v: 1.0 <b>\n\n[subs=none]\n*p* {v} <i>\n\n[subs=quotes]\n*q* {v}\n\n[source,java,subs=\"attributes+\"]\n----\nx = \"{v}\"; // <1>\n----\n<1> one\n\n[subs=\"+quotes\"]\n....\n*l* <!--1-->\n....\n\n[subs=attributes]\n++++\n<u>{v}</u>\n++++\n\n[verse,subs=none]\n____\n*v*\n____\n\n[subs=callouts]\n----\nraw <1>\n----");
        assert!(html.contains("<p>*p* {v} <i></p>"), "{html}");
        assert!(html.contains("<p><strong>q</strong> {v}</p>"), "{html}");
        // Ruby escapes the expanded value again (attributes before specials).
        assert!(html.contains("<code class=\"language-java\" data-lang=\"java\">x = \"1.0 &amp;lt;b&amp;gt;\"; // <b class=\"conum\">(1)</b></code>"), "{html}");
        assert!(html.contains("<pre class=\"literalblock\"><strong>l</strong> &lt;!--<b class=\"conum\">(1)</b>--&gt;</pre>"), "{html}");
        // A passthrough stays unwrapped and raw.
        assert!(html.contains("\n<u>1.0 &lt;b&gt;</u>\n"), "{html}");
        assert!(html.contains("<pre class=\"verse\">*v*</pre>"), "{html}");
        // Callouts before special characters leave the raw mark.
        assert!(
            html.contains("<pre class=\"listing\">raw <1></pre>"),
            "{html}"
        );
        let html = render(":icons: font\n\n[subs=+quotes]\n----\n*x* // <1>\n----");
        assert!(
            html.contains("<strong>x</strong> <i class=\"conum\" data-value=\"1\"></i><b>(1)</b>"),
            "{html}"
        );
        let html = render(":icons:\n:iconsdir: i\n\n[subs=+quotes]\n----\n# <1>\n----");
        assert!(
            html.contains("<pre class=\"listing\"><img src=\"i/callouts/1.png\" alt=\"1\"></pre>"),
            "{html}"
        );
    }

    #[test]
    fn ordered_styles_checklists_and_description_lists() {
        let html = render(". a\n.. b\n\nc. x\nd. y\n\n[%reversed,start=3]\n. z");
        assert!(
            html.contains("<ol class=\"loweralpha\" type=\"a\">"),
            "{html}"
        );
        // A lettered marker gives Ruby's style without the `type` keyword.
        assert!(
            html.contains("<ol class=\"loweralpha\">\n<li>x</li>"),
            "{html}"
        );
        assert!(
            html.contains("<ol class=\"arabic\" start=\"3\" reversed>"),
            "{html}"
        );
        let html = render("* [x] done\n* [ ] todo\n* plain");
        assert!(html.contains("<ul class=\"checklist\">\n<li>&#10003; done</li>\n<li>&#10063; todo</li>\n<li>plain</li>"), "{html}");
        let html = render("[#d.r]\nCPU:: The *brain*\nA::\nB:: shared\n+\n----\nx\n----");
        assert!(html.contains("<dl id=\"d\" class=\"r\">\n<dt class=\"hdlist1\">CPU</dt>\n<dd>\n<p>The <strong>brain</strong></p>\n</dd>\n<dt class=\"hdlist1\">A</dt>\n<dt class=\"hdlist1\">B</dt>\n<dd>\n<p>shared</p>\n<pre class=\"listing\">x</pre>\n</dd>\n</dl>"), "{html}");
        let html = render("[horizontal,labelwidth=25]\nA:: one");
        assert!(html.contains("<dl class=\"hdlist\" style=\"grid-template-columns: 25% 1fr;\">\n<dt>A</dt>\n<dd>\n<p>one</p>\n</dd>\n</dl>"), "{html}");
        let html = render("[qanda]\nWhy?:: Because");
        assert!(
            html.contains(
                "<ol class=\"qanda\">\n<li>\n<p><em>Why?</em></p>\n<p>Because</p>\n</li>\n</ol>"
            ),
            "{html}"
        );
        assert!(STYLESHEET.contains("dl.hdlist"));
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
    fn shorthand_role_on_section_renders_class_on_section() {
        // Like Ruby's `<div class="sect1 notice">`: roles on the section, the id
        // on its heading.
        let html = render("[.notice]\n== Hello World\n\nbody");
        assert!(
            html.contains(
                "<section class=\"sect1 notice\">\n<h2 id=\"_hello_world\">Hello World</h2>\n<p>body</p>\n</section>"
            ),
            "got: {html}"
        );
        // A part keeps them on its `sect0` heading, without a wrapper.
        let html = render("Intro\n\n[.notice]\n= Part\n\n== Chapter");
        assert!(
            html.contains(
                "<h1 id=\"_part\" class=\"sect0 notice\">Part</h1>\n<section class=\"sect1\">"
            ),
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
    fn section_anchors_and_links_wrap_section_titles_like_ruby() {
        let html = render(
            "= Doc\n:sectanchors:\n:sectlinks:\n:sectnums:\n:showtitle:\n\n== One *B*\n\n\
             [discrete]\n== Float\n\n:sectanchors: after\n\n== Two\n\n:sectlinks!:\n\n\
             [[x]]\n== Three\n\n:sectids!:\n\n== Four\n",
        );
        assert!(html.contains("<h1>Doc</h1>"), "{html}");
        assert!(
            html.contains("<h2 id=\"_one_b\"><a class=\"anchor\" href=\"#_one_b\"></a><a class=\"link\" href=\"#_one_b\">1. One <strong>B</strong></a></h2>"),
            "{html}"
        );
        assert!(
            html.contains("<h2 id=\"_float\" class=\"discrete\">Float</h2>"),
            "{html}"
        );
        assert!(
            html.contains("<h2 id=\"_two\"><a class=\"link\" href=\"#_two\">2. Two</a><a class=\"anchor\" href=\"#_two\"></a></h2>"),
            "{html}"
        );
        assert!(
            html.contains("<h2 id=\"x\">3. Three<a class=\"anchor\" href=\"#x\"></a></h2>"),
            "{html}"
        );
        assert!(html.contains("<h2>4. Four</h2>"), "{html}");
    }

    #[test]
    fn section_anchors_apply_to_book_parts_and_cell_documents() {
        let html = render(
            "= Book\n:doctype: book\n:sectanchors:\n\n= Part\n\n== Chap\n\n|===\na|\n:sectanchors!:\n\n== Cell\n|===\n",
        );
        assert!(
            html.contains("<h1 id=\"_part\" class=\"sect0\"><a class=\"anchor\" href=\"#_part\"></a>Part</h1>"),
            "{html}"
        );
        // The cell's document inherits the locked attribute, like Ruby.
        assert!(
            html.contains("<h2 id=\"_cell\"><a class=\"anchor\" href=\"#_cell\"></a>Cell</h2>"),
            "{html}"
        );
    }

    #[test]
    fn collapsible_example_renders_details_like_ruby() {
        let html = render(
            ".Show *me*\n[#d.r%collapsible%open]\n====\nBody\n====\n\n\
             [%collapsible]\n====\nHidden\n====\n\n.Plain\n====\nEx\n====\n",
        );
        assert!(
            html.contains("<details id=\"d\" class=\"r\" open>\n<summary class=\"title\">Show <strong>me</strong></summary>\n<p>Body</p>\n</details>\n"),
            "{html}"
        );
        assert!(
            html.contains("<details>\n<summary class=\"title\">Details</summary>\n<p>Hidden</p>\n</details>\n"),
            "{html}"
        );
        // The collapsible examples took no number.
        assert!(
            html.contains(
                "<div class=\"exampleblock\">\n<div class=\"title\">Example 1. Plain</div>"
            ),
            "{html}"
        );
        // A masqueraded open block and named options collapse too.
        let html = render("[example%collapsible]\n--\nx\n--\n\n[open-option=,collapsible-option=]\n====\ny\n====\n");
        assert!(
            html.contains("<details>\n<summary class=\"title\">Details</summary>\n<p>x</p>"),
            "{html}"
        );
        assert!(
            html.contains("<details open>\n<summary class=\"title\">Details</summary>\n<p>y</p>"),
            "{html}"
        );
    }

    #[test]
    fn markdown_and_quoted_paragraph_blockquotes_render_like_ruby() {
        let html = render("> a *b*\n>\n> * item\n> -- *A*, W & X\n\n\"q\"\n-- B\n");
        assert!(
            html.contains(
                "<figure class=\"quoteblock\">\n<blockquote>\n<p>a <strong>b</strong></p>\n<ul>\n<li>item</li>\n</ul>\n</blockquote>\n\
                 <figcaption>&#8212; <strong>A</strong><br>\n<cite>W &amp; X</cite></figcaption>\n</figure>"
            ),
            "{html}"
        );
        // A quoted paragraph is simple content.
        assert!(
            html.contains("<blockquote>\nq\n</blockquote>\n<figcaption>&#8212; B</figcaption>"),
            "{html}"
        );
    }

    #[test]
    fn credit_split_inside_formatting_continues_after_the_cite() {
        let html = render("> a\n> -- *A, B* rest\n");
        assert!(
            html.contains(
                "<figcaption>&#8212; <strong>A</strong><br>\n<cite><strong>B</strong></cite> rest</figcaption>"
            ),
            "{html}"
        );
    }

    #[test]
    fn styled_paragraph_forms_render_simple_content_like_ruby() {
        let html = render(
            "[example%collapsible]\n.T\ncollapsed\n\n[quote,A]\nquoted *q*\n\n\
             [example%hardbreaks]\none\ntwo\n\n====\ndelimited\n====\n",
        );
        assert!(
            html.contains("<details>\n<summary class=\"title\">T</summary>\ncollapsed\n</details>"),
            "{html}"
        );
        assert!(
            html.contains("<blockquote>\nquoted <strong>q</strong>\n</blockquote>"),
            "{html}"
        );
        assert!(
            html.contains("<div class=\"exampleblock\">\none<br> two\n</div>"),
            "{html}"
        );
        // A delimited example keeps its paragraph.
        assert!(
            html.contains("<div class=\"exampleblock\">\n<p>delimited</p>\n</div>"),
            "{html}"
        );
        // Ruby's book partintro: a `[partintro]` paragraph is compound, a lone
        // `[open]` paragraph stays simple.
        let html = render(
            "= B\n:doctype: book\n\n= A\n\n[partintro]\nintro\n\n== C\n\nx\n\n\
             = P\n\n[open]\nonly\n\n== D\n\ny\n",
        );
        assert!(
            html.contains("<div class=\"openblock partintro\">\n<p>intro</p>\n</div>"),
            "{html}"
        );
        assert!(
            html.contains("<div class=\"openblock partintro\">\nonly\n</div>"),
            "{html}"
        );
    }

    #[test]
    fn verbatim_blocks_get_nowrap_from_the_option_or_unset_prewrap() {
        let html = render(
            "[%nowrap]\n----\na\n----\n\n[source%nowrap,ruby]\n----\nb\n----\n\n\
             [literal%nowrap.r]\nc\n\n----\nwraps\n----\n",
        );
        assert!(
            html.contains("<pre class=\"listing nowrap\">a</pre>"),
            "{html}"
        );
        assert!(
            html.contains("<pre class=\"highlight nowrap\"><code class=\"language-ruby\" data-lang=\"ruby\">b</code></pre>"),
            "{html}"
        );
        assert!(
            html.contains("<pre class=\"literalblock nowrap r\">c</pre>"),
            "{html}"
        );
        assert!(
            html.contains("<pre class=\"listing\">wraps</pre>"),
            "{html}"
        );
        // `prewrap` is read at each block, like Ruby's replayed attributes.
        let html = render(
            ":prewrap!:\n\n----\na\n----\n\n [source]\n\n....\nb\n....\n\n:prewrap:\n\n----\nc\n----\n",
        );
        assert!(
            html.contains("<pre class=\"listing nowrap\">a</pre>"),
            "{html}"
        );
        assert!(
            html.contains("<pre class=\"literalblock nowrap\">[source]</pre>"),
            "{html}"
        );
        assert!(
            html.contains("<pre class=\"literalblock nowrap\">b</pre>"),
            "{html}"
        );
        assert!(html.contains("<pre class=\"listing\">c</pre>"), "{html}");
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
        // Ruby's simple content: the paragraph text without a `<p>`.
        assert!(
            html.contains("</p>\nThis is a <strong>note</strong>.\n</aside>"),
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
            html.contains("</p>\nThis is a <strong>note</strong>.\n</aside>"),
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
            html.contains("<div class=\"exampleblock\">\nJust text.\n</div>"),
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
            html.contains("<aside class=\"sidebarblock\">\nJust text.\n</aside>"),
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
            html.contains("<div class=\"openblock\">\nJust text.\n</div>"),
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

    #[test]
    fn late_anchors_render_from_nodes_and_parts_and_drop_from_borrowed_labels() {
        let html = render(":attribute-missing: drop\n:a: [[q]]x\n\nA [[b,{nope}]] z\n\nQ {a}\n\nC [[c,{nope}]] {a}\n\n[[t]]\n.T {a}\n====\nB\n====\n\n<<t>>");
        assert!(html.contains("<p>A [[b,]] z</p>"), "{html}");
        assert!(html.contains("<p>Q <a id=\"q\"></a>x</p>"), "{html}");
        // As in Ruby, the lazy reftext reaches into the expanded value.
        assert!(html.contains("<p>C <a id=\"c\"></a>x</p>"), "{html}");
        assert!(html.contains("T <a id=\"q\"></a>x</div>"), "{html}");
        assert!(html.contains(r##"<a href="#t">T x</a>"##), "{html}");
    }

    #[test]
    fn late_xrefs_render_from_nodes_and_parts_with_roles() {
        let html = render(":id: sec\n:r: <<sec>>\n\n[[sec]]\n== S&T\n\nA <<{id}>> {r} xref:sec[x,role=r&s] \\<<sec>> <<sec,*a>> b*\n\n.T {r}\n====\nB\n====");
        assert!(
            html.contains(r##"<p>A <a href="#sec">S&amp;T</a> <a href="#sec">S&amp;T</a> <a href="#sec" class="r&amp;s">x</a> &lt;&lt;sec&gt;&gt; <a href="#sec"><strong>a</a> b</strong></p>"##),
            "{html}"
        );
        assert!(
            html.contains(r##"T <a href="#sec">S&amp;T</a></div>"##),
            "{html}"
        );
        // A value-generated reference to a missing id shows the fallback.
        let html = render(":r: xref:nope[]\n\n{r}");
        assert!(
            html.contains(r##"<p><a href="#nope">[nope]</a></p>"##),
            "{html}"
        );
    }

    #[test]
    fn xrefstyle_labels_render_captions_quotes_and_chapters() {
        let html = render(":xrefstyle: full\n\n[[ex]]\n.My *Ex*\n====\nx\n====\n\nA <<ex>> B xref:ex[xrefstyle=short]");
        assert!(
            html.contains(r##"<p>A <a href="#ex">Example 1, &#8220;My <strong>Ex</strong>&#8221;</a> B <a href="#ex">Example 1</a></p>"##),
            "{html}"
        );
        let html = render("= B\n:doctype: book\n:xrefstyle: basic\n\n[[c]]\n== Chap\n\n<<c>>");
        assert!(
            html.contains(r##"<a href="#c"><em>Chap</em></a>"##),
            "{html}"
        );
    }

    #[test]
    fn passthrough_macros_render_cross_references() {
        let html = render("[[sec]]\n== S&T\n\nA pass:c,m[<<sec>>] B pass:c,q,m[<<sec,*b*>>] C pass:c,m[<<other.adoc#f>>]");
        assert!(
            html.contains(r##"<p>A <a href="#sec">S&amp;T</a> B <a href="#sec"><strong>b</strong></a> C <a href="other.html#f">other.html</a></p>"##),
            "{html}"
        );
    }

    #[test]
    fn definition_cross_references_render_labels_fixed_at_the_definition() {
        let html = render(
            ":r: pass:c,m[<<sec>>]\n\n[[sec]]\n== S&T\n\n:s: pass:c,m[<<sec>>]\n\nA {r} B {s}",
        );
        assert!(
            html.contains(r##"<p>A <a href="#sec">[sec]</a> B <a href="#sec">S&amp;T</a></p>"##),
            "{html}"
        );
    }

    #[test]
    fn numbered_sections_render_their_number_inside_the_heading() {
        let html = render(":sectnums:\n:sectnumlevels: 2\n\n== A&B\n\n=== Sub\n\n==== Deep\n\nSee xref:_sub[xrefstyle=full].");
        assert!(html.contains(r#"<h2 id="_ab">1. A&amp;B</h2>"#), "{html}");
        assert!(html.contains(r#"<h3 id="_sub">1.1. Sub</h3>"#), "{html}");
        assert!(html.contains(r#"<h4 id="_deep">Deep</h4>"#), "{html}");
        assert!(
            html.contains(r##"<a href="#_sub">Section 1.1, &#8220;Sub&#8221;</a>"##),
            "{html}"
        );
        let html = render("= B\n:doctype: book\n:partnums:\n:sectnums:\n\n= P\n\n== C");
        assert!(
            html.contains(r#"<h1 id="_p" class="sect0">I: P</h1>"#),
            "{html}"
        );
        assert!(html.contains(r#"<h2 id="_c">1. C</h2>"#), "{html}");
    }

    fn body(html: &str) -> &str {
        html.split("<body>\n").nth(1).unwrap()
    }

    #[test]
    fn breaks_render_like_ruby_without_metadata() {
        let html = render("[#hr.r]\n.T\n'''\n\n<<<\n\n* * *");
        assert_eq!(
            body(&html),
            "<hr>\n<div style=\"page-break-after: always;\"></div>\n<hr>\n</body>\n</html>\n"
        );
    }

    #[test]
    fn verbatim_paragraphs_and_source_defaults_render_like_ruby() {
        let html = render(
            ":source-language: ruby\n:tabsize: 4\n\n----\n\ta <b>\n----\n\n[listing]\n*plain*\n\n[verse,indent=1]\n   *v*",
        );
        assert!(
            html.contains(r#"<pre class="highlight"><code class="language-ruby" data-lang="ruby">    a &lt;b&gt;</code></pre>"#),
            "{html}"
        );
        assert!(
            html.contains("<pre class=\"listing\">*plain*</pre>"),
            "{html}"
        );
        assert!(
            html.contains("<pre class=\"verse\"> <strong>v</strong></pre>"),
            "{html}"
        );
    }

    #[test]
    fn fenced_code_renders_as_a_source_listing() {
        let html = render("```ruby\nputs 1 <1>\n```\n<1> x\n\n```\n<b>\n```");
        assert!(
            html.contains(r#"<pre class="highlight"><code class="language-ruby" data-lang="ruby">puts 1 <b class="conum">(1)</b></code></pre>"#),
            "{html}"
        );
        assert!(
            html.contains("<pre class=\"highlight\"><code>&lt;b&gt;</code></pre>"),
            "{html}"
        );
    }

    #[test]
    fn automatic_toc_follows_the_title_and_nests_to_toclevels() {
        let html = render("= Doc\n:toc:\n\nPre\n\n== A *b* [[x]]\n\n=== A1\n\n==== A11\n\n== B link:http://x[L] anchor:q[]");
        assert!(
            body(&html).starts_with(
                "<h1>Doc</h1>\n<nav id=\"toc\" class=\"toc\">\n<div id=\"toctitle\">Table of Contents</div>\n<ul class=\"sectlevel1\">\n<li><a href=\"#x\">A <strong>b</strong></a>\n<ul class=\"sectlevel2\">\n<li><a href=\"#_a1\">A1</a></li>\n</ul>\n</li>\n<li><a href=\"#_b_l\">B L </a></li>\n</ul>\n</nav>\n<section id=\"preamble\">"
            ),
            "{html}"
        );
        // Without a title the TOC comes first; numbers and appendix captions
        // precede the titles, and toclevels deepens the outline.
        let html = render(":toc: left\n:toclevels: 3\n:sectnums:\n:toc-title: Содержание & co\n\n== A\n\n=== A1\n\n==== A11\n\n[appendix]\n== App");
        assert!(
            body(&html).starts_with("<nav id=\"toc\" class=\"toc\">\n<div id=\"toctitle\">Содержание &amp; co</div>\n<ul class=\"sectlevel1\">\n<li><a href=\"#_a\">1. A</a>\n<ul class=\"sectlevel2\">\n<li><a href=\"#_a1\">1.1. A1</a>\n<ul class=\"sectlevel3\">\n<li><a href=\"#_a11\">1.1.1. A11</a></li>"),
            "{html}"
        );
        assert!(
            html.contains("<li><a href=\"#_app\">Appendix A: App</a></li>"),
            "{html}"
        );
        // A book outline starts at its parts.
        let html = render("= B\n:doctype: book\n:toc:\n:toclevels: 1\n\n= P\n\n== C\n\n=== S");
        assert!(
            html.contains("<ul class=\"sectlevel0\">\n<li><a href=\"#_p\">P</a>\n<ul class=\"sectlevel1\">\n<li><a href=\"#_c\">C</a></li>\n</ul>\n</li>\n</ul>"),
            "{html}"
        );
        // No sections, no TOC; a body `:toc:` does not enable it.
        assert!(!body(&render("= D\n:toc:\n\ntext")).contains("<nav"));
        assert!(!body(&render("= D\n\n:toc:\n\n== A")).contains("<nav"));
    }

    #[test]
    fn preamble_toc_closes_the_preamble() {
        let html = render("= D\n:toc: preamble\n:toc-class: side\n\nPre\n\n== A");
        assert!(
            html.contains("<section id=\"preamble\">\n<p>Pre</p>\n<nav id=\"toc\" class=\"side\">\n<div id=\"toctitle\">Table of Contents</div>\n<ul class=\"sectlevel1\">\n<li><a href=\"#_a\">A</a></li>\n</ul>\n</nav>\n</section>"),
            "{html}"
        );
        // Without a preamble there is no TOC at all.
        assert!(!body(&render("= D\n:toc: preamble\n\n== A")).contains("<nav"));
    }

    #[test]
    fn toc_macro_renders_at_its_position_only_with_macro_placement() {
        let html = render("= D\n:toc: macro\n:toc-class: tc\n\n====\n.Contents *x*\ntoc::[levels=1]\n====\n\n[[t2]]\ntoc::[role=side,title=Second]\n\n== A\n\n=== B");
        assert!(
            html.contains("<div class=\"exampleblock\">\n<nav id=\"toc\" class=\"tc\">\n<div id=\"toctitle\" class=\"title\">Contents <strong>x</strong></div>\n<ul class=\"sectlevel1\">\n<li><a href=\"#_a\">A</a></li>\n</ul>\n</nav>\n</div>"),
            "{html}"
        );
        assert!(
            html.contains("<nav id=\"t2\" class=\"side\">\n<div id=\"t2title\" class=\"title\">Second</div>\n<ul class=\"sectlevel1\">\n<li><a href=\"#_a\">A</a>\n<ul class=\"sectlevel2\">"),
            "{html}"
        );
        // Any other placement disables the macro (and keeps the automatic TOC).
        let html = render("= D\n:toc:\n\ntoc::[]\n\n== A");
        assert_eq!(body(&html).matches("<nav").count(), 1, "{html}");
        assert!(html.contains("<!-- toc disabled -->"), "{html}");
        assert!(render("= D\n\ntoc::[]\n\n== A").contains("<!-- toc disabled -->"));
        assert!(render("= D\n:toc: macro\n\ntoc::[]").contains("<!-- toc disabled -->"));
    }

    #[test]
    fn toc_helpers_follow_ruby() {
        assert_eq!(
            drop_anchors(
                "<a id=\"x\"></a>A <a href=\"#y\" class=\"bare\">y</a> <abbr>z</abbr> <a>w</a> <b"
            ),
            "A y <abbr>z</abbr> w <b"
        );
        for (value, expected) in [
            ("3", 3),
            (" 2x", 2),
            ("-1", -1),
            ("+4", 4),
            ("x", 0),
            ("", 0),
        ] {
            assert_eq!(ruby_to_i(value), expected, "{value}");
        }
        assert_eq!(ruby_to_i("99999999999999999999"), i64::MAX);
    }

    #[test]
    fn book_partintro_renders_only_inside_a_part() {
        let html = render("= B\n:doctype: book\n\n= P\n\nIntro *a*.\n\nMore.\n\n== C\n\n[partintro]\n--\nhidden\n--");
        assert!(
            html.contains("<h1 id=\"_p\" class=\"sect0\">P</h1>\n<div class=\"openblock partintro\">\n<p>Intro <strong>a</strong>.</p>\n<p>More.</p>\n</div>\n<section class=\"sect1\">"),
            "{html}"
        );
        // A partintro outside a part is excluded, like Ruby's converter.
        assert!(
            !html.split("<body>").nth(1).unwrap().contains("hidden"),
            "{html}"
        );
        // Outside a book there is no partintro container at all.
        let html = render("= A\n\n= P\n\n[partintro]\nhidden\n\n== C");
        let body = html.split("<body>").nth(1).unwrap();
        assert!(
            !body.contains("hidden") && !body.contains("partintro"),
            "{html}"
        );
    }

    #[test]
    fn abstract_and_styled_open_blocks_render_like_ruby() {
        let html =
            render("= A\n\n[abstract.r#a]\n.T\nAbs *b*.\n\n[foo]\n--\nx\n--\n\n[open]\n--\ny\n--");
        assert!(
            html.contains("<figure class=\"quoteblock abstract r\" id=\"a\">\n<div class=\"title\">T</div>\n<blockquote>\nAbs <strong>b</strong>.\n</blockquote>\n</figure>"),
            "{html}"
        );
        // Any style that does not masquerade becomes a class; `open` does not.
        assert!(
            html.contains("<div class=\"openblock foo\">\n<p>x</p>"),
            "{html}"
        );
        assert!(
            html.contains("<div class=\"openblock\">\n<p>y</p>"),
            "{html}"
        );
        // A headerless book starting with an abstract has no preamble, so
        // every abstract directly in the document is excluded...
        let html = render(":doctype: book\n\n[abstract]\nhidden\n\nx\n\n[abstract]\nhidden");
        assert!(
            !html.split("<body>").nth(1).unwrap().contains("hidden") && html.contains("<p>x</p>"),
            "{html}"
        );
        // ...while one inside the preamble renders.
        let html = render(":doctype: book\n\nx\n\n[abstract]\nshown");
        assert!(
            html.contains(
                "<section id=\"preamble\">\n<p>x</p>\n<figure class=\"quoteblock abstract\">"
            ),
            "{html}"
        );
    }

    #[test]
    fn preface_title_is_not_rendered() {
        let html = render("= B\n:doctype: book\n:preface-title: Fore\n\nPre\n\n== C");
        assert!(!html.contains("Fore"), "{html}");
        assert!(
            html.contains("<section id=\"preamble\">\n<p>Pre</p>\n</section>"),
            "{html}"
        );
    }

    #[test]
    fn appendix_sections_render_localized_captions() {
        let html = render("= Doc\n:appendix-caption: Приложение\n:appendix-refsig: Прил.\n\n[appendix]\n= A&B\n\n=== Sub\n\nSee xref:_ab[xrefstyle=full].");
        assert!(
            html.contains("<section class=\"sect1\">\n<h2 id=\"_ab\">Приложение A: A&amp;B</h2>"),
            "{html}"
        );
        assert!(html.contains(r#"<h3 id="_sub">Sub</h3>"#), "{html}");
        assert!(
            html.contains(r##"<a href="#_ab">Прил. A, <em>A&amp;B</em></a>"##),
            "{html}"
        );
    }

    #[test]
    fn interdocument_and_natural_xrefs_render_resolved_targets() {
        let html = render(
            ":p: xref:o&p.adoc#v[V,window=_blank]\n:n: <<Sec Title>>\n\n[[sec]]\n== Sec Title\n\nA <<other.adoc#f>> B xref:other.adoc#f[T,role=r,window=_blank,opts=nofollow] C {p} D <<Sec Title>> E {n} F <<#>>",
        );
        assert!(
            html.contains(concat!(
                r##"<p>A <a href="other.html#f">other.html</a> "##,
                r##"B <a href="other.html#f" class="r" target="_blank" rel="nofollow noopener">T</a> "##,
                r##"C <a href="o&amp;p.html#v" target="_blank" rel="noopener">V</a> "##,
                r##"D <a href="#sec">Sec Title</a> E <a href="#sec">Sec Title</a> "##,
                r##"F <a href="#">Sec Title</a></p>"##
            )),
            "{html}"
        );
    }

    #[test]
    fn built_in_roles_follow_asciidoctor_stylesheet() {
        // Ruby's text roles with its colours, defined after the components.
        let roles = STYLESHEET
            .split("--- Built-in roles")
            .nth(1)
            .expect("roles section");
        for rule in [
            ".big { font-size: larger; }",
            ".small { font-size: smaller; }",
            ".underline { text-decoration: underline; }",
            ".overline { text-decoration: overline; }",
            ".line-through { text-decoration: line-through; }",
            ".red { color: #bf0000; }",
            ".red-background { background: #fa0000; }",
            ".green { color: #006000; }",
            ".yellow-background { background: #fafa00; }",
            ".white { color: #bfbfbf; }",
            ".black-background { background: #000; }",
        ] {
            assert!(roles.contains(rule), "{rule}");
        }
        assert_eq!(roles.matches("-background { background:").count(), 16);
        // Content that sets its own colour in Ruby keeps it under a block role.
        assert!(roles.contains("pre:is(.listing, .literalblock, .highlight) {"));
        assert!(roles.contains("aside.admonition { color: var(--adoctra-fg); }"));
        assert!(!STYLESHEET.contains("sup.footnoteref.red"));
        // Roles reach the elements: inline spans, blocks and the unresolved
        // footnote reference.
        let html = render("x [.red]#a# [.white.navy-background]#b# footnote:x[]\n\n[.big.underline]\nc\n\n[.maroon]\n----\nd\n----");
        assert!(
            html.contains(
                "<span class=\"red\">a</span> <span class=\"white navy-background\">b</span>"
            ),
            "{html}"
        );
        assert!(html.contains("<sup class=\"footnoteref red\""), "{html}");
        assert!(html.contains("<p class=\"big underline\">c</p>"), "{html}");
        assert!(
            html.contains("<pre class=\"listing maroon\">d</pre>"),
            "{html}"
        );
    }

    #[test]
    fn font_icons_link_the_icon_font_stylesheet_like_ruby() {
        let link = |src: &str| {
            render(src)
                .lines()
                .filter(|l| l.starts_with("<link rel=\"stylesheet\""))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let cdn = "//cdnjs.cloudflare.com/ajax/libs/font-awesome/4.7.0/css/font-awesome.min.css";
        assert_eq!(
            link(":icons: font\n\nx"),
            [format!("<link rel=\"stylesheet\" href=\"https:{cdn}\">")]
        );
        assert_eq!(
            link(":icons: font\n:asset-uri-scheme:\n\nx"),
            [format!("<link rel=\"stylesheet\" href=\"{cdn}\">")]
        );
        assert_eq!(
            link(":icons: font\n:iconfont-cdn: //c/fa.css?a&b\n\nx"),
            ["<link rel=\"stylesheet\" href=\"//c/fa.css?a&amp;b\">"]
        );
        assert_eq!(
            link(":icons: font\n:iconfont-remote!:\n\nx"),
            ["<link rel=\"stylesheet\" href=\"./font-awesome.css\">"]
        );
        assert_eq!(
            link(
                ":icons: font\n:iconfont-remote!:\n:stylesdir: css/a b\n:iconfont-name: fa 5\n\nx"
            ),
            ["<link rel=\"stylesheet\" href=\"css/a%20b/fa%205.css\">"]
        );
        // The head reads the header: image icons, a body assignment and an
        // unset mode link nothing.
        for src in [
            ":icons:\n\nx",
            "x\n\n:icons: font",
            ":icons: font\n:icons!:\n\nx",
        ] {
            assert!(link(src).is_empty(), "{src}");
        }
        // The icon font renders `icon:` glyphs.
        let html = render(":icons: font\n\nicon:heart[2x]");
        assert!(
            html.contains("font-awesome.min.css\">\n</head>")
                || html.contains("font-awesome.min.css\">\n<"),
            "{html}"
        );
        assert!(
            html.contains("<i class=\"fa fa-heart fa-2x\"></i>"),
            "{html}"
        );
    }

    #[test]
    fn inline_images_and_icons_render_ruby_spans() {
        let html = render(
            ":imagesdir: img\n:img: image:v.png[V \"q\"]\n\n\
             A image:a b.png[Alt *b* & (C),20,title=T,role=r,float=left,link=l,window=_blank,opts=nofollow] \
             icon:heart[2x] {img}\n\n:icons: font\n\nicon:heart[2x,flip=h,title=L]\n\n\
             :icons:\n\nicon:x[icon=y,width=5]",
        );
        assert_eq!(
            body(&html),
            "<p>A <span class=\"image left r\"><a class=\"image\" href=\"l\" target=\"_blank\" rel=\"nofollow noopener\"><img src=\"img/a%20b.png\" alt=\"Alt <strong>b</strong> &amp; &#169;\" width=\"20\" title=\"T\"></a></span> \
             <span class=\"icon\">[heart&#93;</span> <span class=\"image\"><img src=\"img/v.png\" alt=\"V &quot;q&quot;\"></span></p>\n\
             <p><span class=\"icon\"><i class=\"fa fa-heart fa-2x fa-flip-h\" title=\"L\"></i></span></p>\n\
             <p><span class=\"icon\"><img src=\"./images/icons/y.png\" alt=\"x\" width=\"5\"></span></p>\n</body>\n</html>\n"
        );
    }

    #[test]
    fn video_and_audio_render_figures_with_leading_titles_and_players() {
        let html = render(
            ":imagesdir: m\n\n.A *clip*\n[[v]]\nvideo::a b.mp4[p.png,640,360,start=5,opts=\"autoplay,muted,loop\",preload=none,float=left,align=center,role=r]\n\n\
             video::id[youtube,opts=\"nofullscreen\",theme=<&>]\n\nvideo::7[vimeo]\n\n\
             .Theme\naudio::s.ogg[end=3,opts=\"autoplay,nocontrols\",role=q]\n\naudio::t.ogg[]",
        );
        assert_eq!(
            body(&html),
            "<figure id=\"v\" class=\"videoblock left text-center r\">\n<figcaption class=\"title\">A <strong>clip</strong></figcaption>\n<video src=\"m/a%20b.mp4#t=5\" width=\"640\" height=\"360\" poster=\"m/p.png\" autoplay muted controls loop preload=\"none\">\nYour browser does not support the video tag.\n</video>\n</figure>\n\
             <figure class=\"videoblock\">\n<iframe src=\"https://www.youtube.com/embed/id?rel=0&amp;fs=0&amp;theme=&lt;&amp;&gt;\"></iframe>\n</figure>\n\
             <figure class=\"videoblock\">\n<iframe src=\"https://player.vimeo.com/video/7\" allowfullscreen></iframe>\n</figure>\n\
             <figure class=\"audioblock q\">\n<figcaption class=\"title\">Theme</figcaption>\n<audio src=\"m/s.ogg#t=,3\" autoplay>\nYour browser does not support the audio tag.\n</audio>\n</figure>\n\
             <figure class=\"audioblock\">\n<audio src=\"m/t.ogg\" controls>\nYour browser does not support the audio tag.\n</audio>\n</figure>\n</body>\n</html>\n"
        );
    }

    #[test]
    fn stem_macros_emit_math_delimiters_and_selected_content() {
        let html = render(
            r"A stem:[x < y & {a} *b*] latexmath:[$\sqrt{4}$] stem:q[*x*] stem:none[<b>raw</b>]",
        );
        assert!(html.contains(r"<p>A \$x &lt; y &amp; {a} *b*\$ \(\sqrt{4}\) \$<strong>x</strong>\$ \$<b>raw</b>\$</p>"), "{}", body(&html));
        let html = render("[[t]]Target.\n\nstem:c,m[<<t>>] stem:m[footnote:[Note\\]]");
        assert!(
            html.contains(r##"\$<a href="#t">[t]</a>\$"##),
            "{}",
            body(&html)
        );
        assert!(
            html.contains(r#"\$<sup class="footnote">"#),
            "{}",
            body(&html)
        );
        assert!(html.contains("1</a>. Note\n</div>"), "{}", body(&html));
    }

    #[test]
    fn index_terms_render_flow_text_and_hide_concealed_terms() {
        let html = render(":t: ((<tag>))\n\n((A))(((B)))indexterm2:[*C*] indexterm:[D] {t} ((https://example.org))");
        assert!(html.contains("<p>A<strong>C</strong>  &lt;tag&gt; <a href=\"https://example.org\" class=\"bare\">https://example.org</a></p>"), "{}", body(&html));
        // Flow terms have no wrapper: formatting can cross their boundaries.
        let html = render("*a ((b* c)) and ((d *e)) f* and https://example.org(((hidden)))/x");
        assert!(html.contains("<p><strong>a b</strong> c and d <strong>e f</strong> and <a href=\"https://example.org/x\" class=\"bare\">https://example.org/x</a></p>"), "{}", body(&html));
    }

    #[test]
    fn ui_macros_render_ruby_kbd_button_and_menu_markup() {
        let html = render(
            ":experimental:\n:k: kbd:[Alt+<]\n\nkbd:[F1] kbd:[*Ctrl*+T] btn:[OK] menu:File[Save] menu:A[B > C] menu:Tools[] {k}\n\n:icons: font\n\n\"A > B\"",
        );
        assert!(
            html.contains("<p><kbd>F1</kbd> <span class=\"keyseq\"><kbd><strong>Ctrl</strong></kbd>+<kbd>T</kbd></span> <b class=\"button\">OK</b> <span class=\"menuseq\"><b class=\"menu\">File</b>&#160;<b class=\"caret\">&#8250;</b> <b class=\"menuitem\">Save</b></span> <span class=\"menuseq\"><b class=\"menu\">A</b>&#160;<b class=\"caret\">&#8250;</b> <b class=\"submenu\">B</b>&#160;<b class=\"caret\">&#8250;</b> <b class=\"menuitem\">C</b></span> <b class=\"menuref\">Tools</b> <span class=\"keyseq\"><kbd>Alt</kbd>+<kbd>&lt;</kbd></span></p>"),
            "{html}"
        );
        // Font icons switch the caret, as in Ruby.
        assert!(
            html.contains("<p><span class=\"menuseq\"><b class=\"menu\">A</b>&#160;<i class=\"fa fa-angle-right caret\"></i> <b class=\"menuitem\">B</b></span></p>"),
            "{html}"
        );
        assert!(STYLESHEET.contains(".keyseq") && STYLESHEET.contains("b.button::before"));
        // Nested macros render inside the labels, well-formed where Ruby
        // misnests tags; formatting crossing the markup is split.
        let html = render(
            ":experimental:\n\nbtn:[link:x[y]] kbd:[Ctrl+link:k[a+b]] menu:A[link:m[x,y] > Z] btn:[OK footnote:[Press.]] kbd:[*a+b*] *x kbd:[a*] end",
        );
        assert!(
            html.contains("<p><b class=\"button\"><a href=\"x\">y</a></b> <span class=\"keyseq\"><kbd>Ctrl</kbd>+<kbd><a href=\"k\">a+b</a></kbd></span> <span class=\"menuseq\"><b class=\"menu\">A</b>&#160;<b class=\"caret\">&#8250;</b> <b class=\"submenu\"><a href=\"m\">x,y</a></b>&#160;<b class=\"caret\">&#8250;</b> <b class=\"menuitem\">Z</b></span> <b class=\"button\">OK <sup class=\"footnote\">[<a id=\"_footnoteref_1\" class=\"footnote\" href=\"#_footnotedef_1\" title=\"View footnote.\">1</a>]</sup></b> <span class=\"keyseq\"><kbd><strong>a</strong></kbd>+<kbd><strong>b</strong></kbd></span> <strong>x </strong><kbd><strong>a</strong></kbd> end</p>"),
            "{html}"
        );
        assert!(
            html.contains("<a href=\"#_footnoteref_1\">1</a>. Press.\n</div>"),
            "{html}"
        );
        // Without `experimental` the macros stay text.
        let html = render("kbd:[F1] menu:A[B]");
        assert!(html.contains("<p>kbd:[F1] menu:A[B]</p>"), "{html}");
    }

    #[test]
    fn footnotes_render_markers_and_the_list() {
        let html =
            render("A footnote:[*note* <x>] B footnote:x[named] C footnote:x[] D footnote:y[]");
        assert!(html.contains("A <sup class=\"footnote\">[<a id=\"_footnoteref_1\" class=\"footnote\" href=\"#_footnotedef_1\" title=\"View footnote.\">1</a>]</sup> B <sup class=\"footnote\" id=\"_footnote_x\">[<a id=\"_footnoteref_2\""), "{html}");
        assert!(html.contains("C <sup class=\"footnoteref\">[<a class=\"footnote\" href=\"#_footnotedef_2\" title=\"View footnote.\">2</a>]</sup>"), "{html}");
        assert!(html.contains("D <sup class=\"footnoteref red\" title=\"Unresolved footnote reference.\">[y]</sup>"), "{html}");
        assert!(html.contains("<div id=\"footnotes\">\n<hr>\n<div class=\"footnote\" id=\"_footnotedef_1\">\n<a href=\"#_footnoteref_1\">1</a>. <strong>note</strong> &lt;x&gt;\n</div>\n<div class=\"footnote\" id=\"_footnotedef_2\">\n<a href=\"#_footnoteref_2\">2</a>. named\n</div>\n</div>\n</body>"), "{html}");
        assert!(STYLESHEET.contains("#footnotes"));
        let html = render(":nofootnotes:\n\nA footnote:[hidden]");
        assert!(!html.contains("id=\"footnotes\""), "{html}");
        // An AsciiDoc cell lists its nested document's footnotes inside it.
        let html = render("|===\na|in footnote:[cell note]\n|===");
        assert!(html.contains("</p>\n<div id=\"footnotes\">\n<hr>\n<div class=\"footnote\" id=\"_footnotedef_1\">\n<a href=\"#_footnoteref_1\">1</a>. cell note\n</div>\n</div>\n</div></td>"), "{html}");
    }

    #[test]
    fn tables_render_ruby_tableblock_markup() {
        let html = render(
            "[[t]]\n.Data\n[.r,cols=\"1,2e\",width=40%,float=right]\n|===\n|H |I\n\n|<a> |{set:cellbgcolor:#eee}x\n\ny\nl|  <l> m|`c`\n|===",
        );
        assert!(
            html.contains("<table id=\"t\" class=\"tableblock frame-all grid-all right r\" style=\"width: 40%;\">"),
            "{html}"
        );
        assert!(
            html.contains("<caption class=\"title\">Table 1. Data</caption>"),
            "{html}"
        );
        assert!(
            html.contains("<col style=\"width: 33.3333%;\">\n<col style=\"width: 66.6667%;\">"),
            "{html}"
        );
        assert!(
            html.contains("<thead>\n<tr>\n<th class=\"tableblock halign-left valign-top\">H</th>"),
            "{html}"
        );
        assert!(
            html.contains("<p class=\"tableblock\">&lt;a&gt;</p>"),
            "{html}"
        );
        assert!(html.contains("style=\"background-color: #eee;\"><p class=\"tableblock\"><em>x</em></p>\n<p class=\"tableblock\"><em>y</em></p></td>"), "{html}");
        assert!(
            html.contains("<div class=\"literal\"><pre>  &lt;l&gt;</pre></div>"),
            "{html}"
        );
        assert!(
            html.contains("<p class=\"tableblock\"><code><code>c</code></code></p>"),
            "{html}"
        );
    }

    #[test]
    fn empty_tables_and_asciidoc_cells() {
        assert!(STYLESHEET.contains("table.tableblock"));
        let html = render("|===\n|===\n\n[cols=a]\n|===\n|*x*\n\ny\n|===");
        assert!(
            html.contains("<table class=\"tableblock frame-all grid-all stretch\">\n</table>"),
            "{html}"
        );
        // The nested document's blocks render like the parent's.
        assert!(
            html.contains("<td class=\"tableblock halign-left valign-top\"><div class=\"content\"><p><strong>x</strong></p>\n<p>y</p>\n</div></td>"),
            "{html}"
        );
        let html = render("[cols=a]\n|===\n|\n----\n<code>\n----\n!===\n!in\n!===\n|===");
        assert!(
            html.contains("<div class=\"content\"><pre class=\"listing\">&lt;code&gt;</pre>\n<table class=\"tableblock frame-all grid-all stretch\">"),
            "{html}"
        );
        let html = render("[%autowidth]\n|===\n|a\n|===");
        assert!(
            html.contains("fit-content\">\n<colgroup>\n<col>\n</colgroup>"),
            "{html}"
        );
    }

    #[test]
    fn images_render_figures_with_links_classes_and_captions() {
        let html = render(
            ":imagesdir: img/\n\n.A *bold* title\n[[fig1]]\nimage::../a b.png[Alt \"q\" (C) it's,200,100,link=https://x.org,window=_blank,float=left,align=center,role=r1]\n\nimage::b.png[link=l.html,opts=nofollow]\n\nimage::c.png[link=l.html,window=w,opts=\"nofollow,noopener\"]\n\nimage::d<&>.png[]",
        );
        assert_eq!(
            body(&html),
            "<figure id=\"fig1\" class=\"imageblock left text-center r1\">\n<a class=\"image\" href=\"https://x.org\" target=\"_blank\" rel=\"noopener\"><img src=\"a%20b.png\" alt=\"Alt &quot;q&quot; &#169; it&#8217;s\" width=\"200\" height=\"100\"></a>\n<figcaption class=\"title\">Figure 1. A <strong>bold</strong> title</figcaption>\n</figure>\n\
             <figure class=\"imageblock\">\n<a class=\"image\" href=\"l.html\" rel=\"nofollow\"><img src=\"img/b.png\" alt=\"b\"></a>\n</figure>\n\
             <figure class=\"imageblock\">\n<a class=\"image\" href=\"l.html\" target=\"w\" rel=\"nofollow noopener\"><img src=\"img/c.png\" alt=\"c\"></a>\n</figure>\n\
             <figure class=\"imageblock\">\n<img src=\"img/d&lt;&amp;&gt;.png\" alt=\"d&lt;&amp;&gt;\">\n</figure>\n</body>\n</html>\n"
        );
    }
}
