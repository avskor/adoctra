//! Ruby's derived backend and doctype attributes (`Document#
//! update_backend_attributes` / `update_doctype_attributes`): `backend-html5`,
//! `basebackend-html`, `doctype-article`, their combinations, `filetype`,
//! `htmlsyntax`, `outfilesuffix` and `pagewidth`.
//!
//! Like Ruby, the current backend and doctype are tracked separately from the
//! attributes (`@backend`/`@doctype`): unsetting `doctype` in the header
//! removes the attribute only, and later updates still clean up the derived
//! attributes of the remembered value.

use crate::Attributes;

/// Ruby's `DEFAULT_BACKEND`.
pub(crate) const DEFAULT_BACKEND: &str = "html5";
/// Ruby's `DEFAULT_DOCTYPE`.
pub(crate) const DEFAULT_DOCTYPE: &str = "article";

/// The traits of a backend's converter (`init_backend_traits`).
struct Traits {
    basebackend: String,
    filetype: String,
    htmlsyntax: Option<&'static str>,
    outfilesuffix: String,
}

/// The built-in converters' traits; any other backend gets Ruby's
/// `Converter.derive_backend_traits` (Ruby itself aborts without a converter
/// for it, while adoctra keeps parsing).
fn traits(backend: &str, htmlsyntax: Option<&str>) -> Traits {
    let known = |basebackend: &str, filetype: &str, suffix: &str| Traits {
        basebackend: basebackend.to_owned(),
        filetype: filetype.to_owned(),
        htmlsyntax: None,
        outfilesuffix: suffix.to_owned(),
    };
    match backend {
        "html5" => Traits {
            htmlsyntax: Some(if htmlsyntax == Some("xml") {
                "xml"
            } else {
                "html"
            }),
            ..known("html", "html", ".html")
        },
        "docbook5" => known("docbook", "xml", ".xml"),
        "manpage" => known("manpage", "man", ".man"),
        _ => {
            let basebackend = backend.trim_end_matches(|c: char| c.is_ascii_digit());
            let suffix = match basebackend {
                "html" => Some(".html"),
                "docbook" => Some(".xml"),
                "pdf" => Some(".pdf"),
                "epub" => Some(".epub"),
                "manpage" => Some(".man"),
                "asciidoc" => Some(".adoc"),
                _ => None,
            };
            let outfilesuffix = suffix.map_or_else(|| format!(".{basebackend}"), str::to_owned);
            Traits {
                filetype: outfilesuffix[1..].to_owned(),
                htmlsyntax: None,
                basebackend: basebackend.to_owned(),
                outfilesuffix,
            }
        }
    }
}

/// Ruby's `@backend` and `@doctype` with the derived attributes they own.
#[derive(Debug, Clone, Default)]
pub(crate) struct Profile {
    pub backend: Option<String>,
    pub doctype: Option<String>,
}

