//! The template's style registry: one resolver from a style name to the id
//! the template uses, with Word's built-ins added on demand.
//!
//! Styles are looked up by name, then by alias, then by id, ignoring ASCII
//! case. Names are the contract (`Heading 1`, `Code`); ids are whatever the
//! template's Word generated (`1`, `a7` in a Russian UI) and only matter as
//! the last resort.

use std::collections::BTreeSet;

use crate::{fallback, xml};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StyleKind {
    Paragraph,
    Character,
    Table,
    Numbering,
}

impl StyleKind {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "paragraph" => Self::Paragraph,
            "character" => Self::Character,
            "table" => Self::Table,
            "numbering" => Self::Numbering,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Paragraph => "paragraph",
            Self::Character => "character",
            Self::Table => "table",
            Self::Numbering => "numbering",
        }
    }
}

/// One `w:style` of the template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleDef {
    pub id: String,
    pub kind: StyleKind,
    pub name: Option<String>,
    pub aliases: Vec<String>,
    pub based_on: Option<String>,
    /// The numbering the style itself applies (`w:pPr/w:numPr/w:numId`).
    pub num_id: Option<u32>,
}

/// The styles of a template plus the definitions added during conversion.
#[derive(Debug, Clone, Default)]
pub struct Styles {
    defs: Vec<StyleDef>,
    /// Markup of added built-in definitions, appended to `styles.xml`.
    added: String,
    /// Missing styles already reported, so each is reported once.
    reported: BTreeSet<(StyleKind, String)>,
    pub warnings: Vec<String>,
}

