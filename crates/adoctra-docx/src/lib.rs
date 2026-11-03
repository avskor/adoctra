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
mod inline;
mod numbering;
mod package;
mod props;
mod styles;
mod table;
mod template;
mod text;
mod writer;
mod xml;

use std::collections::BTreeMap;
use std::sync::Arc;

use adoctra_core::ast::BlockKind;
use adoctra_core::{Backend, Document, Error, Inline, Result};

pub use builtin::Builtin;
use inline::{Cx, Newlines, Runs};
use props::CoreProperties;
use template::{Fill, Template};
use writer::{ChapterSections, Writer};

/// The DOCX backend.
#[derive(Debug, Default, Clone)]
pub struct DocxBackend {
    template: Option<Arc<[u8]>>,
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

    /// Convert and report the warnings.
    pub fn render(&self, doc: &Document) -> Result<Output> {
        let mut template = match &self.template {
            Some(bytes) => Template::read(bytes).map_err(invalid_template)?,
            None => Template::from_package(
                Builtin::for_doctype(doc.attributes.get("doctype")).package(),
            )
            .expect("built-in templates are valid"),
        };
        let title = header_title(doc);
        let title_marker = template.has_marker("title");
        let mut chapters = (doc.attributes.get("doctype") == Some("book"))
            .then(|| template.body_sect_pr())
            .flatten()
            .filter(|sect_pr| sect_pr.contains("<w:titlePg/>") || sect_pr.contains("<w:titlePg "))
            .map(ChapterSections::new);
        let text_width = template
            .body_sect_pr()
            .map_or(writer::DEFAULT_TEXT_WIDTH, |s| writer::text_width(&s));
        let (styles, rels, numbering) = template.parts_mut();
        let mut cx = Cx {
            styles,
            rels,
            numbering,
            bookmarks: Default::default(),
            warnings: Default::default(),
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
        let body = Writer::new(&mut cx, doc, chapters.as_mut(), text_width).body(lead);
        let mut warnings = std::mem::take(&mut cx.warnings.list);
        warnings.extend(template.styles.warnings.iter().cloned());
        template.set_core_properties(&core_properties(doc, title.as_ref()).to_xml());
        let sect_pr = chapters.and_then(|c| c.final_sect_pr());
        let bytes = template.finish(&body, sect_pr, |field| match fields.get(field) {
            Some(runs) => Fill::Runs(runs.clone()),
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
    if !doc.has_title || doc.attributes.get("notitle").is_some() {
        return None;
    }
    let BlockKind::Heading { inlines, .. } = &doc.blocks.first()?.kind else {
        return None;
    };
    let separator = doc.attributes.get("title-separator").unwrap_or(":");
    let (main, subtitle) = writer::partition_title(inlines, separator);
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
