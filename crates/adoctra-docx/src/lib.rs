//! DOCX backend: renders a document into a `.docx` template.
//!
//! The look comes from a Word template, not from the converter: every
//! element gets a named style ([`contract`]) that the template defines. The
//! built-in templates ([`Builtin`]) are `book` (for `doctype=book`) and
//! `article` (everything else); a custom `.docx`/`.dotx` can replace them.
//! The template package is kept as is: only the body, the markers
//! (`adoctra:*` content controls) and the parts the document needs
//! (styles added on demand, relationships) change. See `docs/docx.adoc`.

mod builtin;
pub mod contract;
mod fallback;
mod highlight;
mod inline;
mod media;
mod numbering;
mod package;
mod props;
mod sections;
mod styles;
mod table;
mod template;
mod text;
mod writer;
mod xml;

use std::collections::BTreeMap;
use std::sync::Arc;

use adoctra_core::ast::BlockKind;
use adoctra_core::{Backend, Document, Error, Inline, ResourceProvider, Result};

pub use builtin::Builtin;
use inline::{Cx, Newlines, Runs};
use props::CoreProperties;
use sections::Sections;
use template::{Fill, Template};
use writer::Writer;

/// The DOCX backend.
#[derive(Default, Clone)]
pub struct DocxBackend {
    template: Option<Arc<[u8]>>,
    resources: Option<(Arc<dyn ResourceProvider>, Option<String>)>,
}

impl std::fmt::Debug for DocxBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocxBackend")
            .field("template", &self.template.as_ref().map(|t| t.len()))
            .field("resources", &self.resources.as_ref().map(|(_, r)| r))
            .finish()
    }
}

/// A converted document and the warnings the conversion reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub bytes: Vec<u8>,
    pub warnings: Vec<String>,
}

impl DocxBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Render into `template` (the bytes of a `.docx` or `.dotx`) instead of
    /// the built-in template for the document's doctype.
    pub fn with_template(mut self, template: impl Into<Arc<[u8]>>) -> Self {
        self.template = Some(template.into());
        self
    }

    /// Read images through `provider`, relative to `referrer` (the primary
    /// document's canonical name), like the HTML backend's assets. Without a
    /// provider only data URIs are embedded (URIs are linked).
    pub fn with_resources(
        mut self,
        provider: Arc<dyn ResourceProvider>,
        referrer: Option<String>,
    ) -> Self {
        self.resources = Some((provider, referrer));
        self
    }

    /// The template named by the `docx-template` attribute, read through the
    /// resource provider from the document's base directory (jailed by the
    /// safe mode, like Ruby's stylesheet). The error is a warning; the
    /// built-in template is used instead.
    fn load_template(
        &self,
        doc: &Document,
        path: &str,
        warnings: &mut Vec<String>,
    ) -> std::result::Result<Template, String> {
        let Some((provider, referrer)) = &self.resources else {
            return Err(format!(
                "docx template cannot be read without a resource provider: {path}"
            ));
        };
        let referrer_dir = referrer
            .as_deref()
            .and_then(|name| provider.document_path(name))
            .and_then(|p| p.rsplit_once('/').map(|(dir, _)| dir.to_string()))
            .map(|dir| if dir.is_empty() { "/".to_string() } else { dir });
        let resolved = adoctra_core::asset_path(
            path,
            None,
            doc.base_dir.as_deref(),
            referrer_dir.as_deref(),
            adoctra_core::SafeMode::of(&doc.attributes),
            "docx template",
        );
        warnings.extend(resolved.warnings);
        let bytes = provider
            .load_binary(adoctra_core::ResourceRequest {
                target: &resolved.request,
                referrer: referrer.as_deref(),
            })
            .map_err(|_| format!("docx template not found or unreadable: {path}"))?
            .bytes;
        Template::read(&bytes).map_err(|reason| format!("invalid docx template {path}: {reason}"))
    }

    /// Convert and report the warnings.
    pub fn render(&self, doc: &Document) -> Result<Output> {
        let mut early_warnings = Vec::new();
        let builtin =
            |b: Builtin| Template::from_package(b.package()).expect("built-in templates are valid");
        let by_doctype = Builtin::for_doctype(doc.attributes.get("doctype"));
        let mut template = match (
            &self.template,
            doc.attributes
                .get("docx-template")
                .filter(|v| !v.is_empty()),
        ) {
            // A template given to the backend wins over the attribute.
            (Some(bytes), _) => Template::read(bytes).map_err(invalid_template)?,
            (None, Some("book")) => builtin(Builtin::Book),
            (None, Some("article")) => builtin(Builtin::Article),
            (None, Some(path)) => match self.load_template(doc, path, &mut early_warnings) {
                Ok(template) => template,
                Err(warning) => {
                    early_warnings.push(warning);
                    builtin(by_doctype)
                }
            },
            (None, None) => builtin(by_doctype),
        };
        let title = header_title(doc);
        let title_marker = template.has_marker("title");
        // Parts and chapters of a book start sections when its body section
        // has a distinct first page.
        let mut sections = template.body_sect_pr().map(|sect_pr| {
            let chapters = doc.attributes.get("doctype") == Some("book")
                && (sect_pr.contains("<w:titlePg/>") || sect_pr.contains("<w:titlePg "));
            let running = if chapters {
                template.running_parts(&sect_pr)
            } else {
                Vec::new()
            };
            Sections::new(sect_pr, chapters, running)
        });
        let text_width = sections
            .as_ref()
            .map_or(sections::DEFAULT_TEXT_WIDTH, Sections::text_width);
        let first_footnote = template.first_footnote_id();
        let allow_uri = doc.attributes.get("allow-uri-read").is_some()
            && adoctra_core::SafeMode::of(&doc.attributes) < adoctra_core::SafeMode::Secure;
        let (provider, referrer) = match &self.resources {
            Some((provider, referrer)) => (Some(provider.clone()), referrer.clone()),
            None => (None, None),
        };
        let (styles, rels, numbering) = template.parts_mut();
        let mut cx = Cx {
            styles,
            rels,
            numbering,
            bookmarks: Default::default(),
            warnings: Default::default(),
            media: media::Media::new(provider, referrer),
            notes: inline::Notes::starting_at(first_footnote),
            text_width,
            allow_uri,
        };
        let fields = header_fields(&mut cx, doc, title.as_ref());
        // Without a title marker the title opens the body.
        let lead = match (&title, title_marker) {
            (Some(_), false) => {
                let mut lead = String::new();
                for (field, style) in [("title", "Title"), ("subtitle", "Subtitle")] {
                    if let Some(runs) = fields.get(field) {
                        lead.push_str(&writer::simple_paragraph(&mut cx, style, runs));
                    }
                }
                Some(lead)
            }
            _ => None,
        };
        let body = Writer::new(&mut cx, doc, sections.as_mut()).body(lead);
        let mut warnings = early_warnings;
        warnings.append(&mut cx.warnings.list);
        let media = std::mem::take(&mut cx.media.parts);
        let footnotes = std::mem::take(&mut cx.notes.xml);
        warnings.extend(template.styles.warnings.iter().cloned());
        template.add_media(media);
        template.add_footnotes(footnotes);
        template.set_core_properties(&core_properties(doc, title.as_ref()).to_xml());
        let sect_pr = sections.as_ref().and_then(Sections::final_sect_pr);
        let copies = sections.as_ref().map_or(&[][..], Sections::copies);
        // The running chapter shows in part and chapter sections only.
        let chapters = sections.as_ref().is_some_and(Sections::chapter_seen);
        let bytes = template.finish(&body, sect_pr, copies, |field| match fields.get(field) {
            Some(runs) => Fill::Runs(runs.clone()),
            None if field == "chapter" && chapters => Fill::Keep,
            None => Fill::Remove,
        });
        Ok(Output { bytes, warnings })
    }
}