impl Styles {
    pub fn parse(text: &str) -> Self {
        let mut defs = Vec::new();
        for tag in xml::tags(text, 0) {
            if tag.name != "w:style" || !tag.opens() {
                continue;
            }
            let (Some(id), Some(kind)) = (
                tag.attr("w:styleId"),
                tag.attr("w:type").as_deref().and_then(StyleKind::parse),
            ) else {
                continue;
            };
            let mut def = StyleDef {
                id,
                kind,
                name: None,
                aliases: Vec::new(),
                based_on: None,
                num_id: None,
            };
            if let Some((_, content)) = xml::element(text, &tag) {
                let mut in_ppr = false;
                for inner in xml::tags(&text[..content.end], content.start) {
                    let val = || inner.attr("w:val");
                    match (inner.name, inner.kind) {
                        ("w:name", _) if inner.opens() => def.name = val(),
                        ("w:aliases", _) if inner.opens() => {
                            def.aliases = val()
                                .unwrap_or_default()
                                .split(',')
                                .map(|a| a.trim().to_string())
                                .filter(|a| !a.is_empty())
                                .collect();
                        }
                        ("w:basedOn", _) if inner.opens() => def.based_on = val(),
                        ("w:pPr", xml::TagKind::Start) => in_ppr = true,
                        ("w:pPr", xml::TagKind::End) => in_ppr = false,
                        ("w:numId", _) if in_ppr && inner.opens() => {
                            def.num_id = val().and_then(|v| v.parse().ok());
                        }
                        _ => {}
                    }
                }
            }
            defs.push(def);
        }
        Self {
            defs,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub fn defs(&self) -> &[StyleDef] {
        &self.defs
    }

    /// The template's style of `kind` named `name` (by name, alias, then id).
    pub fn find(&self, kind: StyleKind, name: &str) -> Option<&StyleDef> {
        let of_kind = || self.defs.iter().filter(move |d| d.kind == kind);
        of_kind()
            .find(|d| {
                d.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .or_else(|| of_kind().find(|d| d.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))))
            .or_else(|| of_kind().find(|d| d.id.eq_ignore_ascii_case(name)))
    }

    /// The numbering style `id` applies itself (a numbered heading).
    pub fn num_id(&self, id: &str) -> Option<u32> {
        let def = self.defs.iter().find(|d| d.id == id)?;
        match def.num_id {
            Some(0) => None,
            Some(n) => Some(n),
            // Inherited from the base style.
            None => {
                let mut base = def.based_on.as_deref();
                for _ in 0..16 {
                    let parent = self.defs.iter().find(|d| Some(d.id.as_str()) == base)?;
                    if let Some(n) = parent.num_id {
                        return (n != 0).then_some(n);
                    }
                    base = parent.based_on.as_deref();
                }
                None
            }
        }
    }

    /// The id of style `name` for the output. A missing Word built-in is
    /// added with Word's defaults; a missing custom style is reported once
    /// and gives `None` (the element falls back to its base formatting).
    pub fn resolve(&mut self, kind: StyleKind, name: &str) -> Option<String> {
        if let Some(def) = self.find(kind, name) {
            return Some(def.id.clone());
        }
        match fallback::builtin(name).filter(|b| b.kind == kind) {
            Some(builtin) => Some(self.add_builtin(builtin)),
            None => {
                if self.reported.insert((kind, name.to_ascii_lowercase())) {
                    self.warnings.push(format!(
                        "docx template is missing {} style: {name}",
                        kind.as_str()
                    ));
                }
                None
            }
        }
    }

    /// Whether the template defines style `name` itself (roles map to
    /// template styles only when they exist; that is not an error).
    #[cfg(test)]
    pub fn has(&self, kind: StyleKind, name: &str) -> bool {
        self.find(kind, name).is_some()
    }

    fn add_builtin(&mut self, builtin: fallback::Builtin) -> String {
        // Bases first: a built-in's base is itself a built-in (`Normal`,
        // `heading 1`), at most two levels deep.
        let based_on = builtin
            .based_on
            .and_then(|base| self.resolve(StyleKind::Paragraph, base));
        let next = builtin
            .next
            .and_then(|next| self.resolve(StyleKind::Paragraph, next));
        let mut id = builtin.id.to_string();
        let mut n = 1;
        while self.defs.iter().any(|d| d.id.eq_ignore_ascii_case(&id)) {
            n += 1;
            id = format!("{}{n}", builtin.id);
        }
        let mut markup = format!(
            r#"<w:style w:type="{}" w:styleId="{}"><w:name w:val="{}"/>"#,
            builtin.kind.as_str(),
            xml::escape_attr(&id),
            builtin.name
        );
        if let Some(base) = &based_on {
            markup.push_str(&format!(
                r#"<w:basedOn w:val="{}"/>"#,
                xml::escape_attr(base)
            ));
        }
        if let Some(next) = &next {
            markup.push_str(&format!(r#"<w:next w:val="{}"/>"#, xml::escape_attr(next)));
        }
        markup.push_str("<w:uiPriority w:val=\"9\"/><w:qFormat/>");
        markup.push_str(&builtin.body);
        markup.push_str("</w:style>");
        self.added.push_str(&markup);
        self.defs.push(StyleDef {
            id: id.clone(),
            kind: builtin.kind,
            name: Some(builtin.name.to_string()),
            aliases: Vec::new(),
            based_on,
            num_id: None,
        });
        id
    }

    /// `styles.xml` with the added definitions.
    pub fn apply(&self, text: &str) -> String {
        if self.added.is_empty() {
            return text.to_string();
        }
        crate::package::insert_before_close(text, "w:styles", &self.added)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUSSIAN_WORD: &str = r#"<w:styles>
<w:style w:type="paragraph" w:default="1" w:styleId="a"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="1"><w:name w:val="heading 1"/><w:basedOn w:val="a"/><w:pPr><w:numPr><w:numId w:val="3"/></w:numPr></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Listing"/><w:aliases w:val="Code, Source"/></w:style>
<w:style w:type="character" w:styleId="Code1"><w:name w:val="Code"/></w:style>
<w:style w:type="table" w:styleId="Heading1"><w:name w:val="Weird table"/></w:style>
</w:styles>"#;

    #[test]
    fn parses_ids_names_aliases_and_numbering() {
        let styles = Styles::parse(RUSSIAN_WORD);
        let h1 = styles.find(StyleKind::Paragraph, "Heading 1").unwrap();
        assert_eq!(h1.id, "1");
        assert_eq!(h1.based_on.as_deref(), Some("a"));
        assert_eq!(h1.num_id, Some(3));
        let code = styles.find(StyleKind::Paragraph, "source").unwrap();
        assert_eq!(code.aliases, ["Code", "Source"]);
    }

    #[test]
    fn resolves_by_name_then_alias_then_id_within_a_kind() {
        let styles = Styles::parse(RUSSIAN_WORD);
        // The paragraph style aliased `Code`, not the character style named so.
        assert_eq!(
            styles.find(StyleKind::Paragraph, "code").unwrap().id,
            "Code"
        );
        assert_eq!(
            styles.find(StyleKind::Character, "Code").unwrap().id,
            "Code1"
        );
        assert_eq!(styles.find(StyleKind::Paragraph, "A").unwrap().id, "a");
        assert!(styles.find(StyleKind::Character, "Listing").is_none());
    }

    #[test]
    fn adds_missing_builtins_with_resolved_bases_and_fresh_ids() {
        let mut styles = Styles::parse(RUSSIAN_WORD);
        assert_eq!(
            styles.resolve(StyleKind::Paragraph, "heading 1").as_deref(),
            Some("1")
        );
        // `TOC Heading` is based on the template's `heading 1` (id `1`).
        assert_eq!(
            styles
                .resolve(StyleKind::Paragraph, "TOC Heading")
                .as_deref(),
            Some("TOCHeading")
        );
        assert_eq!(
            styles.resolve(StyleKind::Paragraph, "Heading 2").as_deref(),
            Some("Heading2")
        );
        assert_eq!(
            styles.resolve(StyleKind::Table, "Weird table").as_deref(),
            Some("Heading1")
        );
        assert_eq!(
            styles.resolve(StyleKind::Paragraph, "heading 2").as_deref(),
            Some("Heading2")
        );
        let out = styles.apply(RUSSIAN_WORD);
        assert!(out.contains(r#"<w:style w:type="paragraph" w:styleId="TOCHeading"><w:name w:val="TOC Heading"/><w:basedOn w:val="1"/><w:next w:val="a"/>"#));
        assert_eq!(out.matches(r#"w:styleId="Heading2""#).count(), 1);
        assert!(out.ends_with("</w:style></w:styles>"));
        assert!(styles.warnings.is_empty());
    }

    #[test]
    fn numbering_is_inherited_from_base_styles() {
        let styles = Styles::parse(
            r#"<w:styles><w:style w:type="paragraph" w:styleId="H1"><w:name w:val="heading 1"/><w:pPr><w:numPr><w:numId w:val="7"/></w:numPr></w:pPr></w:style><w:style w:type="paragraph" w:styleId="A1"><w:name w:val="Appendix"/><w:basedOn w:val="H1"/></w:style><w:style w:type="paragraph" w:styleId="F1"><w:name w:val="Floating"/><w:basedOn w:val="H1"/><w:pPr><w:numPr><w:numId w:val="0"/></w:numPr></w:pPr></w:style></w:styles>"#,
        );
        assert_eq!(styles.num_id("H1"), Some(7));
        assert_eq!(styles.num_id("A1"), Some(7));
        assert_eq!(styles.num_id("F1"), None);
        assert_eq!(styles.num_id("missing"), None);
    }

    #[test]
    fn colliding_builtin_ids_get_a_suffix() {
        let mut styles = Styles::parse(
            r#"<w:styles><w:style w:type="table" w:styleId="Caption"><w:name w:val="x"/></w:style></w:styles>"#,
        );
        assert_eq!(
            styles.resolve(StyleKind::Paragraph, "caption").as_deref(),
            Some("Caption2")
        );
        // `Normal` was added as the base.
        assert!(styles.has(StyleKind::Paragraph, "Normal"));
    }

    #[test]
    fn reports_a_missing_custom_style_once() {
        let mut styles = Styles::parse(RUSSIAN_WORD);
        assert_eq!(styles.resolve(StyleKind::Table, "Admonition Table"), None);
        assert_eq!(styles.resolve(StyleKind::Table, "admonition table"), None);
        assert_eq!(
            styles.warnings,
            ["docx template is missing table style: Admonition Table"]
        );
        assert_eq!(styles.apply(RUSSIAN_WORD), RUSSIAN_WORD);
    }
}