impl Profile {
    /// Ruby's `update_backend_attributes`. `init` keeps an existing
    /// `outfilesuffix` (`||=`), otherwise it is replaced unless locked.
    pub fn update_backend(
        &mut self,
        attrs: &mut Attributes,
        new_backend: &str,
        init: bool,
        outfilesuffix_locked: bool,
    ) {
        if !init && self.backend.as_deref() == Some(new_backend) {
            return;
        }
        let current_basebackend = attrs.get("basebackend").map(str::to_owned);
        let (actual, mut backend) = match new_backend.split_once(':') {
            Some((actual, delegate)) => (Some(actual), delegate),
            None => (None, new_backend),
        };
        if let Some(rest) = backend.strip_prefix('x').filter(|b| b.starts_with("html")) {
            attrs.set("htmlsyntax", "xml");
            backend = rest;
        } else if backend.starts_with("html") && attrs.get("htmlsyntax").is_none() {
            attrs.set("htmlsyntax", "html");
        }
        let backend = match backend {
            "html" => "html5",
            "docbook" => "docbook5",
            other => other,
        };
        let backend = actual.unwrap_or(backend).to_owned();
        if let Some(doctype) = &self.doctype {
            if let Some(current) = &self.backend {
                attrs.remove(&format!("backend-{current}"));
                attrs.remove(&format!("backend-{current}-doctype-{doctype}"));
            }
            attrs.set(format!("backend-{backend}-doctype-{doctype}"), "");
            attrs.set(format!("doctype-{doctype}"), "");
        } else if let Some(current) = &self.backend {
            attrs.remove(&format!("backend-{current}"));
        }
        attrs.set(format!("backend-{backend}"), "");
        attrs.set("backend", backend.clone());
        let traits = traits(&backend, attrs.get("htmlsyntax"));
        self.backend = Some(backend);
        if let Some(syntax) = traits.htmlsyntax {
            attrs.set("htmlsyntax", syntax);
        }
        if if init {
            attrs.get("outfilesuffix").is_none()
        } else {
            !outfilesuffix_locked
        } {
            attrs.set("outfilesuffix", traits.outfilesuffix);
        }
        if let Some(current) = attrs.get("filetype").map(str::to_owned) {
            attrs.remove(&format!("filetype-{current}"));
        }
        attrs.set(format!("filetype-{}", traits.filetype), "");
        attrs.set("filetype", traits.filetype);
        // Ruby's DEFAULT_PAGE_WIDTHS.
        if traits.basebackend == "docbook" {
            attrs.set("pagewidth", "425");
        } else {
            attrs.remove("pagewidth");
        }
        let basebackend = traits.basebackend;
        if current_basebackend.as_deref() != Some(basebackend.as_str()) {
            if let Some(doctype) = &self.doctype {
                if let Some(current) = &current_basebackend {
                    attrs.remove(&format!("basebackend-{current}"));
                    attrs.remove(&format!("basebackend-{current}-doctype-{doctype}"));
                }
                attrs.set(format!("basebackend-{basebackend}-doctype-{doctype}"), "");
            } else if let Some(current) = &current_basebackend {
                attrs.remove(&format!("basebackend-{current}"));
            }
            attrs.set(format!("basebackend-{basebackend}"), "");
            attrs.set("basebackend", basebackend);
        }
    }

    /// Ruby's `update_doctype_attributes`.
    pub fn update_doctype(&mut self, attrs: &mut Attributes, new_doctype: &str) {
        if self.doctype.as_deref() == Some(new_doctype) {
            return;
        }
        let basebackend = attrs.get("basebackend").map(str::to_owned);
        if let Some(current) = &self.doctype {
            attrs.remove(&format!("doctype-{current}"));
            if let Some(backend) = &self.backend {
                attrs.remove(&format!("backend-{backend}-doctype-{current}"));
            }
            if let Some(base) = &basebackend {
                attrs.remove(&format!("basebackend-{base}-doctype-{current}"));
            }
        }
        if let Some(backend) = &self.backend {
            attrs.set(format!("backend-{backend}-doctype-{new_doctype}"), "");
        }
        if let Some(base) = &basebackend {
            attrs.set(format!("basebackend-{base}-doctype-{new_doctype}"), "");
        }
        attrs.set(format!("doctype-{new_doctype}"), "");
        attrs.set("doctype", new_doctype);
        self.doctype = Some(new_doctype.to_owned());
    }

    /// The root document's initialization after the host attributes: the
    /// default doctype (`manpage` for the manpage backend, locking it) and
    /// the initial backend. Returns whether `doctype` must be locked.
    pub fn initialize(&mut self, attrs: &mut Attributes, outfilesuffix_locked: bool) -> bool {
        let backend = attrs.get("backend").unwrap_or(DEFAULT_BACKEND).to_owned();
        let lock = backend == "manpage";
        if lock {
            attrs.set("doctype", "manpage");
        } else if attrs.get("doctype").is_none() {
            attrs.set("doctype", DEFAULT_DOCTYPE);
        }
        self.doctype = attrs.get("doctype").map(str::to_owned);
        self.update_backend(attrs, &backend, true, outfilesuffix_locked);
        lock
    }