/// The document title split into the main title and subtitle, unless the
/// document has none or sets `notitle`.
struct Title {
    full: Vec<Inline>,
    main: Vec<Inline>,
    subtitle: Option<Vec<Inline>>,
}

fn header_title(doc: &Document) -> Option<Title> {
    let manpage = doc.attributes.get("doctype") == Some("manpage");
    if doc.attributes.get("notitle").is_some() || !(doc.has_title || manpage) {
        return None;
    }
    // A manual page without a title is named after its first section, like
    // Ruby's `doctitle`.
    let heading = if doc.has_title {
        doc.blocks.first()
    } else {
        doc.blocks.iter().find_map(|b| match &b.kind {
            BlockKind::Section { blocks, .. } => blocks.first(),
            _ => None,
        })
    };
    let BlockKind::Heading { inlines, .. } = &heading?.kind else {
        return None;
    };
    let separator = doc.attributes.get("title-separator").unwrap_or(":");
    let (mut main, subtitle) = writer::partition_title(inlines, separator);
    // A manual page's title, like Ruby's: `<title> Manual Page`.
    if manpage {
        let span = heading.map(|h| h.span).expect("a heading");
        if let Some(title) = doc.attributes.get("title") {
            main = vec![Inline::Text {
                value: title.to_string(),
                span,
            }];
        }
        main.push(Inline::Text {
            value: " Manual Page".into(),
            span,
        });
    }
    Some(Title {
        full: inlines.clone(),
        main,
        subtitle,
    })
}

/// Runs for each header field a template marker can name.
fn header_fields(
    cx: &mut Cx<'_>,
    doc: &Document,
    title: Option<&Title>,
) -> BTreeMap<&'static str, String> {
    let mut fields = BTreeMap::new();
    let mut put = |cx: &mut Cx<'_>, name: &'static str, f: &dyn Fn(&mut Runs)| {
        let mut runs = Runs::new(cx, Newlines::Space);
        f(&mut runs);
        if !runs.is_empty() {
            fields.insert(name, runs.finish());
        }
    };
    if let Some(title) = title {
        put(cx, "title", &|r| r.inlines(&title.main));
        if let Some(subtitle) = &title.subtitle {
            put(cx, "subtitle", &|r| r.inlines(subtitle));
        }
    }
    let authors = doc.authors();
    put(cx, "author", &|r| {
        for (i, author) in authors.iter().enumerate() {
            if i > 0 {
                r.text(", ");
            }
            r.parts(&author.name);
        }
    });
    if let Some(email) = authors.first().and_then(|a| a.email.as_ref()) {
        put(cx, "email", &|r| r.parts(email));
    }
    let attr = |name: &str| doc.attributes.get(name).filter(|v| !v.is_empty());
    for field in ["revnumber", "revdate", "revremark"] {
        if let Some(value) = attr(field) {
            put(cx, field, &|r| r.text(value));
        }
    }
    if let Some(line) = revision_line(doc) {
        put(cx, "revision", &|r| r.text(&line));
    }
    fields
}

