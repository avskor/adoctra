//! A `.docx` template: the package, its style registry and the places the
//! converted document goes.
//!
//! The template contract (`docs/docx.adoc`):
//!
//! * Styles are found by English name ([`crate::styles`]); the template's own
//!   formatting is used as is.
//! * A block-level content control (SDT) tagged `adoctra:body` marks where the
//!   body goes; it is replaced, everything around it (a cover page, sections,
//!   headers) stays. Without one, the template's body is cleared and the
//!   document goes before the final section properties.
//! * Content controls tagged `adoctra:<field>` (`title`, `author`…) are
//!   replaced by the field's text, keeping the paragraph properties of their
//!   first paragraph, or removed when the document has no such field.

use std::ops::Range;

use crate::numbering::Numbering;
use crate::package::{self, Package, Relationships};
use crate::styles::Styles;
use crate::xml;

const EMPTY_NUMBERING: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"></w:numbering>"#;
const REL_NUMBERING: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
const NUMBERING_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";

const REL_FOOTNOTES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
const FOOTNOTES_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";
/// A footnotes part with Word's separator footnotes.
const EMPTY_FOOTNOTES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:footnote></w:footnotes>"#;

const EMPTY_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"></w:styles>"#;

/// The tag prefix of the template's content-control markers.
pub const MARKER_PREFIX: &str = "adoctra:";
/// The marker of the body's place.
pub const BODY_MARKER: &str = "adoctra:body";
/// Relationship type of the core properties part.
pub const REL_CORE_PROPERTIES: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
const CORE_PROPERTIES_TYPE: &str = "application/vnd.openxmlformats-package.core-properties+xml";
const REL_HEADER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const REL_FOOTER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";

/// A template ready to receive a converted document.
#[derive(Debug, Clone)]
pub struct Template {
    package: Package,
    main: String,
    document: String,
    styles_part: String,
    styles_text: String,
    pub styles: Styles,
    pub rels: Relationships,
    /// The template's numbering part and its text, if it has one.
    numbering_part: Option<(String, String)>,
    pub numbering: Numbering,
    /// The template's footnotes part and its text, if it has one.
    footnotes_part: Option<(String, String)>,
    /// Added footnotes (`w:footnote` elements).
    footnotes: String,
    /// Added parts (media): name, bytes, extension, content type.
    media: Vec<(String, Vec<u8>, &'static str, &'static str)>,
}

/// What a marker is replaced by.
pub enum Fill {
    /// Runs (`<w:r>…`) for the marker's paragraph.
    Runs(String),
    /// Remove the marker.
    Remove,
}

