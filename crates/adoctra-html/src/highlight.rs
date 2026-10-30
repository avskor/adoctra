//! Ruby's syntax highlighter adapters: the source listing's `<pre>`/`<code>`
//! and the markup they add to the document head and the end of the body.
//! For the client-side ones (`highlight.js`, `prettify`, `html-pipeline`)
//! the browser highlights. rouge highlights while converting (see
//! `rouge.rs`). coderay and pygments behave like Ruby without their
//! libraries (the pinned reference environment): a warning and the
//! adapter's plain `<pre>`.

use adoctra_core::ast::Block;
use adoctra_core::Attributes;

/// highlight.js release Ruby loads (`HIGHLIGHT_JS_VERSION`).
const HIGHLIGHT_JS_VERSION: &str = "9.18.3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Highlighter {
    HighlightJs,
    Prettify,
    HtmlPipeline,
    /// The rouge adapter over adoctra-highlight.
    Rouge,
    /// coderay without its gem: `CodeRay` pre class, no highlighting.
    CodeRay,
    /// pygments without pygments.rb.
    Pygments,
}

impl Highlighter {
    /// The document's highlighter, chosen like Ruby's `save_attributes` from
    /// the header attributes: `source-highlighter` names a registered adapter
    /// for an html basebackend, unless `<name>-unavailable` is set.
    pub(super) fn of(header: &Attributes) -> Option<Self> {
        let name = header.get("source-highlighter")?;
        if header.get("basebackend") != Some("html")
            || header.get(&format!("{name}-unavailable")).is_some()
        {
            return None;
        }
        match name {
            "highlightjs" | "highlight.js" => Some(Self::HighlightJs),
            "prettify" => Some(Self::Prettify),
            "html-pipeline" => Some(Self::HtmlPipeline),
            "rouge" => Some(Self::Rouge),
            "coderay" => Some(Self::CodeRay),
            "pygments" => Some(Self::Pygments),
            _ => None,
        }
    }

