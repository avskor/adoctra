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
mod package;
mod styles;
mod template;
mod xml;

use std::sync::Arc;

use adoctra_core::{Backend, Document, Error, Result};

pub use builtin::Builtin;
use template::{Fill, Template};

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
        let template = match &self.template {
            Some(bytes) => Template::read(bytes).map_err(invalid_template)?,
            None => Template::from_package(
                Builtin::for_doctype(doc.attributes.get("doctype")).package(),
            )
            .expect("built-in templates are valid"),
        };
        let mut warnings = Vec::new();
        let content = doc.blocks.len() - usize::from(doc.has_title);
        if content > 0 {
            warnings.push("docx backend does not convert block content yet".to_string());
        }
        let styles_warnings = template.styles.warnings.clone();
        let bytes = template.finish("", |field| header_field(doc, field));
        warnings.extend(styles_warnings);
        Ok(Output { bytes, warnings })
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

/// The text of the header field a template marker names.
fn header_field(doc: &Document, field: &str) -> Fill {
    let attr = |name: &str| doc.attributes.get(name).filter(|v| !v.is_empty());
    let text = match field {
        "title" if doc.has_title => attr("doctitle").map(str::to_string),
        "author" => attr("authors").map(str::to_string),
        "email" => attr("email").map(str::to_string),
        "revnumber" => attr("revnumber").map(str::to_string),
        "revdate" => attr("revdate").map(str::to_string),
        "revremark" => attr("revremark").map(str::to_string),
        "revision" => revision_line(doc),
        _ => None,
    };
    match text {
        Some(text) => Fill::Runs(run(&text)),
        None => Fill::Remove,
    }
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

fn run(text: &str) -> String {
    format!(
        r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
        xml::escape_text(text)
    )
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
