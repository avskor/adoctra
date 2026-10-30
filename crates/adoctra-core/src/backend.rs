//! The [`Backend`] trait: a render target for a parsed [`Document`].
//!
//! Each output format (HTML, DOCX, JSON, ...) lives in its own crate and
//! implements this trait. Third parties can add formats the same way without
//! touching the core — that's the whole point of keeping this trait here and
//! depending only on the document model.

use crate::ast::Document;
use crate::attributes::Attributes;
use crate::error::Result;

pub trait Backend {
    /// Stable identifier used on the CLI (`--to <name>`).
    fn name(&self) -> &'static str;

    /// Conventional file extension for this backend's output.
    fn extension(&self) -> &'static str;

    /// Render a document to bytes (text formats return UTF-8 bytes).
    fn convert(&self, doc: &Document) -> Result<Vec<u8>>;
}

/// A converter's work at the start of conversion. Ruby's `convert_document`
/// builds the head (reading a custom stylesheet, which can also activate the
/// URI cache) after converting the document title and before the header
/// outline and the body convert, so its reads and warnings precede those of
/// the body's images. Core converts the body while parsing, so a converter
/// that reads resources in its head passes this hook to
/// [`parse_resource_with_start`](crate::parse_resource_with_start) or
/// [`parse_with_start`](crate::parse_with_start); core calls it once at that
/// point and reports the returned warnings as conversion diagnostics there.
pub trait ConversionStart {
    /// Called with the document's attributes as conversion starts (the
    /// header attributes, like Ruby's converter sees them). Returns warnings.
    fn start(&mut self, attributes: &Attributes) -> Vec<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder(Vec<(Option<String>, Option<String>)>);

    impl ConversionStart for Recorder {
        fn start(&mut self, attributes: &Attributes) -> Vec<String> {
            self.0.push((
                attributes.get("stylesheet").map(str::to_owned),
                attributes.get("body-entry").map(str::to_owned),
            ));
            vec!["head warning".into()]
        }
    }

    #[test]
    fn conversion_start_runs_once_with_the_header_attributes() {
        // Once per document (not for an AsciiDoc cell's nested document),
        // with the header attributes: a body entry is not set yet.
        let source =
            "= T\n:stylesheet: a.css\n\n:body-entry: x\n\n|===\na|\n= Inner\n\ncell\n|===\n";
        let mut recorder = Recorder::default();
        let result = crate::parse_with_start(source, &Default::default(), &mut recorder);
        assert_eq!(recorder.0, [(Some("a.css".to_owned()), None)]);
        // Its warnings are conversion diagnostics without a source cursor.
        let warning = result
            .diagnostics
            .iter()
            .find(|d| d.message == "head warning")
            .expect("start warning");
        assert_eq!(warning.severity, crate::Severity::Warning);
        assert_eq!(
            warning.report.location,
            crate::DiagnosticLocation::Unlocated
        );
        // The document is the same as without the hook.
        assert_eq!(
            result.document,
            crate::parse_with_options(source, &Default::default()).document
        );
    }
}