impl Template {
    /// Read a `.docx` or `.dotx` template.
    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        Self::from_package(Package::read(bytes)?)
    }

    pub fn from_package(mut package: Package) -> Result<Self, String> {
        let main = package
            .root_target(package::REL_OFFICE_DOCUMENT)
            .ok_or("no main document part")?;
        let document = package
            .text(&main)
            .ok_or_else(|| format!("missing or non-UTF-8 part {main}"))?;
        if xml::find_element(&document, 0, "w:body").is_none() {
            return Err(format!("{main} has no w:body"));
        }
        let rels_name = package::rels_name(&main);
        let rels = Relationships::parse(&package.text(&rels_name).unwrap_or_default());
        let styles_part = rels
            .target_of(package::REL_STYLES)
            .map(|target| package::resolve_target(&main, &target));
        let (styles_part, styles_text, rels) = match styles_part {
            Some(part) => {
                let text = package
                    .text(&part)
                    .unwrap_or_else(|| EMPTY_STYLES.to_string());
                (part, text, rels)
            }
            None => {
                let part = package::resolve_target(&main, "styles.xml");
                let mut rels = rels;
                rels.add(package::REL_STYLES, "styles.xml", false);
                package.set_override_type(&part, package::STYLES_CONTENT_TYPE);
                (part, EMPTY_STYLES.to_string(), rels)
            }
        };
        // A `.dotx` main part becomes a document.
        package.set_override_type(&main, package::MAIN_CONTENT_TYPE);
        let numbering_part = rels.target_of(REL_NUMBERING).map(|target| {
            let part = package::resolve_target(&main, &target);
            let text = package
                .text(&part)
                .unwrap_or_else(|| EMPTY_NUMBERING.to_string());
            (part, text)
        });
        let numbering = Numbering::parse(numbering_part.as_ref().map_or("", |(_, t)| t));
        let footnotes_part = rels.target_of(REL_FOOTNOTES).map(|target| {
            let part = package::resolve_target(&main, &target);
            let text = package
                .text(&part)
                .unwrap_or_else(|| EMPTY_FOOTNOTES.to_string());
            (part, text)
        });
        Ok(Self {
            footnotes_part,
            footnotes: String::new(),
            media: Vec::new(),
            numbering_part,
            numbering,
            styles: Styles::parse(&styles_text),
            package,
            main,
            document,
            styles_part,
            styles_text,
            rels,
        })
    }

    /// The markers of the template (`adoctra:*` tags of content controls),
    /// in document order.
    #[cfg(test)]
    pub fn markers(&self) -> Vec<String> {
        markers(&self.document).into_iter().map(|m| m.tag).collect()
    }

    /// Whether the main document has the marker of `field` (`title`…).
    /// The style registry and the main part's relationships, to write the
    /// body with.
    pub fn parts_mut(&mut self) -> (&mut Styles, &mut Relationships, &mut Numbering) {
        (&mut self.styles, &mut self.rels, &mut self.numbering)
    }

    /// The first id free for a footnote (after the template's).
    pub fn first_footnote_id(&self) -> u32 {
        let text = self.footnotes_part.as_ref().map_or("", |(_, t)| t);
        xml::tags(text, 0)
            .filter(|t| t.name == "w:footnote" && t.opens())
            .filter_map(|t| t.attr("w:id")?.parse::<i64>().ok())
            .max()
            .map_or(1, |max| u32::try_from(max + 1).unwrap_or(1).max(1))
    }

    /// Footnotes (`w:footnote` elements) to add to the footnotes part.
    pub fn add_footnotes(&mut self, footnotes: String) {
        self.footnotes = footnotes;
    }

    /// Media parts to add.
    pub fn add_media(&mut self, parts: Vec<(String, Vec<u8>, &'static str, &'static str)>) {
        self.media = parts;
    }

    pub fn has_marker(&self, field: &str) -> bool {
        let tag = format!("{MARKER_PREFIX}{field}");
        markers(&self.document).iter().any(|m| m.tag == tag)
    }

    /// The body's final section properties (`w:sectPr`), if any.
    pub fn body_sect_pr(&self) -> Option<String> {
        final_sect_pr(&self.document).map(|range| self.document[range].to_string())
    }

    /// Replace the package's core properties (`docProps/core.xml`).
    pub fn set_core_properties(&mut self, xml_text: &str) {
        let mut rels = Relationships::parse(&self.package.text("_rels/.rels").unwrap_or_default());
        let part = match rels.target_of(REL_CORE_PROPERTIES) {
            Some(target) => package::resolve_target("", &target),
            None => {
                rels.add(REL_CORE_PROPERTIES, "docProps/core.xml", false);
                self.package.put("_rels/.rels", rels.to_xml());
                "docProps/core.xml".to_string()
            }
        };
        self.package.put(&part, xml_text);
        self.package.set_override_type(&part, CORE_PROPERTIES_TYPE);
    }

    /// Put `body` (block-level markup) in place, fill the other markers of
    /// the document and its headers and footers, replace the final section
    /// properties with `sect_pr_override` if given, and write the package.
    pub fn finish(
        mut self,
        body: &str,
        sect_pr_override: Option<String>,
        mut fill: impl FnMut(&str) -> Fill,
    ) -> Vec<u8> {
        let mut document = place(&self.document, Some(body), &mut fill);
        if let (Some(sect_pr), Some(range)) = (sect_pr_override, final_sect_pr(&document)) {
            document.replace_range(range, &sect_pr);
        }
        for (prefix, uri) in [
            (
                "r",
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
            ),
            (
                "wp",
                "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
            ),
        ] {
            if document.contains(&format!("{prefix}:")) {
                document = declare_namespace(&document, prefix, uri);
            }
        }
        for (name, bytes, ext, content_type) in std::mem::take(&mut self.media) {
            self.package.put(&name, bytes);
            self.package.ensure_default_type(ext, content_type);
        }
        if !self.footnotes.is_empty() {
            let (part, text) = match self.footnotes_part.take() {
                Some(existing) => existing,
                None => {
                    let part = package::resolve_target(&self.main, "footnotes.xml");
                    self.rels.add(REL_FOOTNOTES, "footnotes.xml", false);
                    self.package
                        .set_override_type(&part, FOOTNOTES_CONTENT_TYPE);
                    (part, EMPTY_FOOTNOTES.to_string())
                }
            };
            let mut text = package::insert_before_close(&text, "w:footnotes", &self.footnotes);
            if text.contains("r:id=\"") || text.contains("r:embed=\"") {
                text = declare_namespace(
                    &text,
                    "r",
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                );
            }
            self.package.put(&part, text);
        }
        self.package.put(&self.main, document);
        let parts: Vec<String> = self
            .rels
            .entries
            .iter()
            .filter(|r| !r.external && (r.rel_type == REL_HEADER || r.rel_type == REL_FOOTER))
            .map(|r| package::resolve_target(&self.main, &r.target))
            .collect();
        for part in parts {
            if let Some(text) = self.package.text(&part) {
                if !markers(&text).is_empty() {
                    let filled = place(&text, None, &mut fill);
                    self.package.put(&part, filled);
                }
            }
        }
        self.package
            .put(&self.styles_part, self.styles.apply(&self.styles_text));
        if !self.numbering.is_empty() {
            let (part, text) = match self.numbering_part.take() {
                Some(existing) => existing,
                None => {
                    let part = package::resolve_target(&self.main, "numbering.xml");
                    self.rels.add(REL_NUMBERING, "numbering.xml", false);
                    self.package
                        .set_override_type(&part, NUMBERING_CONTENT_TYPE);
                    (part, EMPTY_NUMBERING.to_string())
                }
            };
            self.package.put(&part, self.numbering.apply(&text));
        }
        self.package
            .put(&package::rels_name(&self.main), self.rels.to_xml());
        self.package.write()
    }
}

