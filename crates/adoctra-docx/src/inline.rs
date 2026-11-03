//! Runs from inline content: `Inline` trees and the flat `AttributePart`
//! streams substitutions produce.
//!
//! Word gives a run one character style. The most specific active one is the
//! run's `w:rStyle` (keyboard/button/menu, callout, footnote reference,
//! monospace, mark, xref/hyperlink, strong, emphasis); the other active
//! formats become direct properties (`w:b`, `w:i`, highlight, underline,
//! vertical alignment), so nested formatting is never lost.
//!
//! Inline nesting is bounded by the inline syntax (like the HTML backend),
//! so the tree is walked recursively; blocks use an explicit stack.

use std::collections::{BTreeMap, BTreeSet};

use adoctra_core::{AttributeFormat, AttributePart, Inline};

use crate::package::Relationships;
use crate::styles::{StyleKind, Styles};
use crate::text::{decode_references, raw_text};
use crate::xml::escape_text;

/// Relationship type of a hyperlink.
pub const REL_HYPERLINK: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

/// Monospace font for a template without a `Monospace` style.
const FALLBACK_MONO: &str =
    r#"<w:rFonts w:ascii="Courier New" w:hAnsi="Courier New" w:cs="Courier New"/>"#;

/// Warnings reported once each.
#[derive(Debug, Default)]
pub struct Warnings {
    pub list: Vec<String>,
    seen: BTreeSet<String>,
}

impl Warnings {
    pub fn once(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.seen.insert(message.clone()) {
            self.list.push(message);
        }
    }
}

/// A complex field (`XE`, `NOTEREF`…): its instruction and, when Word
/// should show something before updating, the result runs.
pub fn field(instruction: &str, result: &str) -> String {
    let instruction = escape_text(instruction);
    if result.is_empty() {
        return format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instruction} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        );
    }
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instruction} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{result}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

/// A field instruction's quoted argument (Word's quotes cannot nest).
fn field_text(text: &str) -> String {
    text.replace('"', "\u{201d}")
}

/// Bookmark names for AsciiDoc ids. Word limits a name to 40 letters, digits
/// and underscores, starting with a letter (an underscore makes a hidden
/// bookmark, like Asciidoctor's generated `_section` ids). Each id maps to
/// one stable, unique name, whether its anchor or a reference comes first.
#[derive(Debug, Default)]
pub struct Bookmarks {
    names: BTreeMap<String, String>,
    taken: BTreeSet<String>,
    next_id: u32,
    placed: BTreeSet<String>,
}

impl Bookmarks {
    pub fn name(&mut self, id: &str) -> String {
        if let Some(name) = self.names.get(id) {
            return name.clone();
        }
        let mut base: String = id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if !base.starts_with(|c: char| c.is_alphabetic() || c == '_') {
            base.insert(0, '_');
        }
        let mut name: String = base.chars().take(40).collect();
        let mut n = 1;
        while self.taken.contains(&name.to_lowercase()) {
            n += 1;
            let suffix = format!("_{n}");
            name = base.chars().take(40 - suffix.len()).collect::<String>() + &suffix;
        }
        self.taken.insert(name.to_lowercase());
        self.names.insert(id.to_string(), name.clone());
        name
    }

    /// A bookmark named `name` around `content`.
    pub fn around(&mut self, name: &str, content: &str) -> String {
        let n = self.next_id;
        self.next_id += 1;
        format!(
            r#"<w:bookmarkStart w:id="{n}" w:name="{}"/>{content}<w:bookmarkEnd w:id="{n}"/>"#,
            crate::xml::escape_attr(name)
        )
    }

    /// A zero-length bookmark for `id` (once per id: a duplicate anchor
    /// keeps the first).
    pub fn point(&mut self, id: &str) -> String {
        if !self.placed.insert(id.to_string()) {
            return String::new();
        }
        let name = self.name(id);
        let n = self.next_id;
        self.next_id += 1;
        format!(
            r#"<w:bookmarkStart w:id="{n}" w:name="{}"/><w:bookmarkEnd w:id="{n}"/>"#,
            crate::xml::escape_attr(&name)
        )
    }
}