    /// The warning Ruby logs once when a source listing asks whether the
    /// adapter highlights (`library_available?`, at block completion).
    pub(super) fn unavailable_warning(self) -> Option<&'static str> {
        match self {
            Self::CodeRay => {
                Some("optional gem 'coderay' is not available. Functionality disabled.")
            }
            Self::Pygments => Some(
                "optional gem 'pygments.rb' is not available (reason: cannot load 'pygments'). \
                 Functionality disabled.",
            ),
            _ => None,
        }
    }

    /// The opening tags of a source listing, like the adapter's `format`.
    /// `id` and `roles` are adoctra's: it has no wrapper `<div>`, so they go
    /// on the `<pre>` as for a plain source listing. Returns the closing tags.
    pub(super) fn open(self, block: &Block, nowrap: &str, out: &mut String) -> &'static str {
        let lang = block
            .attrs
            .named
            .get("language")
            .map(|lang| super::escape(lang));
        let id = super::id_attr(&block.id);
        let base = match self {
            Self::HtmlPipeline => {
                // Its own markup without nowrap; `listing` stands for Ruby's
                // `listingblock` wrapper, as on adoctra's plain listing.
                let class = super::class_attr(&["listing"], &block.roles);
                let lang = lang.map_or_else(String::new, |lang| format!(" lang=\"{lang}\""));
                out.push_str(&format!("<pre{class}{id}{lang}><code>"));
                return "</code></pre>\n";
            }
            Self::HighlightJs => "highlightjs",
            Self::Prettify => "prettyprint",
            Self::Rouge => unreachable!("rouge listings render in rouge.rs"),
            Self::CodeRay => "CodeRay",
            Self::Pygments => "pygments",
        };
        // prettify's transform appends `linenums[:start]` with `linenums`.
        let linenums = match (self, block.attrs.named.get("linenums")) {
            (Self::Prettify, Some(_)) => match block.attrs.named.get("start") {
                Some(start) => format!("linenums:{start}"),
                None => "linenums".to_owned(),
            },
            _ => String::new(),
        };
        let class = super::class_attr(&[base, "highlight", nowrap, &linenums], &block.roles);
        out.push_str(&format!("<pre{class}{id}><code"));
        if self == Self::HighlightJs {
            out.push_str(&format!(
                " class=\"language-{} hljs\"",
                lang.as_deref().unwrap_or("none")
            ));
        }
        if let Some(lang) = lang {
            out.push_str(&format!(" data-lang=\"{lang}\""));
        }
        out.push('>');
        "</code></pre>\n"
    }

    /// The adapter's head docinfo (a theme stylesheet). Like Ruby's
    /// converter it is produced after the content, from the final
    /// attributes, with the CDN scheme of the header.
    pub(super) fn head(self, attrs: &Attributes, header: &Attributes, out: &mut String) {
        let href = match self {
            Self::HighlightJs => format!(
                "{}/styles/{}.min.css",
                highlightjs_dir(attrs, header),
                attrs.get("highlightjs-theme").unwrap_or("github")
            ),
            Self::Prettify => {
                let theme = attrs.get("prettify-theme").unwrap_or("prettify");
                if theme.starts_with("http://") || theme.starts_with("https://") {
                    theme.to_owned()
                } else {
                    format!("{}/{theme}.min.css", prettify_dir(attrs, header))
                }
            }
            Self::HtmlPipeline | Self::Rouge | Self::CodeRay | Self::Pygments => return,
        };
        out.push_str(&format!(
            "<link rel=\"stylesheet\" href=\"{}\">\n",
            super::attr_value(&href)
        ));
    }

    /// The adapter's footer docinfo (the scripts), at the end of the body
    /// before the MathJax setup.
    pub(super) fn footer(self, attrs: &Attributes, header: &Attributes, out: &mut String) {
        match self {
            Self::HighlightJs => {
                let dir = super::attr_value(&highlightjs_dir(attrs, header));
                out.push_str(&format!(
                    "<script src=\"{dir}/highlight.min.js\"></script>\n"
                ));
                if let Some(languages) = attrs.get("highlightjs-languages") {
                    // Ruby's `split ','` drops trailing empty fields.
                    let mut languages: Vec<&str> = languages.split(',').collect();
                    while languages.last() == Some(&"") {
                        languages.pop();
                    }
                    for lang in languages {
                        out.push_str(&format!(
                            "<script src=\"{dir}/languages/{}.min.js\"></script>\n",
                            super::attr_value(lang.trim_start())
                        ));
                    }
                }
                out.push_str(include_str!("../assets/highlightjs-init.js.html"));
            }
            Self::Prettify => out.push_str(&format!(
                "<script src=\"{}/run_prettify.min.js\"></script>\n",
                super::attr_value(&prettify_dir(attrs, header))
            )),
            Self::HtmlPipeline | Self::Rouge | Self::CodeRay | Self::Pygments => {}
        }
    }
}

/// Ruby's `cdn_base_url` from the header `asset-uri-scheme`.
fn cdn_base_url(header: &Attributes) -> String {
    let scheme = header.get("asset-uri-scheme").unwrap_or("https");
    let colon = if scheme.is_empty() { "" } else { ":" };
    format!("{scheme}{colon}//cdnjs.cloudflare.com/ajax/libs")
}

fn highlightjs_dir(attrs: &Attributes, header: &Attributes) -> String {
    match attrs.get("highlightjsdir") {
        Some(dir) => dir.to_owned(),
        None => format!(
            "{}/highlight.js/{HIGHLIGHT_JS_VERSION}",
            cdn_base_url(header)
        ),
    }
}

fn prettify_dir(attrs: &Attributes, header: &Attributes) -> String {
    match attrs.get("prettifydir") {
        Some(dir) => dir.to_owned(),
        None => format!("{}/prettify/r298", cdn_base_url(header)),
    }
}