/// A marker content control.
struct Marker {
    tag: String,
    range: Range<usize>,
    content: Range<usize>,
}

/// The outermost `adoctra:*` content controls of `doc`.
fn markers(doc: &str) -> Vec<Marker> {
    let mut found = Vec::new();
    let mut pos = 0;
    while let Some(open) = xml::tags(doc, pos).find(|t| t.name == "w:sdt" && t.opens()) {
        let Some((range, inner)) = xml::element(doc, &open) else {
            break;
        };
        let tag =
            xml::find_element(&doc[..inner.end], inner.start, "w:sdtPr").and_then(|(_, pr)| {
                xml::tags(&doc[..pr.end], pr.start)
                    .find(|t| t.name == "w:tag" && t.opens())
                    .and_then(|t| t.attr("w:val"))
            });
        match tag.filter(|t| t.starts_with(MARKER_PREFIX)) {
            Some(tag) => {
                let content = xml::find_element(&doc[..inner.end], inner.start, "w:sdtContent")
                    .map_or(inner.end..inner.end, |(_, c)| c);
                found.push(Marker {
                    tag,
                    range: range.clone(),
                    content,
                });
                pos = range.end;
            }
            // Look inside other content controls.
            None => pos = open.range.end,
        }
    }
    found
}

/// The range of the final `w:sectPr` of the body.
fn final_sect_pr(doc: &str) -> Option<Range<usize>> {
    let (_, content) = xml::find_element(doc, 0, "w:body")?;
    xml::children(doc, content)
        .into_iter()
        .next_back()
        .filter(|(tag, _)| tag.name == "w:sectPr")
        .map(|(_, range)| range)
}

