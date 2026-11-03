//! The OPC package: a `.docx` read into named parts and written back as a
//! deterministic zip.
//!
//! Parts are kept as bytes. Only the parts the converter changes are touched;
//! everything else in a template (theme, fonts, headers, custom XML) is
//! copied unchanged.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use crate::xml;

/// Content type of the main document part.
pub const MAIN_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
/// Content type of the styles part.
pub const STYLES_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml";
/// Relationship type of the main document part.
pub const REL_OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
/// Relationship type of the styles part.
pub const REL_STYLES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";

const CONTENT_TYPES: &str = "[Content_Types].xml";

/// The largest part a template may hold; guards against zip bombs.
const MAX_PART: u64 = 256 * 1024 * 1024;

/// A package's parts by name (no leading slash), in name order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Package {
    parts: BTreeMap<String, Vec<u8>>,
}

impl Package {
    /// Read a `.docx` (any OPC zip).
    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a zip file: {e}"))?;
        let mut parts = BTreeMap::new();
        for index in 0..archive.len() {
            let mut file = archive
                .by_index(index)
                .map_err(|e| format!("unreadable zip entry: {e}"))?;
            if file.is_dir() {
                continue;
            }
            if file.size() > MAX_PART {
                return Err(format!("zip entry too large: {}", file.name()));
            }
            let name = file.name().trim_start_matches('/').to_string();
            let mut data = Vec::with_capacity(file.size() as usize);
            file.by_ref()
                .take(MAX_PART + 1)
                .read_to_end(&mut data)
                .map_err(|e| format!("unreadable zip entry {name}: {e}"))?;
            parts.insert(name, data);
        }
        if !parts.contains_key(CONTENT_TYPES) {
            return Err(format!("not an OPC package: no {CONTENT_TYPES}"));
        }
        Ok(Self { parts })
    }

    /// Write the package: `[Content_Types].xml` first, then the parts in name
    /// order, deflated, with fixed timestamps and permissions so the same
    /// input always gives the same bytes.
    pub fn write(&self) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o644);
        let ordered = self
            .parts
            .get_key_value(CONTENT_TYPES)
            .into_iter()
            .chain(self.parts.iter().filter(|(name, _)| *name != CONTENT_TYPES));
        for (name, data) in ordered {
            // Writing into memory cannot fail.
            zip.start_file(name.as_str(), options)
                .and_then(|()| zip.write_all(data).map_err(Into::into))
                .expect("in-memory zip write");
        }
        zip.finish().expect("in-memory zip write").into_inner()
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.parts.get(name).map(Vec::as_slice)
    }

    /// A part as UTF-8 text (OOXML parts are UTF-8 XML; a stray BOM is
    /// dropped).
    pub fn text(&self, name: &str) -> Option<String> {
        let bytes = self.get(name)?;
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        String::from_utf8(bytes.to_vec()).ok()
    }

    pub fn put(&mut self, name: &str, data: impl Into<Vec<u8>>) {
        self.parts.insert(name.to_string(), data.into());
    }

    #[cfg(test)]
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.parts.keys().map(String::as_str)
    }

    /// The target of the package-level relationship of `rel_type`, as a part
    /// name.
    pub fn root_target(&self, rel_type: &str) -> Option<String> {
        let rels = Relationships::parse(&self.text("_rels/.rels")?);
        rels.target_of(rel_type)
            .map(|target| resolve_target("", &target))
    }

    /// Declare `ext` (without the dot) with `content_type` unless an entry
    /// for it exists (media parts).
    pub fn ensure_default_type(&mut self, ext: &str, content_type: &str) {
        let types = self.text(CONTENT_TYPES).unwrap_or_default();
        let exists = xml::tags(&types, 0).any(|tag| {
            tag.name == "Default"
                && tag
                    .attr("Extension")
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        });
        if !exists {
            let entry = format!(
                r#"<Default Extension="{}" ContentType="{}"/>"#,
                xml::escape_attr(ext),
                xml::escape_attr(content_type)
            );
            self.put(CONTENT_TYPES, insert_before_close(&types, "Types", &entry));
        }
    }

    /// Declare part `name` with `content_type`, replacing an override for it.
    pub fn set_override_type(&mut self, name: &str, content_type: &str) {
        let mut types = self.text(CONTENT_TYPES).unwrap_or_default();
        let part = format!("/{name}");
        let existing = xml::tags(&types, 0)
            .find(|tag| {
                tag.name == "Override"
                    && tag
                        .attr("PartName")
                        .is_some_and(|p| p.eq_ignore_ascii_case(&part))
            })
            .map(|tag| tag.range);
        if let Some(range) = existing {
            types.replace_range(range, "");
        }
        let entry = format!(
            r#"<Override PartName="{}" ContentType="{}"/>"#,
            xml::escape_attr(&part),
            xml::escape_attr(content_type)
        );
        self.put(CONTENT_TYPES, insert_before_close(&types, "Types", &entry));
    }
}