    /// A nested document (Ruby's `parent_doc` branch): the backend stays, the
    /// parent's doctype attribute is reset to the default.
    pub fn nested(attrs: &mut Attributes) -> Self {
        let mut profile = Self {
            backend: attrs.get("backend").map(str::to_owned),
            doctype: attrs.get("doctype").map(str::to_owned),
        };
        profile.update_doctype(attrs, DEFAULT_DOCTYPE);
        profile
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(attrs: &Attributes) -> Vec<String> {
        attrs.iter().map(|(k, v)| format!("{k}={v}")).collect()
    }

    #[test]
    fn initial_html5_article() {
        let mut attrs = Attributes::new();
        let mut profile = Profile::default();
        assert!(!profile.initialize(&mut attrs, false));
        assert_eq!(
            keys(&attrs),
            [
                "backend=html5",
                "backend-html5=",
                "backend-html5-doctype-article=",
                "basebackend=html",
                "basebackend-html=",
                "basebackend-html-doctype-article=",
                "doctype=article",
                "doctype-article=",
                "filetype=html",
                "filetype-html=",
                "htmlsyntax=html",
                "outfilesuffix=.html",
            ]
        );
    }

    #[test]
    fn manpage_backend_forces_doctype() {
        let mut attrs = Attributes::new();
        attrs.set("backend", "manpage");
        attrs.set("doctype", "book");
        let mut profile = Profile::default();
        assert!(profile.initialize(&mut attrs, false));
        assert_eq!(attrs.get("doctype"), Some("manpage"));
        assert_eq!(attrs.get("outfilesuffix"), Some(".man"));
        assert_eq!(attrs.get("basebackend-manpage-doctype-manpage"), Some(""));
        assert_eq!(attrs.get("htmlsyntax"), None);
    }

    #[test]
    fn switching_backend_and_doctype_cleans_up() {
        let mut attrs = Attributes::new();
        let mut profile = Profile::default();
        profile.initialize(&mut attrs, false);
        profile.update_doctype(&mut attrs, "book");
        profile.update_backend(&mut attrs, "docbook", false, false);
        assert_eq!(attrs.get("backend"), Some("docbook5"));
        assert_eq!(attrs.get("backend-html5"), None);
        assert_eq!(attrs.get("backend-html5-doctype-book"), None);
        assert_eq!(attrs.get("basebackend-html"), None);
        assert_eq!(attrs.get("basebackend-html-doctype-book"), None);
        assert_eq!(attrs.get("doctype-article"), None);
        assert_eq!(attrs.get("basebackend-docbook-doctype-book"), Some(""));
        assert_eq!(attrs.get("filetype"), Some("xml"));
        assert_eq!(attrs.get("filetype-html"), None);
        assert_eq!(attrs.get("outfilesuffix"), Some(".xml"));
        assert_eq!(attrs.get("pagewidth"), Some("425"));
        // Ruby leaves htmlsyntax behind.
        assert_eq!(attrs.get("htmlsyntax"), Some("html"));
    }

    #[test]
    fn xhtml_and_derived_backends() {
        let mut attrs = Attributes::new();
        let mut profile = Profile::default();
        profile.initialize(&mut attrs, false);
        profile.update_backend(&mut attrs, "xhtml", false, true);
        assert_eq!(attrs.get("backend"), Some("html5"));
        assert_eq!(attrs.get("htmlsyntax"), Some("xml"));
        assert_eq!(attrs.get("outfilesuffix"), Some(".html"));
        profile.update_backend(&mut attrs, "pdf2", false, false);
        assert_eq!(attrs.get("basebackend"), Some("pdf"));
        assert_eq!(attrs.get("filetype-pdf"), Some(""));
        assert_eq!(attrs.get("outfilesuffix"), Some(".pdf"));
        assert_eq!(attrs.get("backend-html5"), None);
        assert_eq!(attrs.get("backend-pdf2"), Some(""));
    }

    #[test]
    fn nested_resets_doctype() {
        let mut attrs = Attributes::new();
        let mut profile = Profile::default();
        profile.initialize(&mut attrs, false);
        profile.update_doctype(&mut attrs, "book");
        let nested = Profile::nested(&mut attrs);
        assert_eq!(nested.doctype.as_deref(), Some("article"));
        assert_eq!(attrs.get("doctype-book"), None);
        assert_eq!(attrs.get("doctype-article"), Some(""));
        assert_eq!(attrs.get("backend-html5-doctype-article"), Some(""));
    }

    fn paragraphs(blocks: &[crate::Block], out: &mut Vec<String>) {
        for block in blocks {
            match &block.kind {
                crate::BlockKind::Paragraph { inlines } => {
                    out.push(crate::resolve::inline_text(inlines))
                }
                crate::BlockKind::Section { blocks, .. }
                | crate::BlockKind::Preamble { blocks } => paragraphs(blocks, out),
                crate::BlockKind::Table(table) => {
                    for cell in table.body.iter().flatten() {
                        if let crate::ast::CellContent::Blocks { blocks, .. } = &cell.content {
                            paragraphs(blocks, out);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn texts(source: &str) -> Vec<String> {
        let mut out = Vec::new();
        paragraphs(&crate::parse(source).document.blocks, &mut out);
        out
    }

    #[test]
    fn parse_exposes_the_initial_profile() {
        let result = crate::parse("{backend} {doctype} {filetype} {last-update-label}\n");
        let attrs = &result.document.attributes;
        assert_eq!(attrs.get("basebackend-html-doctype-article"), Some(""));
        assert_eq!(attrs.get("attribute-missing"), Some("skip"));
        assert_eq!(attrs.get("attribute-undefined"), Some("drop-line"));
        assert_eq!(attrs.get("version-label"), Some("Version"));
        assert_eq!(attrs.get("untitled-label"), Some("Untitled"));
        assert_eq!(
            texts("{backend} {doctype} {filetype} {last-update-label}\n"),
            ["html5 article html Last updated"]
        );
    }

    #[test]
    fn header_doctype_updates_derived_attributes_body_does_not() {
        let result = crate::parse("= T\n:doctype: book\n\n:doctype: article\n\n{doctype}\n");
        let header = &result.document.attributes;
        assert_eq!(header.get("doctype-book"), Some(""));
        assert_eq!(header.get("backend-html5-doctype-book"), Some(""));
        assert_eq!(header.get("doctype-article"), None);
        let last = &result.document.final_attributes;
        assert_eq!(last.get("doctype"), Some("article"));
        assert_eq!(last.get("doctype-book"), Some(""));
        assert_eq!(last.get("doctype-article"), None);
    }

    #[test]
    fn unset_doctype_keeps_the_book_behaviour() {
        let result = crate::parse("= T\n:doctype: book\n:doctype!:\n\n= Part\n\n== Chapter\n");
        let attrs = &result.document.attributes;
        assert_eq!(attrs.get("doctype"), None);
        assert_eq!(attrs.get("doctype-book"), Some(""));
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("level 0")),
            "{:?}",
            result.diagnostics
        );
        // A later assignment cleans up after the remembered doctype.
        let result = crate::parse("= T\n:doctype: book\n:doctype!:\n:doctype: article\n");
        assert_eq!(result.document.attributes.get("doctype-book"), None);
    }

    #[test]
    fn header_backend_switches_traits() {
        let result = crate::parse(":backend: docbook\n\ntext\n");
        let attrs = &result.document.attributes;
        assert_eq!(attrs.get("backend"), Some("docbook5"));
        assert_eq!(attrs.get("backend-docbook5-doctype-article"), Some(""));
        assert_eq!(attrs.get("basebackend-docbook"), Some(""));
        assert_eq!(attrs.get("backend-html5"), None);
        assert_eq!(attrs.get("outfilesuffix"), Some(".xml"));
        assert_eq!(attrs.get("pagewidth"), Some("425"));
        // The same backend again is a no-op unless htmlsyntax changed.
        let result = crate::parse(":outfilesuffix!:\n:backend: html5\n");
        assert_eq!(result.document.attributes.get("outfilesuffix"), None);
        let result = crate::parse(":outfilesuffix!:\n:htmlsyntax: xml\n:backend: html5\n");
        let attrs = &result.document.attributes;
        assert_eq!(attrs.get("outfilesuffix"), Some(".html"));
        assert_eq!(attrs.get("htmlsyntax"), Some("xml"));
    }

    #[test]
    fn host_overrides_lock_backend_suffix_and_manpage_doctype() {
        let mut options = crate::ParseOptions::default();
        options.attributes.set("backend", "manpage");
        options.attributes.set("outfilesuffix", ".8");
        let source = "= prog(1)\n:doctype: book\n:backend: html5\n:outfilesuffix: .x\n\n== NAME\n\nprog - p\n";
        let result = crate::parse_with_options(source, &options);
        let attrs = &result.document.attributes;
        assert_eq!(attrs.get("doctype"), Some("manpage"));
        assert_eq!(attrs.get("backend"), Some("manpage"));
        assert_eq!(attrs.get("outfilesuffix"), Some(".8"));
        assert_eq!(attrs.get("filetype-man"), Some(""));
        assert!(result.document.manpage.is_some());
    }

    #[test]
    fn nested_documents_reset_the_doctype() {
        let source = "= T\n:doctype: book\n\n[cols=1a]\n|===\n\
                      |{doctype} {doctype-book} [{doctype-article}]\n\n\
                      :doctype-book: x\n\n{doctype-book}\n|===\n\n{doctype} [{doctype-book}]\n";
        assert_eq!(
            texts(source),
            ["article {doctype-book} []", "{doctype-book}", "book []"]
        );
    }
}