/// Declare namespace `prefix` on the root element (a template whose
/// document had no relationships or drawings may lack it).
fn declare_namespace(doc: &str, prefix: &str, uri: &str) -> String {
    let Some(root) = xml::tags(doc, 0).next() else {
        return doc.to_string();
    };
    if root.attr(&format!("xmlns:{prefix}")).is_some() {
        return doc.to_string();
    }
    let at = root.range.start + 1 + root.name.len();
    format!(r#"{} xmlns:{prefix}="{uri}"{}"#, &doc[..at], &doc[at..])
}

/// Fill the markers of `doc`: the body marker with `body` (a header or
/// footer has none to give and drops it), the others with `fill`.
fn place(doc: &str, body: Option<&str>, fill: &mut dyn FnMut(&str) -> Fill) -> String {
    let found = markers(doc);
    if let (Some(body), false) = (body, found.iter().any(|m| m.tag == BODY_MARKER)) {
        // No marker: replace the whole body except the final section
        // properties.
        let (_, content) = xml::find_element(doc, 0, "w:body").expect("checked on read");
        let end = xml::children(doc, content.clone())
            .into_iter()
            .rev()
            .find(|(tag, _)| tag.name == "w:sectPr")
            .map_or(content.end, |(_, range)| range.start);
        return format!("{}{body}{}", &doc[..content.start], &doc[end..]);
    }
    let mut out = String::with_capacity(doc.len() + body.map_or(0, str::len));
    let mut pos = 0;
    for marker in &found {
        out.push_str(&doc[pos..marker.range.start]);
        pos = marker.range.end;
        if marker.tag == BODY_MARKER {
            out.push_str(body.unwrap_or_default());
            continue;
        }
        let Fill::Runs(runs) = fill(&marker.tag[MARKER_PREFIX.len()..]) else {
            continue;
        };
        let kids = xml::children(doc, marker.content.clone());
        match kids.iter().find(|(tag, _)| tag.name == "w:p") {
            // A block-level control: its first paragraph with the text.
            Some((p, _)) => {
                let open = &doc[p.range.clone()];
                let (open, ppr) = match p.kind {
                    xml::TagKind::Empty => (format!("{}>", open[..open.len() - 2].trim_end()), ""),
                    _ => {
                        let (_, content) = xml::element(doc, p).expect("listed as a child");
                        let ppr = xml::children(doc, content)
                            .into_iter()
                            .next()
                            .filter(|(tag, _)| tag.name == "w:pPr")
                            .map_or("", |(_, r)| &doc[r]);
                        (open.to_string(), ppr)
                    }
                };
                out.push_str(&open);
                out.push_str(ppr);
                out.push_str(&runs);
                out.push_str("</w:p>");
            }
            // A run-level control inside a paragraph.
            None => out.push_str(&runs),
        }
    }
    out.push_str(&doc[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn sdt(tag: &str, content: &str) -> String {
        format!(
            r#"<w:sdt><w:sdtPr><w:alias w:val="x"/><w:tag w:val="{tag}"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>"#
        )
    }

    fn package(document: &str, with_styles: bool) -> Package {
        let mut p = Package::default();
        p.put(
            "[Content_Types].xml",
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml"/></Types>"#,
        );
        p.put(
            "_rels/.rels",
            r#"<Relationships xmlns="r"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        );
        p.put("word/document.xml", document);
        if with_styles {
            p.put(
                "word/_rels/document.xml.rels",
                r#"<Relationships xmlns="r"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
            );
            p.put(
                "word/styles.xml",
                format!(r#"<w:styles {W}><w:style w:type="paragraph" w:styleId="a"><w:name w:val="Normal"/></w:style></w:styles>"#),
            );
        }
        p
    }

    fn finish(document: &str, fill: impl FnMut(&str) -> Fill) -> Package {
        let template = Template::from_package(package(document, true)).unwrap();
        Package::read(&template.finish("<w:p>BODY</w:p>", None, fill)).unwrap()
    }

    #[test]
    fn replaces_the_body_marker_and_keeps_the_rest() {
        let doc = format!(
            r#"<w:document {W}><w:body><w:p>cover</w:p>{}<w:p>end</w:p><w:sectPr/></w:body></w:document>"#,
            sdt("adoctra:body", "<w:p>sample</w:p>")
        );
        let out = finish(&doc, |_| Fill::Remove)
            .text("word/document.xml")
            .unwrap();
        assert_eq!(
            out,
            format!(
                r#"<w:document {W}><w:body><w:p>cover</w:p><w:p>BODY</w:p><w:p>end</w:p><w:sectPr/></w:body></w:document>"#
            )
        );
    }

    #[test]
    fn without_a_marker_the_body_is_replaced_before_the_final_section() {
        let doc = format!(
            r#"<w:document {W}><w:body><w:p>specimen</w:p><w:p><w:pPr><w:sectPr/></w:pPr></w:p><w:sectPr><w:pgSz/></w:sectPr></w:body></w:document>"#
        );
        let out = finish(&doc, |_| Fill::Remove)
            .text("word/document.xml")
            .unwrap();
        assert_eq!(
            out,
            format!(
                r#"<w:document {W}><w:body><w:p>BODY</w:p><w:sectPr><w:pgSz/></w:sectPr></w:body></w:document>"#
            )
        );
    }

    #[test]
    fn fills_field_markers_keeping_paragraph_properties() {
        let doc = format!(
            r#"<w:document {W}><w:body>{}{}<w:p>by {}</w:p>{}<w:sectPr/></w:body></w:document>"#,
            sdt(
                "adoctra:title",
                r#"<w:p w:rsidR="1"><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p><w:p>second</w:p>"#
            ),
            sdt("adoctra:subtitle", "<w:p><w:r><w:t>Sub</w:t></w:r></w:p>"),
            sdt("adoctra:author", "<w:r><w:t>Name</w:t></w:r>"),
            sdt("adoctra:body", "<w:p/>"),
        );
        let mut asked = Vec::new();
        let out = finish(&doc, |field| {
            asked.push(field.to_string());
            match field {
                "title" => Fill::Runs("<w:r><w:t>Doc</w:t></w:r>".into()),
                "author" => Fill::Runs("<w:r><w:t>Ann</w:t></w:r>".into()),
                _ => Fill::Remove,
            }
        })
        .text("word/document.xml")
        .unwrap();
        assert_eq!(asked, ["title", "subtitle", "author"]);
        assert_eq!(
            out,
            format!(
                r#"<w:document {W}><w:body><w:p w:rsidR="1"><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Doc</w:t></w:r></w:p><w:p>by <w:r><w:t>Ann</w:t></w:r></w:p><w:p>BODY</w:p><w:sectPr/></w:body></w:document>"#
            )
        );
    }

    #[test]
    fn looks_for_markers_inside_other_content_controls() {
        let doc = format!(
            r#"<w:document {W}><w:body>{}<w:sectPr/></w:body></w:document>"#,
            sdt("cover", &sdt("adoctra:body", "<w:p/>"))
        );
        let template = Template::from_package(package(&doc, true)).unwrap();
        assert_eq!(template.markers(), ["adoctra:body"]);
    }

    #[test]
    fn a_template_without_styles_gets_a_styles_part() {
        let doc = format!(r#"<w:document {W}><w:body/></w:document>"#);
        let mut template = Template::from_package(package(&doc, false)).unwrap();
        assert_eq!(
            template
                .styles
                .resolve(crate::styles::StyleKind::Paragraph, "Heading 1")
                .as_deref(),
            Some("Heading1")
        );
        let out = Package::read(&template.finish("<w:p/>", None, |_| Fill::Remove)).unwrap();
        let styles = out.text("word/styles.xml").unwrap();
        assert!(styles.contains(r#"w:styleId="Heading1""#));
        assert!(styles.contains(r#"w:styleId="Normal""#));
        assert!(out
            .text("word/_rels/document.xml.rels")
            .unwrap()
            .contains("Target=\"styles.xml\""));
        let types = out.text("[Content_Types].xml").unwrap();
        assert!(types.contains(package::STYLES_CONTENT_TYPE));
        // The `.dotx` main part is declared a document.
        assert!(types.contains(package::MAIN_CONTENT_TYPE));
        assert!(!types.contains("template.main"));
    }

    #[test]
    fn fills_markers_in_headers_and_replaces_the_final_section() {
        let doc = format!(
            r#"<w:document {W}><w:body>{}<w:sectPr><w:headerReference r:id="rId9"/><w:pgNumType w:start="1"/></w:sectPr></w:body></w:document>"#,
            sdt("adoctra:body", "<w:p/>")
        );
        let mut p = package(&doc, true);
        p.put(
            "word/_rels/document.xml.rels",
            r#"<Relationships xmlns="r"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/></Relationships>"#,
        );
        p.put(
            "word/header1.xml",
            format!(
                r#"<w:hdr {W}><w:p>{}<w:r><w:t>x</w:t></w:r>{}</w:p></w:hdr>"#,
                sdt("adoctra:title", "<w:r><w:t>T</w:t></w:r>"),
                sdt("adoctra:body", "<w:p/>")
            ),
        );
        let template = Template::from_package(p).unwrap();
        assert!(template.has_marker("body"));
        assert!(!template.has_marker("title"));
        assert_eq!(
            template.body_sect_pr().unwrap(),
            r#"<w:sectPr><w:headerReference r:id="rId9"/><w:pgNumType w:start="1"/></w:sectPr>"#
        );
        let out = Package::read(&template.finish(
            "<w:p>BODY</w:p>",
            Some("<w:sectPr/>".into()),
            |field| match field {
                "title" => Fill::Runs("<w:r><w:t>Doc</w:t></w:r>".into()),
                _ => Fill::Remove,
            },
        ))
        .unwrap();
        assert_eq!(
            out.text("word/header1.xml").unwrap(),
            format!(
                r#"<w:hdr {W}><w:p><w:r><w:t>Doc</w:t></w:r><w:r><w:t>x</w:t></w:r></w:p></w:hdr>"#
            )
        );
        let document = out.text("word/document.xml").unwrap();
        assert!(
            document.ends_with("<w:p>BODY</w:p><w:sectPr/></w:body></w:document>"),
            "{document}"
        );
    }

    #[test]
    fn declares_the_relationships_namespace_for_links() {
        let doc = format!(r#"<w:document {W}><w:body/></w:document>"#);
        let template = Template::from_package(package(&doc, true)).unwrap();
        let out = Package::read(&template.finish(
            r#"<w:p><w:hyperlink r:id="rId5"/></w:p>"#,
            None,
            |_| Fill::Remove,
        ))
        .unwrap();
        assert!(out
            .text("word/document.xml")
            .unwrap()
            .starts_with(r#"<w:document xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:w="#));
    }

    #[test]
    fn writes_core_properties_once() {
        let doc = format!(r#"<w:document {W}><w:body/></w:document>"#);
        let mut template = Template::from_package(package(&doc, true)).unwrap();
        template.set_core_properties("<cp:coreProperties/>");
        template.set_core_properties("<cp:coreProperties>2</cp:coreProperties>");
        let out = Package::read(&template.finish("", None, |_| Fill::Remove)).unwrap();
        assert_eq!(
            out.text("docProps/core.xml").unwrap(),
            "<cp:coreProperties>2</cp:coreProperties>"
        );
        let rels = out.text("_rels/.rels").unwrap();
        assert_eq!(rels.matches("core-properties").count(), 1);
        assert!(out
            .text("[Content_Types].xml")
            .unwrap()
            .contains(r#"PartName="/docProps/core.xml""#));
    }

    #[test]
    fn rejects_packages_without_a_document() {
        let mut p = package("<w:document/>", true);
        assert!(Template::from_package(p.clone())
            .unwrap_err()
            .contains("w:body"));
        p.put("_rels/.rels", "<Relationships/>");
        assert!(Template::from_package(p).is_err());
    }
}