/// Insert `markup` before the closing tag of the root element `root`
/// (expanding a self-closing root).
pub fn insert_before_close(xml_text: &str, root: &str, markup: &str) -> String {
    let close = format!("</{root}>");
    if let Some(pos) = xml_text.rfind(&close) {
        let mut out = String::with_capacity(xml_text.len() + markup.len());
        out.push_str(&xml_text[..pos]);
        out.push_str(markup);
        out.push_str(&xml_text[pos..]);
        return out;
    }
    let root_tag = xml::tags(xml_text, 0).find(|tag| tag.name == root);
    match root_tag {
        Some(tag) if tag.kind == xml::TagKind::Empty => {
            let open = xml_text[tag.range.start..tag.range.end - 2].trim_end();
            format!(
                "{}{open}>{markup}{close}{}",
                &xml_text[..tag.range.start],
                &xml_text[tag.range.end..]
            )
        }
        _ => format!("{xml_text}{markup}"),
    }
}

/// The part name for `target` of a relationship in `source` (a part name;
/// empty for the package root).
pub fn resolve_target(source: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut segments: Vec<&str> = match source.rfind('/') {
        Some(slash) => source[..slash].split('/').collect(),
        None => Vec::new(),
    };
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}

/// The relationships part of part `name` (`word/document.xml` →
/// `word/_rels/document.xml.rels`).
pub fn rels_name(name: &str) -> String {
    match name.rfind('/') {
        Some(slash) => format!("{}/_rels/{}.rels", &name[..slash], &name[slash + 1..]),
        None => format!("_rels/{name}.rels"),
    }
}

/// One relationship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relationship {
    pub id: String,
    pub rel_type: String,
    pub target: String,
    pub external: bool,
}

/// A relationships part: the template's entries plus added ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relationships {
    source: String,
    pub entries: Vec<Relationship>,
    added: usize,
}

impl Relationships {
    pub fn parse(text: &str) -> Self {
        let entries: Vec<_> = xml::tags(text, 0)
            .filter(|tag| tag.name == "Relationship" && tag.opens())
            .filter_map(|tag| {
                Some(Relationship {
                    id: tag.attr("Id")?,
                    rel_type: tag.attr("Type")?,
                    target: tag.attr("Target")?,
                    external: tag
                        .attr("TargetMode")
                        .is_some_and(|m| m.eq_ignore_ascii_case("External")),
                })
            })
            .collect();
        Self {
            source: text.to_string(),
            added: 0,
            entries,
        }
    }

