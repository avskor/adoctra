//! The package's core properties (`docProps/core.xml`): title, authors,
//! subject, keywords and dates, from the document header. Word shows them in
//! File › Info, and document-property content controls (in a header, on a
//! cover) display them.

use crate::xml::escape_text;

/// The properties of one document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreProperties {
    pub title: Option<String>,
    pub creator: Option<String>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub keywords: Option<String>,
    /// W3CDTF date-time.
    pub date: Option<String>,
}

impl CoreProperties {
    pub fn to_xml(&self) -> String {
        let mut out = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">"#,
        );
        for (tag, value) in [
            ("dc:title", &self.title),
            ("dc:subject", &self.subject),
            ("dc:creator", &self.creator),
            ("cp:keywords", &self.keywords),
            ("dc:description", &self.description),
            ("cp:lastModifiedBy", &self.creator),
        ] {
            if let Some(value) = value {
                out.push_str(&format!("<{tag}>{}</{tag}>", escape_text(value)));
            }
        }
        if let Some(date) = &self.date {
            for tag in ["dcterms:created", "dcterms:modified"] {
                out.push_str(&format!(
                    r#"<{tag} xsi:type="dcterms:W3CDTF">{}</{tag}>"#,
                    escape_text(date)
                ));
            }
        }
        out.push_str("</cp:coreProperties>");
        out
    }
}

/// A W3CDTF date-time from Ruby's `docdatetime` (`2026-11-04 10:20:30 +0300`)
/// or `docdate` (`2026-11-04`) value.
pub fn w3cdtf(value: &str) -> Option<String> {
    let mut fields = value.split_whitespace();
    let date = fields.next()?;
    let valid_date = date.len() == 10
        && date.char_indices().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !valid_date {
        return None;
    }
    let Some(time) = fields.next() else {
        return Some(format!("{date}T00:00:00Z"));
    };
    let valid_time = time.len() == 8
        && time.char_indices().all(|(i, c)| {
            if i == 2 || i == 5 {
                c == ':'
            } else {
                c.is_ascii_digit()
            }
        });
    if !valid_time {
        return None;
    }
    let zone = match fields.next() {
        None | Some("UTC" | "Z") => "Z".to_string(),
        Some(z) if z.len() == 5 && (z.starts_with('+') || z.starts_with('-')) => {
            format!("{}:{}", &z[..3], &z[3..])
        }
        Some(_) => return None,
    };
    Some(format!("{date}T{time}{zone}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_ruby_dates() {
        assert_eq!(
            w3cdtf("2026-11-04 10:20:30 +0300").as_deref(),
            Some("2026-11-04T10:20:30+03:00")
        );
        assert_eq!(
            w3cdtf("2026-11-04 10:20:30 UTC").as_deref(),
            Some("2026-11-04T10:20:30Z")
        );
        assert_eq!(
            w3cdtf("2026-11-04").as_deref(),
            Some("2026-11-04T00:00:00Z")
        );
        assert_eq!(w3cdtf("November 4"), None);
        assert_eq!(w3cdtf("2026-11-04 10:20"), None);
    }

    #[test]
    fn writes_only_known_properties() {
        let props = CoreProperties {
            title: Some("A & B".into()),
            creator: Some("Ann".into()),
            date: Some("2026-11-04T00:00:00Z".into()),
            ..Default::default()
        };
        let xml = props.to_xml();
        assert!(xml.contains("<dc:title>A &amp; B</dc:title>"));
        assert!(
            xml.contains("<dc:creator>Ann</dc:creator><cp:lastModifiedBy>Ann</cp:lastModifiedBy>")
        );
        assert!(xml.contains(
            r#"<dcterms:modified xsi:type="dcterms:W3CDTF">2026-11-04T00:00:00Z</dcterms:modified>"#
        ));
        assert!(!xml.contains("dc:subject"));
        let empty = CoreProperties::default().to_xml();
        assert!(
            empty.ends_with("\"></cp:coreProperties>") || empty.ends_with("></cp:coreProperties>")
        );
    }
}