fn core_properties(doc: &Document, title: Option<&Title>) -> CoreProperties {
    let attr = |name: &str| {
        doc.attributes
            .get(name)
            .filter(|v| !v.is_empty())
            .map(text::decode_references)
    };
    let authors: Vec<String> = doc
        .authors()
        .iter()
        .map(|a| inline::parts_text(&a.name))
        .filter(|n| !n.is_empty())
        .collect();
    let date = if doc.attributes.get("reproducible").is_some() {
        None
    } else {
        attr("docdatetime")
            .and_then(|d| props::w3cdtf(&d))
            .or_else(|| attr("docdate").and_then(|d| props::w3cdtf(&d)))
    };
    CoreProperties {
        title: title.map(|t| inline::plain_text(&t.full)),
        creator: (!authors.is_empty()).then(|| authors.join("; ")),
        subject: None,
        description: attr("description"),
        keywords: attr("keywords"),
        date,
    }
}

/// Check a custom template (`.docx`/`.dotx` bytes) against the style
/// contract: the warnings name each custom style the template lacks. Missing
/// Word built-ins are not reported; conversion adds them with Word's defaults.
pub fn check_template(bytes: &[u8]) -> Result<Vec<String>> {
    let mut template = Template::read(bytes).map_err(invalid_template)?;
    let kinds = [
        (styles::StyleKind::Paragraph, contract::PARAGRAPH),
        (styles::StyleKind::Character, contract::CHARACTER),
        (styles::StyleKind::Table, contract::TABLE),
    ];
    for (kind, names) in kinds {
        for name in names {
            template.styles.resolve(kind, name);
        }
    }
    Ok(template.styles.warnings)
}

fn invalid_template(message: String) -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("invalid docx template: {message}"),
    ))
}

/// The revision line of Ruby's HTML header: `version 1.2, 2026-01-02: Draft`.
fn revision_line(doc: &Document) -> Option<String> {
    let attr = |name: &str| doc.attributes.get(name).filter(|v| !v.is_empty());
    let number = attr("revnumber").map(|n| match attr("version-label") {
        Some(label) => format!("{} {n}", label.to_lowercase()),
        None => n.to_string(),
    });
    let date = attr("revdate");
    let mut line = match (number, date) {
        (Some(n), Some(d)) => format!("{n}, {d}"),
        (Some(n), None) => n,
        (None, Some(d)) => d.to_string(),
        (None, None) => return None,
    };
    if let Some(remark) = attr("revremark") {
        line.push_str(": ");
        line.push_str(remark);
    }
    Some(line)
}

impl Backend for DocxBackend {
    fn name(&self) -> &'static str {
        "docx"
    }

    fn extension(&self) -> &'static str {
        "docx"
    }

    fn convert(&self, doc: &Document) -> Result<Vec<u8>> {
        self.render(doc).map(|output| output.bytes)
    }
}

/// The text of a converted `.docx`: the main document's paragraphs (one per
/// line, tabs and line breaks kept) followed by its footnotes. Fields show
/// their results. For comparing a conversion's content with other outputs.
pub fn extract_text(bytes: &[u8]) -> Result<String> {
    let package = package::Package::read(bytes).map_err(invalid_template)?;
    let main = package
        .root_target(package::REL_OFFICE_DOCUMENT)
        .ok_or_else(|| invalid_template("no main document part".into()))?;
    let mut parts = vec![main.clone()];
    let rels = package::Relationships::parse(
        &package.text(&package::rels_name(&main)).unwrap_or_default(),
    );
    if let Some(target) = rels
        .target_of("http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes")
    {
        parts.push(package::resolve_target(&main, &target));
    }
    let mut out = String::new();
    for part in parts {
        let Some(text) = package.text(&part) else {
            continue;
        };
        let mut pos = 0;
        while let Some(tag) = xml::tags(&text, pos).next() {
            pos = tag.range.end;
            match (tag.name, tag.kind) {
                ("w:t", xml::TagKind::Start) => {
                    if let Some((range, content)) = xml::element(&text, &tag) {
                        out.push_str(&xml::unescape(&text[content]));
                        pos = range.end;
                    }
                }
                ("w:tab", xml::TagKind::Empty) => out.push('\t'),
                ("w:br", xml::TagKind::Empty) => out.push('\n'),
                ("w:p", xml::TagKind::End) => out.push('\n'),
                _ => {}
            }
        }
    }
    Ok(out)
}
