//! adoctra's built-in templates, assembled from the text sources under
//! `templates/` (no binary `.docx` is kept in the repository).

use crate::package::{Package, Relationships};

const REL_HEADER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const HEADER_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml";

/// A built-in template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// `doctype=book`: a cover section, chapters and parts start a page.
    Book,
    /// Every other doctype: the header flows into the body.
    Article,
}

impl Builtin {
    /// The template for a document's `doctype`.
    pub fn for_doctype(doctype: Option<&str>) -> Self {
        match doctype {
            Some("book") => Self::Book,
            _ => Self::Article,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Book => "book",
            Self::Article => "article",
        }
    }

    /// The template as a `.docx` file, to start a custom template from.
    pub fn to_docx(self) -> Vec<u8> {
        self.package().write()
    }

    /// The template's package.
    pub(crate) fn package(self) -> Package {
        let (document, page_break) = match self {
            Self::Book => (
                include_str!("../templates/book/document.xml"),
                "<w:pageBreakBefore/>",
            ),
            Self::Article => (include_str!("../templates/article/document.xml"), ""),
        };
        let styles = strip_leading_comment(include_str!("../templates/common/styles.xml"))
            .replace("<!--@book:pageBreakBefore-->", page_break);
        let mut package = Package::default();
        for (name, text) in [
            (
                "[Content_Types].xml",
                include_str!("../templates/common/content-types.xml"),
            ),
            ("_rels/.rels", include_str!("../templates/common/rels.xml")),
            (
                "word/_rels/document.xml.rels",
                include_str!("../templates/common/document.xml.rels"),
            ),
            ("word/document.xml", document),
            (
                "word/numbering.xml",
                include_str!("../templates/common/numbering.xml"),
            ),
            (
                "word/settings.xml",
                include_str!("../templates/common/settings.xml"),
            ),
            (
                "word/fontTable.xml",
                include_str!("../templates/common/fontTable.xml"),
            ),
            (
                "word/footer1.xml",
                include_str!("../templates/common/footer1.xml"),
            ),
        ] {
            package.put(name, text);
        }
        package.put("word/styles.xml", styles);
        if self == Self::Book {
            // The body's running header (title and chapter) and the empty
            // first-page header of each part and chapter; `document.xml`
            // refers to them as rId6 and rId7.
            let mut rels = Relationships::parse(
                &package
                    .text("word/_rels/document.xml.rels")
                    .unwrap_or_default(),
            );
            for (part, text) in [
                ("header1.xml", include_str!("../templates/book/header1.xml")),
                ("header2.xml", include_str!("../templates/book/header2.xml")),
            ] {
                rels.add(REL_HEADER, part, false);
                package.put(&format!("word/{part}"), text);
                package.set_override_type(&format!("word/{part}"), HEADER_CONTENT_TYPE);
            }
            package.put("word/_rels/document.xml.rels", rels.to_xml());
        }
        package
    }
}

/// The source's explanatory comment is for maintainers, not the output.
fn strip_leading_comment(xml: &str) -> String {
    match (xml.find("<!--"), xml.find("-->")) {
        (Some(start), Some(end)) if start < end => {
            let after = xml[end + 3..].trim_start_matches(['\r', '\n']);
            format!("{}{after}", &xml[..start])
        }
        _ => xml.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::styles::StyleKind;
    use crate::template::Template;
    use crate::xml;

    #[test]
    fn picks_the_template_by_doctype() {
        assert_eq!(Builtin::for_doctype(Some("book")), Builtin::Book);
        assert_eq!(Builtin::for_doctype(Some("article")), Builtin::Article);
        assert_eq!(Builtin::for_doctype(Some("manpage")), Builtin::Article);
        assert_eq!(Builtin::for_doctype(None), Builtin::Article);
    }

    #[test]
    fn every_part_is_well_formed_xml() {
        for builtin in [Builtin::Book, Builtin::Article] {
            let package = builtin.package();
            for name in package.names() {
                let text = package.text(name).unwrap();
                let root = xml::tags(&text, 0).next().unwrap();
                assert!(
                    xml::element(&text, &root)
                        .is_some_and(|(r, _)| text[r.end..].trim().is_empty()),
                    "{} {name}",
                    builtin.name()
                );
                assert!(!text.contains("@book:"), "{name}");
            }
        }
    }

    #[test]
    fn the_book_body_has_running_and_first_page_headers() {
        let package = Builtin::Book.package();
        let rels = Relationships::parse(&package.text("word/_rels/document.xml.rels").unwrap());
        let target = |id: &str| {
            rels.entries
                .iter()
                .find(|r| r.id == id)
                .map(|r| (r.rel_type.as_str(), r.target.as_str()))
        };
        assert_eq!(target("rId6"), Some((REL_HEADER, "header1.xml")));
        assert_eq!(target("rId7"), Some((REL_HEADER, "header2.xml")));
        let header = package.text("word/header1.xml").unwrap();
        assert!(header.contains("adoctra:title") && header.contains("adoctra:chapter"));
        assert!(header.contains(r#"STYLEREF "Chapter Title""#));
        let template = Template::from_package(package).unwrap();
        let body = template.body_sect_pr().unwrap();
        assert!(body.contains("<w:titlePg/>") && body.contains(r#"r:id="rId7""#));
        // The article has no header.
        assert!(Builtin::Article
            .package()
            .text("word/header1.xml")
            .is_none());
    }

    #[test]
    fn book_chapters_and_parts_start_a_page() {
        let styles = |b: Builtin| Builtin::package(b).text("word/styles.xml").unwrap();
        let book = styles(Builtin::Book);
        let article = styles(Builtin::Article);
        assert_eq!(book.matches("<w:pageBreakBefore/>").count(), 2);
        assert!(!article.contains("<w:pageBreakBefore/>"));
        assert!(!book.contains("Styles of adoctra"));
    }

    /// The contract styles every built-in template defines itself.
    #[test]
    fn defines_the_contract_styles() {
        for builtin in [Builtin::Book, Builtin::Article] {
            let template = Template::from_package(builtin.package()).unwrap();
            for name in crate::contract::PARAGRAPH {
                assert!(template.styles.has(StyleKind::Paragraph, name), "{name}");
            }
            for name in crate::contract::CHARACTER {
                assert!(template.styles.has(StyleKind::Character, name), "{name}");
            }
            for name in crate::contract::TABLE {
                assert!(template.styles.has(StyleKind::Table, name), "{name}");
            }
            assert!(template
                .styles
                .has(StyleKind::Character, crate::contract::CHAPTER_TITLE));
            assert_eq!(
                template.markers(),
                [
                    "adoctra:title",
                    "adoctra:subtitle",
                    "adoctra:author",
                    "adoctra:revision",
                    "adoctra:body"
                ]
            );
            // Every style a style refers to exists.
            let ids: Vec<_> = template
                .styles
                .defs()
                .iter()
                .map(|d| d.id.as_str())
                .collect();
            for def in template.styles.defs() {
                if let Some(base) = &def.based_on {
                    assert!(ids.contains(&base.as_str()), "{} based on {base}", def.id);
                }
            }
        }
    }
}
