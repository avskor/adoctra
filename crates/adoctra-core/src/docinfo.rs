//! Ruby's `Document#docinfo`: the shared (`docinfo<-location><suffix>`) and
//! private (`<docname>-docinfo<-location><suffix>`) files a document selects
//! with `docinfo`/`docinfo1`/`docinfo2`, read through the host's provider
//! from `docinfodir` (default: the document's directory), normalized and
//! substituted with `docinfosubs` (default: attributes).
use crate::pass_subs::{Defaults, Stage};
use crate::ruby_strip::RubyStrip;
use crate::{AttributePart, Attributes, ResourceProvider, ResourceRequest, Span};

/// Where the converter places docinfo content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocinfoLocation {
    /// The end of `<head>` (no file qualifier).
    Head,
    /// The start of `<body>` (`-header`).
    Header,
    /// The end of `<body>` (`-footer`).
    Footer,
}

impl DocinfoLocation {
    fn name(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Header => "header",
            Self::Footer => "footer",
        }
    }
}

/// The docinfo content for `location` with the given attributes and output
/// file suffix (`.html`), loaded relative to `referrer` (the primary
/// document's canonical name). Files are joined by line feeds; missing ones
/// are skipped silently, like Ruby's `read_asset`.
pub fn docinfo(
    attributes: &Attributes,
    location: DocinfoLocation,
    suffix: &str,
    provider: &dyn ResourceProvider,
    referrer: Option<&str>,
) -> Vec<AttributePart> {
    let selected: Vec<String> = match attributes.get("docinfo") {
        Some(value) if !value.is_empty() => value
            .split(',')
            .map(|name| name.ruby_strip().to_owned())
            .collect(),
        value => {
            let names: &[&str] = if attributes.get("docinfo2").is_some() {
                &["private", "shared"]
            } else if attributes.get("docinfo1").is_some() {
                &["shared"]
            } else if value.is_some() {
                &["private"]
            } else {
                &[]
            };
            names.iter().map(|name| (*name).to_owned()).collect()
        }
    };
    if selected.is_empty() {
        return Vec::new();
    }
    let has = |kind: &str| {
        selected
            .iter()
            .any(|name| name == kind || *name == format!("{kind}-{}", location.name()))
    };
    let qualifier = match location {
        DocinfoLocation::Head => String::new(),
        _ => format!("-{}", location.name()),
    };
    let file = format!("docinfo{qualifier}{suffix}");
    let path = |name: &str| match attributes.get("docinfodir").filter(|dir| !dir.is_empty()) {
        Some(dir) => format!("{}/{name}", dir.trim_end_matches('/')),
        None => name.to_owned(),
    };
    let mut targets = Vec::new();
    if has("shared") {
        targets.push(path(&file));
    }
    if let Some(docname) = attributes.get("docname").filter(|name| !name.is_empty()) {
        if has("private") {
            targets.push(path(&format!("{docname}-{file}")));
        }
    }
    let stages = match attributes.get("docinfosubs") {
        Some(subs) => crate::pass_subs::resolve_block(
            subs,
            Defaults::None,
            "docinfo",
            Span::new(0, 0),
            &mut Vec::new(),
        ),
        None => vec![Stage::Attributes],
    };
    let mut content = Vec::new();
    for target in targets {
        // Ruby reads docinfo with `read_asset normalize: true`: a UTF-16 BOM
        // decodes the file. Ruby fails the whole conversion on malformed
        // text; adoctra skips such a file like a missing one.
        let Ok(resource) = crate::resource::load_source_text(
            provider,
            ResourceRequest {
                target: &target,
                referrer,
            },
        ) else {
            continue;
        };
        if !content.is_empty() {
            content.push(AttributePart::Raw("\n".to_owned()));
        }
        content.extend(substitute(&normalize(&resource.text), &stages, attributes));
    }
    content
}

/// Ruby's `prepare_source_string(…).join LF`: no BOM, lines split at LF only
/// (`each_line`) and right-stripped with Ruby's `rstrip` (ASCII whitespace and
/// NUL; Unicode spaces such as U+00A0 stay).
fn normalize(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.split_terminator('\n')
        .map(RubyStrip::ruby_rstrip)
        .collect::<Vec<_>>()
        .join("\n")
}

fn substitute(text: &str, stages: &[Stage], attributes: &Attributes) -> Vec<AttributePart> {
    let mut evaluation = crate::substitution_context::Evaluation::read_only(attributes);
    let compat = attributes.get("compat-mode").is_some();
    let (_, mut passes) = crate::inline_pass::Passes::extract("", 0, &mut evaluation, compat);
    let mut parts = vec![AttributePart::Raw(text.to_owned())];
    for &stage in stages {
        parts = crate::pass_subs::apply(
            parts,
            stage,
            &mut evaluation,
            compat,
            Span::new(0, 0),
            0,
            &mut passes,
        );
    }
    parts
}