/// What the converter shares while writing: the template's styles and the
/// main part's relationships, bookmarks and warnings.
pub struct Cx<'t> {
    pub styles: &'t mut Styles,
    pub rels: &'t mut Relationships,
    pub numbering: &'t mut crate::numbering::Numbering,
    pub bookmarks: Bookmarks,
    pub warnings: Warnings,
    pub media: crate::media::Media,
    pub notes: Notes,
    /// The page's text width (twips): the widest an inline image is drawn.
    pub text_width: u32,
    /// URIs are read (`allow-uri-read`), not linked.
    pub allow_uri: bool,
}

/// The document's footnotes as Word footnotes.
#[derive(Debug, Default)]
pub struct Notes {
    next_id: u32,
    /// Footnote id and bookmark by Ruby's footnote index.
    by_index: BTreeMap<String, (u32, String)>,
    /// `w:footnote` elements.
    pub xml: String,
}

impl Notes {
    /// Footnotes numbered after `first` (the template's ids).
    pub fn starting_at(first: u32) -> Self {
        Self {
            next_id: first,
            ..Self::default()
        }
    }
}

impl Cx<'_> {
    pub fn style(&mut self, kind: StyleKind, name: &str) -> Option<String> {
        self.styles.resolve(kind, name)
    }
}

/// How a newline in text is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Newlines {
    /// Paragraph text: source line breaks are spaces.
    Space,
    /// Verbatim and verse text: a line break.
    Break,
}

#[derive(Debug, Clone, Copy, Default)]
struct Fmt {
    strong: u16,
    emphasis: u16,
    mono: u16,
    mark: u16,
    sup: u16,
    sub: u16,
    underline: u16,
    strike: u16,
    /// A character style of a UI macro, callout or footnote reference.
    special: Option<&'static str>,
    link: Option<&'static str>,
}

/// Effects of an opened format, undone by its close.
#[derive(Debug, Clone)]
struct Opened {
    format: AttributeFormat,
    underline: bool,
    strike: bool,
    /// The role style this format replaced (restored on close).
    role_style: Option<Option<String>>,
}

/// Writes runs for inline content into one paragraph.
pub struct Runs<'c, 't> {
    cx: &'c mut Cx<'t>,
    out: String,
    fmt: Fmt,
    newlines: Newlines,
    link_depth: u32,
    opened: Vec<Opened>,
    /// Writing a copy of a title (a TOC entry): no anchors, footnotes or
    /// index entries, which belong to the original.
    outline: bool,
    /// The template's character style named by an enclosing phrase role.
    role_style: Option<String>,
    /// A hard line break was just written: the source line end after it
    /// (a newline or the space core keeps for it) is not text.
    after_break: bool,
}

impl<'c, 't> Runs<'c, 't> {
    pub fn new(cx: &'c mut Cx<'t>, newlines: Newlines) -> Self {
        Self {
            cx,
            out: String::new(),
            fmt: Fmt::default(),
            newlines,
            link_depth: 0,
            opened: Vec::new(),
            outline: false,
            role_style: None,
            after_break: false,
        }
    }

    /// Runs for a copy of a title (see [`Runs::outline`]).
    pub fn outline(mut self) -> Self {
        self.outline = true;
        self
    }

