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

    /// Called before [`start`](Self::start) with Ruby's base directory of the
    /// document ([`crate::Document::base_dir`]), from which a converter
    /// resolves the assets it reads as conversion starts (a stylesheet).
    fn base_dir(&mut self, dir: Option<&str>) {
        let _ = dir;
    }

    /// Ruby's `highlight` substitution: a `source` listing whose custom
    /// `subs` keep `specialcharacters` is highlighted by a server-side syntax
    /// highlighter in their place, and the later stages run over its HTML
    /// (`highlight_source`). Core extracts and restores the callouts; the
    /// converter returns the highlighted HTML and, when it wraps the code
    /// (a line number table), the offset where the code starts. `None`
    /// keeps Ruby's `sub_source` (special characters, then callouts).
    fn highlight(&mut self, request: &HighlightRequest<'_>) -> Option<Highlighted> {
        let _ = request;
        None
    }
}

/// What a [`ConversionStart::highlight`] call highlights.
#[derive(Debug)]
pub struct HighlightRequest<'a> {
    /// The text the earlier stages produced, without callout marks.
    pub source: &'a str,
    /// The listing's named attributes (`language`, `linenums`, `start`,
    /// `highlight` …) and options.
    pub named: &'a std::collections::BTreeMap<String, String>,
    pub options: &'a [String],
    /// The document attributes where the listing converts.
    pub attributes: &'a Attributes,
}

/// The highlighted HTML of a [`HighlightRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlighted {
    pub html: String,
    /// Where the code starts in `html` (Ruby's `source_offset`), if not 0.
    pub source_offset: Option<usize>,
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

    /// Wraps every word in a span, like a highlighter, and records requests.
    #[derive(Default)]
    struct Spans(Vec<(String, Option<String>, Option<String>)>);

    impl ConversionStart for Spans {
        fn start(&mut self, _: &Attributes) -> Vec<String> {
            Vec::new()
        }

        fn highlight(&mut self, request: &HighlightRequest<'_>) -> Option<Highlighted> {
            self.0.push((
                request.source.to_owned(),
                request.named.get("language").cloned(),
                request.attributes.get("x").map(str::to_owned),
            ));
            let escaped = request
                .source
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            Some(Highlighted {
                html: format!("<span class=\"k\">{escaped}</span>"),
                source_offset: None,
            })
        }
    }

    fn listing_parts(result: &crate::ParseResult) -> Vec<crate::AttributePart> {
        let crate::BlockKind::Listing {
            inlines: Some(inlines),
            ..
        } = &result.document.blocks[0].kind
        else {
            panic!("listing with substituted content");
        };
        let [crate::Inline::Substituted { parts, .. }] = inlines.as_slice() else {
            panic!("one substituted node");
        };
        parts.clone()
    }

    #[test]
    fn highlight_replaces_special_characters_of_a_source_listing() {
        use crate::AttributePart as P;
        // `attributes+` prepends the stage: the attribute expands first.
        let source = ":x: <v>\n\n[source,java,subs=\"attributes+\"]\n----\na < {x} // <1>\n----\n";
        let mut spans = Spans::default();
        let result = crate::parse_with_start(source, &Default::default(), &mut spans);
        // The earlier attributes stage feeds the highlighter Ruby's string
        // (the stored value is escaped), without the callout mark.
        assert_eq!(
            spans.0,
            [(
                "a < &lt;v&gt; \n".to_owned(),
                Some("java".to_owned()),
                Some("<v>".to_owned())
            )]
        );
        let parts = listing_parts(&result);
        // Tags are raw, text decoded; the mark follows its line.
        assert_eq!(parts[0], P::Raw("<span class=\"k\">".into()));
        assert_eq!(parts[1], P::Text("a < &lt;v&gt; ".into()));
        assert!(
            matches!(&parts[2], P::Callout(c) if c.number == "1" && c.guard.as_deref() == Some("// ") && c.id.as_deref() == Some("CO1-1"))
        );
    }

    #[test]
    fn without_a_highlighter_custom_subs_substitute_special_characters() {
        use crate::AttributePart as P;
        // A converter that does not highlight (and plain parsing) keeps
        // Ruby's `sub_source`; a non-source listing is never highlighted.
        let source = "[source,java,subs=\"+attributes\"]\n----\na < b\n----\n";
        let plain = crate::parse_with_options(source, &Default::default());
        assert_eq!(listing_parts(&plain), [P::Text("a < b".into())]);
        let mut recorder = Recorder::default();
        let hooked = crate::parse_with_start(source, &Default::default(), &mut recorder);
        assert_eq!(hooked.document, plain.document);
        let mut spans = Spans::default();
        let listing = "[listing,subs=\"+attributes\"]\n----\na < b\n----\n";
        crate::parse_with_start(listing, &Default::default(), &mut spans);
        assert!(spans.0.is_empty());
    }
}