    pub fn target_of(&self, rel_type: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|r| r.rel_type == rel_type)
            .map(|r| r.target.clone())
    }

    /// The id of a relationship to `target`, reusing an equal one: links to
    /// the same URL share one relationship.
    pub fn add(&mut self, rel_type: &str, target: &str, external: bool) -> String {
        if let Some(r) = self
            .entries
            .iter()
            .find(|r| r.rel_type == rel_type && r.target == target && r.external == external)
        {
            return r.id.clone();
        }
        let mut n = self.entries.len() + 1;
        let id = loop {
            let id = format!("rId{n}");
            if !self.entries.iter().any(|r| r.id == id) {
                break id;
            }
            n += 1;
        };
        self.entries.push(Relationship {
            id: id.clone(),
            rel_type: rel_type.to_string(),
            target: target.to_string(),
            external,
        });
        self.added += 1;
        id
    }

    /// The part text: the template's markup with added entries appended.
    pub fn to_xml(&self) -> String {
        const EMPTY: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#;
        let base = if self.source.trim().is_empty() {
            EMPTY
        } else {
            &self.source
        };
        let added: String = self.entries[self.entries.len() - self.added..]
            .iter()
            .map(|r| {
                format!(
                    r#"<Relationship Id="{}" Type="{}" Target="{}"{}/>"#,
                    xml::escape_attr(&r.id),
                    xml::escape_attr(&r.rel_type),
                    xml::escape_attr(&r.target),
                    if r.external {
                        r#" TargetMode="External""#
                    } else {
                        ""
                    }
                )
            })
            .collect();
        if added.is_empty() {
            return base.to_string();
        }
        insert_before_close(base, "Relationships", &added)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> Package {
        let mut p = Package::default();
        p.put(
            CONTENT_TYPES,
            r#"<?xml version="1.0"?><Types xmlns="t"><Default Extension="xml" ContentType="application/xml"/></Types>"#,
        );
        p.put("word/document.xml", "<w:document/>");
        p
    }

    #[test]
    fn round_trips_parts_through_zip() {
        let p = minimal();
        let bytes = p.write();
        assert_eq!(Package::read(&bytes).unwrap(), p);
    }

    #[test]
    fn writes_identical_bytes_and_content_types_first() {
        let p = minimal();
        let bytes = p.write();
        assert_eq!(bytes, p.write());
        // The first local file header names [Content_Types].xml.
        assert_eq!(
            &bytes[30..30 + CONTENT_TYPES.len()],
            CONTENT_TYPES.as_bytes()
        );
    }

    #[test]
    fn rejects_non_packages() {
        assert!(Package::read(b"not a zip").is_err());
        let mut p = Package::default();
        p.put("a.xml", "<a/>");
        assert!(Package::read(&p.write()).unwrap_err().contains("OPC"));
    }

    #[test]
    fn adds_content_types_once() {
        let mut p = minimal();
        p.ensure_default_type("png", "image/png");
        p.ensure_default_type("PNG", "image/png");
        p.ensure_default_type("xml", "other");
        p.set_override_type("word/styles.xml", "a");
        p.set_override_type("word/styles.xml", STYLES_CONTENT_TYPE);
        let types = p.text(CONTENT_TYPES).unwrap();
        assert_eq!(types.matches("Extension=\"png\"").count(), 1);
        assert_eq!(types.matches("Extension=").count(), 2);
        assert_eq!(types.matches("/word/styles.xml").count(), 1);
        assert!(types.contains(STYLES_CONTENT_TYPE));
        assert!(types.ends_with("</Types>"));
    }

    #[test]
    fn resolves_relationship_targets() {
        assert_eq!(resolve_target("", "word/document.xml"), "word/document.xml");
        assert_eq!(
            resolve_target("word/document.xml", "styles.xml"),
            "word/styles.xml"
        );
        assert_eq!(
            resolve_target("word/document.xml", "../customXml/a.xml"),
            "customXml/a.xml"
        );
        assert_eq!(
            resolve_target("word/document.xml", "/word/x.xml"),
            "word/x.xml"
        );
        assert_eq!(
            rels_name("word/document.xml"),
            "word/_rels/document.xml.rels"
        );
    }

    #[test]
    fn adds_and_reuses_relationships() {
        let mut rels = Relationships::parse(
            r#"<Relationships xmlns="r"><Relationship Id="rId2" Type="s" Target="styles.xml"/></Relationships>"#,
        );
        let a = rels.add("h", "https://example.org/?a&b", true);
        assert_eq!(a, "rId3");
        assert_eq!(rels.add("h", "https://example.org/?a&b", true), a);
        assert_eq!(rels.add("s", "styles.xml", false), "rId2");
        let text = rels.to_xml();
        assert!(text.contains(
            r#"<Relationship Id="rId3" Type="h" Target="https://example.org/?a&amp;b" TargetMode="External"/></Relationships>"#
        ));
        assert_eq!(Relationships::parse(&text).entries.len(), 2);
    }

    #[test]
    fn expands_a_self_closing_root() {
        assert_eq!(
            insert_before_close("<w:numbering x=\"1\"/>", "w:numbering", "<a/>"),
            "<w:numbering x=\"1\"><a/></w:numbering>"
        );
    }
}