    /// The runs, closing a link left open by unbalanced parts.
    pub fn finish(mut self) -> String {
        if self.link_depth > 0 {
            self.out.push_str("</w:hyperlink>");
        }
        self.out
    }

    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    /// Plain text with the current formatting.
    pub fn text(&mut self, text: &str) {
        let text = match std::mem::take(&mut self.after_break) {
            true => text.trim_start_matches([' ', '\n']),
            false => text,
        };
        if text.is_empty() {
            return;
        }
        let rpr = self.rpr();
        self.out.push_str("<w:r>");
        self.out.push_str(&rpr);
        let mut chunk = String::new();
        let flush = |chunk: &mut String, out: &mut String| {
            if !chunk.is_empty() {
                out.push_str(r#"<w:t xml:space="preserve">"#);
                out.push_str(&escape_text(chunk));
                out.push_str("</w:t>");
                chunk.clear();
            }
        };
        for ch in text.chars() {
            match ch {
                '\t' => {
                    flush(&mut chunk, &mut self.out);
                    self.out.push_str("<w:tab/>");
                }
                '\n' if self.newlines == Newlines::Break => {
                    flush(&mut chunk, &mut self.out);
                    self.out.push_str("<w:br/>");
                }
                '\n' | '\r' => chunk.push(' '),
                c => chunk.push(c),
            }
        }
        flush(&mut chunk, &mut self.out);
        self.out.push_str("</w:r>");
    }

    /// Text in character style `style` (a callout, a UI label).
    pub fn styled(&mut self, style: &'static str, text: &str) {
        let saved = self.fmt.special.replace(style);
        self.text(text);
        self.fmt.special = saved;
    }

    /// A callout mark like Ruby's: with icons the number in the `Callout`
    /// style (Ruby's numbered circle), as text `(n)` in bold.
    pub fn conum(&mut self, number: &str, icons: Option<&str>) {
        match icons {
            Some(_) => self.styled("Callout", number),
            None => self.with(AttributeFormat::Strong, |r| r.text(&format!("({number})"))),
        }
    }

    /// Raw run markup (a drawing).
    pub fn markup(&mut self, markup: &str) {
        self.out.push_str(markup);
    }

    /// `f`'s content as an external link to `target`.
    pub fn link(&mut self, target: &str, f: impl FnOnce(&mut Self)) {
        self.open_link(Some(target), None, None);
        f(self);
        self.close_link();
    }

    /// `f`'s content as a link to the bookmark of `id`.
    pub fn anchor_link(&mut self, id: &str, f: impl FnOnce(&mut Self)) {
        self.open_link(None, Some(id), None);
        f(self);
        self.close_link();
    }

    pub fn line_break(&mut self) {
        self.out.push_str("<w:r><w:br/></w:r>");
        self.after_break = true;
    }

    fn rpr(&mut self) -> String {
        let f = self.fmt;
        // A role's style (the template has it) comes right after the
        // macro styles.
        let role = self.role_style.clone();
        let candidates = [
            f.special,
            role.as_deref(),
            (f.mono > 0).then_some("Monospace"),
            (f.mark > 0).then_some("Mark"),
            f.link,
            (f.strong > 0).then_some("Strong"),
            (f.emphasis > 0).then_some("Emphasis"),
        ];
        let mut chosen = None;
        let mut style_id = None;
        for name in candidates.into_iter().flatten() {
            if let Some(id) = self.cx.style(StyleKind::Character, name) {
                chosen = Some(name.to_string());
                style_id = Some(id);
                break;
            }
        }
        let is = |name: &str| chosen.as_deref() == Some(name);
        let mut props = String::new();
        if let Some(id) = &style_id {
            props.push_str(&format!(
                r#"<w:rStyle w:val="{}"/>"#,
                crate::xml::escape_attr(id)
            ));
        }
        if f.mono > 0 && !is("Monospace") && f.special.is_none() {
            props.push_str(FALLBACK_MONO);
        }
        if f.strong > 0 && !is("Strong") {
            props.push_str("<w:b/><w:bCs/>");
        }
        if f.emphasis > 0 && !is("Emphasis") {
            props.push_str("<w:i/><w:iCs/>");
        }
        if f.strike > 0 {
            props.push_str("<w:strike/>");
        }
        if f.mark > 0 && !is("Mark") {
            props.push_str(r#"<w:highlight w:val="yellow"/>"#);
        }
        if f.underline > 0 || (f.link.is_some() && !f.link.is_some_and(is)) {
            props.push_str(r#"<w:u w:val="single"/>"#);
        }
        if f.sup > 0 {
            props.push_str(r#"<w:vertAlign w:val="superscript"/>"#);
        } else if f.sub > 0 {
            props.push_str(r#"<w:vertAlign w:val="subscript"/>"#);
        }
        if props.is_empty() {
            props
        } else {
            format!("<w:rPr>{props}</w:rPr>")
        }
    }

    fn counter(&mut self, format: AttributeFormat) -> Option<&mut u16> {
        Some(match format {
            AttributeFormat::Strong => &mut self.fmt.strong,
            AttributeFormat::Emphasis => &mut self.fmt.emphasis,
            AttributeFormat::Monospace => &mut self.fmt.mono,
            AttributeFormat::Mark => &mut self.fmt.mark,
            AttributeFormat::Superscript => &mut self.fmt.sup,
            AttributeFormat::Subscript => &mut self.fmt.sub,
            AttributeFormat::Span => return None,
        })
    }

    /// Write `f`'s content with `format` added.
    pub fn with(&mut self, format: AttributeFormat, f: impl FnOnce(&mut Self)) {
        if let Some(c) = self.counter(format) {
            *c += 1;
        }
        f(self);
        if let Some(c) = self.counter(format) {
            *c = c.saturating_sub(1);
        }
    }

    /// Open an external hyperlink (or a link to bookmark `anchor`).
    fn open_link(&mut self, target: Option<&str>, anchor: Option<&str>, tooltip: Option<&str>) {
        self.link_depth += 1;
        if self.link_depth > 1 {
            // Word links do not nest: the outer link wins.
            return;
        }
        let mut open = String::from("<w:hyperlink");
        match (anchor, target) {
            (Some(anchor), _) => {
                let name = self.cx.bookmarks.name(anchor);
                open.push_str(&format!(
                    r#" w:anchor="{}""#,
                    crate::xml::escape_attr(&name)
                ));
                self.fmt.link = Some("Xref");
            }
            (None, Some(target)) => {
                let id = self.cx.rels.add(REL_HYPERLINK, target, true);
                open.push_str(&format!(r#" r:id="{id}""#));
                self.fmt.link = Some("Hyperlink");
            }
            (None, None) => self.fmt.link = Some("Xref"),
        }
        if let Some(tooltip) = tooltip {
            open.push_str(&format!(
                r#" w:tooltip="{}""#,
                crate::xml::escape_attr(tooltip)
            ));
        }
        open.push_str(r#" w:history="1">"#);
        self.out.push_str(&open);
    }

    fn close_link(&mut self) {
        match self.link_depth {
            0 => {}
            1 => {
                self.link_depth = 0;
                self.fmt.link = None;
                self.out.push_str("</w:hyperlink>");
            }
            _ => self.link_depth -= 1,
        }
    }

    /// Open the link of a cross-reference: a bookmark in this document, or
    /// another document's path.
    fn open_xref(&mut self, xref: &adoctra_core::Xref, lexical: bool) {
        let field = |s: &str| {
            if lexical {
                decode_references(s)
            } else {
                s.to_string()
            }
        };
        match (&xref.path, &xref.fragment) {
            (None, Some(fragment)) => self.open_link(None, Some(&field(fragment)), None),
            (None, None) => self.open_link(None, None, None),
            (Some(_), _) => self.open_link(Some(&field(&xref.target)), None, None),
        }
    }

    pub fn inlines(&mut self, inlines: &[Inline]) {
        for inline in inlines {
            self.inline(inline);
        }
    }

    fn inline(&mut self, inline: &Inline) {
        match inline {
            Inline::Text { value, .. } => self.text(value),
            Inline::Raw { value, .. } => self.text(&raw_text(value)),
            Inline::Substituted { parts, .. } => self.parts(parts),
            Inline::Strong { children, .. } => {
                self.with(AttributeFormat::Strong, |r| r.inlines(children))
            }
            Inline::Emphasis { children, .. } => {
                self.with(AttributeFormat::Emphasis, |r| r.inlines(children))
            }
            Inline::Monospace { children, .. } => {
                self.with(AttributeFormat::Monospace, |r| r.inlines(children))
            }
            Inline::Mark { children, .. } => {
                self.with(AttributeFormat::Mark, |r| r.inlines(children))
            }
            Inline::Superscript { children, .. } => {
                self.with(AttributeFormat::Superscript, |r| r.inlines(children))
            }
            Inline::Subscript { children, .. } => {
                self.with(AttributeFormat::Subscript, |r| r.inlines(children))
            }
            Inline::AttributeRef { value, parts, .. } if !parts.is_empty() && value.is_some() => {
                self.parts(parts)
            }
            Inline::AttributeRef { name, value, .. } => match value {
                Some(value) => self.text(value),
                None => self.text(&format!("{{{name}}}")),
            },
            Inline::Link {
                target, children, ..
            } => {
                self.open_link(Some(target), None, None);
                self.inlines(children);
                self.close_link();
            }
            Inline::CrossReference { xref, children, .. } => {
                self.open_xref(xref, false);
                self.inlines(children);
                self.close_link();
            }
            Inline::Anchor { .. } if self.outline => {}
            Inline::Anchor { id, .. } => {
                let mark = self.cx.bookmarks.point(id);
                self.out.push_str(&mark);
            }
            Inline::Image { image, .. } => self.image(image),
            Inline::Footnote { footnote, .. } => self.footnote(footnote),
            Inline::Ui { ui, .. } => self.ui(ui),
            Inline::IndexTerm { term, .. } => self.index_term(term),
            Inline::Stem { stem, .. } => self.parts(&stem.parts),
        }
    }

    pub fn parts(&mut self, parts: &[AttributePart]) {
        for part in parts {
            match part {
                AttributePart::Text(text) => self.text(text),
                AttributePart::Raw(raw) => self.text(&raw_text(raw)),
                AttributePart::Open(format) => self.open(*format, None, None),
                AttributePart::OpenWithAttributes { format, id, role } => {
                    self.open(*format, id.as_deref(), role.as_deref())
                }
                AttributePart::Close(format) => self.close(*format),
                AttributePart::OpenLink(link) => {
                    let target = decode_references(&link.target);
                    let title = link.title.as_deref().map(decode_references);
                    self.open_link(Some(&target), None, title.as_deref());
                }
                AttributePart::CloseLink | AttributePart::CloseXref => self.close_link(),
                AttributePart::OpenXref(xref) => self.open_xref(xref, true),
                AttributePart::LineBreak => self.line_break(),
                AttributePart::Anchor(_) if self.outline => {}
                AttributePart::Anchor(id) => {
                    let mark = self.cx.bookmarks.point(&decode_references(id));
                    self.out.push_str(&mark);
                }
                AttributePart::Image(image) => self.image(image),
                AttributePart::Footnote(footnote) => self.footnote(footnote),
                AttributePart::Ui(ui) => self.ui(ui),
                AttributePart::IndexTerm(term) => self.index_term(term),
                AttributePart::Stem(stem) => self.parts(&stem.parts),
                AttributePart::Callout(callout) => {
                    if callout.xml {
                        self.text("<!--");
                    } else if let Some(guard) = &callout.guard {
                        self.text(&raw_text(guard));
                    }
                    self.conum(&callout.number, callout.icons.as_deref());
                    if callout.xml {
                        self.text("-->");
                    }
                }
            }
        }
    }

    fn open(&mut self, format: AttributeFormat, id: Option<&str>, role: Option<&str>) {
        if let Some(c) = self.counter(format) {
            *c += 1;
        }
        let roles: Vec<&str> = role.unwrap_or_default().split_whitespace().collect();
        // A role naming a character style of the template applies it.
        let styled = roles
            .iter()
            .map(|r| decode_references(r))
            .find(|r| self.cx.styles.has(StyleKind::Character, r));
        let role_style = styled.map(|style| self.role_style.replace(style));
        let opened = Opened {
            format,
            underline: roles.contains(&"underline"),
            strike: roles.contains(&"line-through"),
            role_style,
        };
        self.fmt.underline += u16::from(opened.underline);
        self.fmt.strike += u16::from(opened.strike);
        self.opened.push(opened);
        if let Some(id) = id {
            let mark = self.cx.bookmarks.point(&decode_references(id));
            self.out.push_str(&mark);
        }
    }

    fn close(&mut self, format: AttributeFormat) {
        if let Some(c) = self.counter(format) {
            *c = c.saturating_sub(1);
        }
        if let Some(at) = self.opened.iter().rposition(|o| o.format == format) {
            let opened = self.opened.remove(at);
            if let Some(previous) = opened.role_style {
                self.role_style = previous;
            }
            self.fmt.underline = self
                .fmt
                .underline
                .saturating_sub(u16::from(opened.underline));
            self.fmt.strike = self.fmt.strike.saturating_sub(u16::from(opened.strike));
        }
    }

    /// An inline image (or an image icon) drawn in the text, sized by its
    /// `width`/`height`; a text or font icon, or an image that cannot be
    /// read, shows its alt text in brackets, like Ruby's text icon.
    fn image(&mut self, image: &adoctra_core::InlineImage) {
        let alt = decode_references(&parts_text(&image.alt));
        let drawable = !image.icon || image.icons.as_deref().is_some_and(|i| i != "font");
        if !drawable || self.outline {
            self.text(&format!("[{alt}]"));
            return;
        }
        let src = if image.src.is_empty() {
            &image.target
        } else {
            &image.src
        };
        let src = decode_references(src);
        let picture = self.cx.media.picture(&src, self.cx.rels, self.cx.allow_uri);
        let picture = match picture {
            Ok(picture) => picture,
            Err(reason) => {
                self.cx.warnings.once(format!("{reason}: {src}"));
                self.text(&format!("[{alt}]"));
                return;
            }
        };
        let size = crate::media::draw_size(&image.attributes, picture.pixels, self.cx.text_width);
        let id = self.cx.media.doc_pr();
        let name = src.rsplit('/').next().unwrap_or_default().to_string();
        let drawing = crate::media::drawing(
            &picture,
            size,
            id,
            &name,
            &alt,
            crate::media::Placement::Inline,
        );
        match image.attributes.get("link") {
            Some(link) => {
                self.open_link(Some(&decode_references(link)), None, None);
                self.out.push_str(&drawing);
                self.close_link();
            }
            None => self.out.push_str(&drawing),
        }
    }

    /// A Word footnote: the first reference to a footnote defines it (its
    /// content in the footnotes part, a bookmark at its mark); a repeated
    /// reference is a `NOTEREF` to that mark. An unknown id shows `[id]`,
    /// like Ruby.
    fn footnote(&mut self, footnote: &adoctra_core::Footnote) {
        if self.outline {
            return;
        }
        let index = footnote.index.clone().unwrap_or_default();
        let reference_props = match self.cx.style(StyleKind::Character, "Footnote Reference") {
            Some(id) => format!(
                r#"<w:rPr><w:rStyle w:val="{}"/></w:rPr>"#,
                crate::xml::escape_attr(&id)
            ),
            None => r#"<w:rPr><w:vertAlign w:val="superscript"/></w:rPr>"#.to_string(),
        };
        match (footnote.kind, self.cx.notes.by_index.get(&index).cloned()) {
            (adoctra_core::FootnoteKind::Unresolved, _) => {
                self.fmt.sup += 1;
                self.text(&format!("[{}]", footnote.id.clone().unwrap_or_default()));
                self.fmt.sup -= 1;
            }
            (_, Some((_, bookmark))) => {
                let result = format!(
                    r#"<w:r>{reference_props}<w:t>{}</w:t></w:r>"#,
                    escape_text(&index)
                );
                self.out
                    .push_str(&field(&format!("NOTEREF {bookmark} \\f \\h"), &result));
            }
            (_, None) => {
                let id = self.cx.notes.next_id;
                self.cx.notes.next_id += 1;
                let bookmark = format!("_Ref_footnote_{id}");
                self.cx.notes.by_index.insert(index, (id, bookmark.clone()));
                let mark =
                    format!(r#"<w:r>{reference_props}<w:footnoteReference w:id="{id}"/></w:r>"#);
                let marked = self.cx.bookmarks.around(&bookmark, &mark);
                self.out.push_str(&marked);
                // The footnote's text, written with the same styles.
                let mut inner = Runs::new(self.cx, Newlines::Space);
                inner.parts(&footnote.content);
                let content = inner.finish();
                let text_style = match self.cx.style(StyleKind::Paragraph, "Footnote Text") {
                    Some(id) => format!(
                        r#"<w:pPr><w:pStyle w:val="{}"/></w:pPr>"#,
                        crate::xml::escape_attr(&id)
                    ),
                    None => String::new(),
                };
                self.cx.notes.xml.push_str(&format!(
                    r#"<w:footnote w:id="{id}"><w:p>{text_style}<w:r>{reference_props}<w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> </w:t></w:r>{content}</w:p></w:footnote>"#
                ));
            }
        }
    }

    /// An index term: a flow term's text, then an `XE` field with its
    /// terms (`Big cats:Lions`) and a `see` reference; a concealed term only
    /// the field.
    fn index_term(&mut self, term: &adoctra_core::IndexTerm) {
        if let Some(text) = &term.text {
            self.parts(text);
        }
        if self.outline {
            return;
        }
        let entry = if term.terms.is_empty() {
            term.text.as_deref().map(parts_text).unwrap_or_default()
        } else {
            term.terms
                .iter()
                .map(|t| decode_references(t).replace(':', "\\:"))
                .collect::<Vec<_>>()
                .join(":")
        };
        if entry.trim().is_empty() {
            return;
        }
        let mut instruction = format!("XE \"{}\"", field_text(&entry));
        if let Some(see) = &term.see {
            instruction.push_str(&format!(
                " \\t \"See {}\"",
                field_text(&decode_references(see))
            ));
        } else if !term.see_also.is_empty() {
            let also = term
                .see_also
                .iter()
                .map(|t| decode_references(t))
                .collect::<Vec<_>>()
                .join(", ");
            instruction.push_str(&format!(" \\t \"See also {}\"", field_text(&also)));
        }
        self.out.push_str(&field(&instruction, ""));
    }

    /// Ruby's keyboard, button and menu macros: each label in its character
    /// style (`Keyboard`, `Button`, `Menu`), with Ruby's separators.
    fn ui(&mut self, ui: &adoctra_core::UiMacro) {
        use adoctra_core::UiMacro;
        let label = |r: &mut Self, style: &'static str, parts: &[AttributePart]| {
            let saved = r.fmt.special.replace(style);
            r.parts(parts);
            r.fmt.special = saved;
        };
        match ui {
            UiMacro::Keyboard { keys } => {
                for (i, key) in keys.iter().enumerate() {
                    if i > 0 {
                        self.text("+");
                    }
                    label(self, "Keyboard", key);
                }
            }
            UiMacro::Button { label: text } => {
                let saved = self.fmt.special.replace("Button");
                self.text("[");
                self.parts(text);
                self.text("]");
                self.fmt.special = saved;
            }
            UiMacro::Menu {
                menu,
                submenus,
                item,
                ..
            } => {
                label(self, "Menu", menu);
                for submenu in submenus {
                    self.text("\u{a0}\u{203a} ");
                    label(self, "Menu", submenu);
                }
                if let Some(item) = item {
                    self.text("\u{a0}\u{203a} ");
                    label(self, "Menu", item);
                }
            }
        }
    }
}

/// The plain text of inline content (document properties, bookmarks).
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text { value, .. } => out.push_str(value),
            Inline::Raw { value, .. } => out.push_str(&raw_text(value)),
            Inline::Substituted { parts, .. } => out.push_str(&parts_text(parts)),
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Monospace { children, .. }
            | Inline::Mark { children, .. }
            | Inline::Superscript { children, .. }
            | Inline::Subscript { children, .. }
            | Inline::Link { children, .. }
            | Inline::CrossReference { children, .. } => out.push_str(&plain_text(children)),
            Inline::AttributeRef { value, parts, .. } if !parts.is_empty() && value.is_some() => {
                out.push_str(&parts_text(parts))
            }
            Inline::AttributeRef { name, value, .. } => match value {
                Some(value) => out.push_str(value),
                None => out.push_str(&format!("{{{name}}}")),
            },
            Inline::IndexTerm { term, .. } => {
                if let Some(text) = &term.text {
                    out.push_str(&parts_text(text));
                }
            }
            Inline::Stem { stem, .. } => out.push_str(&parts_text(&stem.parts)),
            Inline::Anchor { .. }
            | Inline::Image { .. }
            | Inline::Footnote { .. }
            | Inline::Ui { .. } => {}
        }
    }
    out.replace('\n', " ")
}

/// The plain text of attribute parts.
pub fn parts_text(parts: &[AttributePart]) -> String {
    let mut out = String::new();
    for part in parts {
        match part {
            AttributePart::Text(text) => out.push_str(text),
            AttributePart::Raw(raw) => out.push_str(&raw_text(raw)),
            AttributePart::LineBreak => out.push(' '),
            AttributePart::IndexTerm(term) => {
                if let Some(text) = &term.text {
                    out.push_str(&parts_text(text));
                }
            }
            AttributePart::Stem(stem) => out.push_str(&parts_text(&stem.parts)),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_names_are_valid_unique_and_stable() {
        let mut b = Bookmarks::default();
        assert_eq!(b.name("_hello_world"), "_hello_world");
        assert_eq!(b.name("intro-1.2"), "intro_1_2");
        assert_eq!(b.name("intro_1_2"), "intro_1_2_2");
        assert_eq!(b.name("intro-1.2"), "intro_1_2");
        assert_eq!(b.name("1st"), "_1st");
        assert_eq!(b.name("раздел"), "раздел");
        let long = "a".repeat(50);
        assert_eq!(b.name(&long).chars().count(), 40);
        assert_eq!(b.name(&format!("{long}b")), format!("{}_2", "a".repeat(38)));
    }

    #[test]
    fn a_bookmark_is_placed_once_with_increasing_ids() {
        let mut b = Bookmarks::default();
        assert_eq!(
            b.point("x"),
            r#"<w:bookmarkStart w:id="0" w:name="x"/><w:bookmarkEnd w:id="0"/>"#
        );
        assert_eq!(b.point("x"), "");
        assert!(b.point("y").contains(r#"w:id="1""#));
    }

    #[test]
    fn warnings_are_reported_once() {
        let mut w = Warnings::default();
        w.once("a");
        w.once("b");
        w.once("a");
        assert_eq!(w.list, ["a", "b"]);
    }
}
