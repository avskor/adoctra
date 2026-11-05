//! Ruby's `image_uri` with `data-uri`: below secure mode an image source is
//! embedded as a base64 data URI, read through the host's resource provider
//! (relative to the primary document, Ruby's `base_dir`) at conversion.
use crate::{Attributes, Diagnostic, ResourceProvider, ResourceRequest, Severity, Span};

/// Where images are read: the provider, the primary document's canonical
/// name and, when known, Ruby's base directory (with the safe mode that
/// decides its jail).
pub(crate) struct ImageSource<'a> {
    pub provider: &'a dyn ResourceProvider,
    pub referrer: String,
    /// The referrer's own directory (its `document_path`): requests are
    /// relative to it, whatever base directory Ruby resolves from.
    pub referrer_dir: Option<String>,
    pub base_dir: Option<String>,
    pub safe: crate::SafeMode,
}

/// The image source for reads, if any, and the inline macro processors:
/// the host services the macros stage uses while fields convert.
pub(crate) struct Images<'a> {
    source: Option<ImageSource<'a>>,
    inline_macros: Option<InlineMacros>,
}

/// What an inline macro processor's [`crate::ProcessContext`] sees.
pub(crate) struct InlineMacros {
    pub registry: std::sync::Arc<crate::Registry>,
    pub safe: crate::SafeMode,
    pub base_dir: Option<String>,
    pub output: Option<crate::OutputTarget>,
    /// Ruby's document reader mark once reading is done (diagram
    /// processors log at `cursor_at_mark`).
    pub mark: std::cell::Cell<Option<crate::DiagnosticCursor>>,
}

impl<'a> Images<'a> {
    pub fn new(source: Option<ImageSource<'a>>) -> Self {
        Self {
            source,
            inline_macros: None,
        }
    }

    pub fn with_inline_macros(mut self, inline_macros: Option<InlineMacros>) -> Self {
        self.inline_macros = inline_macros;
        self
    }

    pub fn inline_macros(&self) -> Option<&InlineMacros> {
        self.inline_macros.as_ref()
    }

    /// The path Ruby reads and reports for `target` under `start` (its
    /// `normalize_system_path` with the base directory, warning about jail
    /// recovery like Ruby, without a location) and the same path relative
    /// to the primary document for the provider; without a base directory,
    /// `target` joined to a relative start for both.
    fn system_path(
        &self,
        target: &str,
        start: Option<&str>,
        label: &str,
        span: Span,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> (String, String) {
        let (base_dir, referrer_dir, safe) = match &self.source {
            Some(source) => (
                source.base_dir.as_deref(),
                source.referrer_dir.as_deref(),
                source.safe,
            ),
            None => (None, None, crate::SafeMode::default()),
        };
        let resolved =
            crate::system_path::asset_path(target, start, base_dir, referrer_dir, safe, label);
        for warning in resolved.warnings {
            warn(diagnostics, span, warning);
        }
        (resolved.path, resolved.request)
    }

    /// Ruby's `image_uri target, asset_dir_key` with the document attributes
    /// at conversion: `dir` is the asset directory's value (`imagesdir` or
    /// `iconsdir`). Without `data-uri` (or in secure mode) it is the web path.
    /// Failed reads warn at `span`, like Ruby's log while converting.
    pub fn uri(
        &self,
        target: &str,
        dir: Option<&str>,
        attrs: &Attributes,
        span: Span,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> String {
        if crate::context::SafeMode::of(attrs) >= crate::context::SafeMode::Secure
            || attrs.get("data-uri").is_none()
        {
            return crate::image::web_path(target, dir);
        }
        let uri = if uriish(target) {
            Some(target.replace(' ', "%20"))
        } else {
            dir.filter(|dir| uriish(dir))
                .map(|dir| crate::image::normalize_web_path(target, Some(dir), false))
        };
        match uri {
            Some(uri) if attrs.get("allow-uri-read").is_some() => {
                self.enable_cache(attrs);
                self.embed_uri(&uri, span, diagnostics)
            }
            Some(uri) => uri,
            None => self.embed_file(target, dir, span, diagnostics),
        }
    }

    /// `generate_data_uri`: the type from the extension, the file resolved
    /// under the asset directory; an unreadable file embeds no data.
    fn embed_file(
        &self,
        target: &str,
        dir: Option<&str>,
        span: Span,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> String {
        let mimetype = match extname(target) {
            Some(".svg") => "image/svg+xml".to_owned(),
            Some(ext) => format!("image/{}", &ext[1..]),
            None => "application/octet-stream".to_owned(),
        };
        let (path, request) = self.system_path(target, dir, "image", span, diagnostics);
        match self.load(&request) {
            Ok((_, bytes)) => format!("data:{mimetype};base64,{}", base64(&bytes)),
            Err(name) => {
                let name = if self.resolves() { path } else { name };
                warn(
                    diagnostics,
                    span,
                    format!("image to embed not found or not readable: {name}"),
                );
                format!("data:{mimetype};base64,")
            }
        }
    }

    /// `generate_data_uri_from_uri`: the type from the response; a failed
    /// read keeps the URI.
    fn embed_uri(&self, uri: &str, span: Span, diagnostics: &mut Vec<Diagnostic>) -> String {
        match self.load(uri) {
            Ok((content_type, bytes)) => format!(
                "data:{};base64,{}",
                content_type.unwrap_or_else(|| "application/octet-stream".to_owned()),
                base64(&bytes)
            ),
            Err(_) => {
                warn(
                    diagnostics,
                    span,
                    format!("could not retrieve image data from URI: {uri}"),
                );
                uri.to_owned()
            }
        }
    }

    /// Ruby's SVG branch of `convert_image`/`convert_inline_image`: below
    /// secure mode an SVG target (`.svg` in it, or `format=svg`) with the
    /// `inline` option is read (`read_svg_contents`), with `interactive` it
    /// takes a fallback source; otherwise `None` (an `<img>`).
    #[allow(clippy::too_many_arguments)]
    pub fn svg(
        images: Option<&Self>,
        target: &str,
        named: &std::collections::BTreeMap<String, String>,
        options: &[String],
        attrs: &Attributes,
        span: Span,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<crate::Svg> {
        let svg = named.get("format").is_some_and(|f| f == "svg") || target.contains(".svg");
        if !svg || crate::context::SafeMode::of(attrs) >= crate::context::SafeMode::Secure {
            return None;
        }
        let none = Self::new(None);
        let images = images.unwrap_or(&none);
        let option = |name: &str| options.iter().any(|o| o == name);
        if option("inline") {
            Some(crate::Svg::Inline(images.read_svg(
                target,
                attrs.get("imagesdir"),
                named.get("docfile").map(String::as_str),
                attrs,
                span,
                diagnostics,
            )))
        } else if option("interactive") {
            let fallback = named.get("fallback").map(|fallback| {
                images.uri(fallback, attrs.get("imagesdir"), attrs, span, diagnostics)
            });
            Some(crate::Svg::Interactive { fallback })
        } else {
            None
        }
    }

    /// `read_contents target, start: imagesdir, normalize: true, label:
    /// 'SVG', warn_if_empty: true`: a URI only with `allow-uri-read`, else
    /// the file under `imagesdir`; lines are right-stripped (`prepare_source_string`).
    /// `None` (Ruby's alt fallback) when unreadable or empty. `docfile` is
    /// the node's *own* attribute: Ruby's `read_asset` warns with `attr
    /// 'docfile'` on the image node, which does not fall back to the
    /// document, so a file document still reports `<stdin>`.
    #[allow(clippy::too_many_arguments)]
    fn read_svg(
        &self,
        target: &str,
        dir: Option<&str>,
        docfile: Option<&str>,
        attrs: &Attributes,
        span: Span,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<String> {
        let uri = if uriish(target) {
            Some(target.to_owned())
        } else {
            dir.filter(|dir| uriish(dir))
                .map(|dir| crate::image::normalize_web_path(target, Some(dir), false))
        };
        let (name, text) = match uri {
            Some(uri) if attrs.get("allow-uri-read").is_none() => {
                warn(
                    diagnostics,
                    span,
                    format!("cannot retrieve contents of SVG at URI: {uri} (allow-uri-read attribute not enabled)"),
                );
                return None;
            }
            Some(uri) => {
                self.enable_cache(attrs);
                match self.load_named(&uri) {
                    Ok(loaded) => loaded,
                    Err(_) => {
                        warn(
                            diagnostics,
                            span,
                            format!("could not retrieve contents of SVG at URI: {uri}"),
                        );
                        return None;
                    }
                }
            }
            None => {
                let (path, request) = self.system_path(target, dir, "SVG", span, diagnostics);
                match self.load_named(&request) {
                    // Ruby reports (and reads) the resolved path.
                    Ok((name, text)) if !self.resolves() => (name, text),
                    Ok((_, text)) => (path, text),
                    Err(name) => {
                        let name = if self.resolves() { path } else { name };
                        let docfile = docfile.unwrap_or("<stdin>");
                        warn(
                            diagnostics,
                            span,
                            format!("{docfile}: SVG does not exist or cannot be read: {name}"),
                        );
                        return None;
                    }
                }
            }
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let contents = text
            .split_inclusive('\n')
            .map(|line| line.trim_end_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r', '\0']))
            .collect::<Vec<_>>()
            .join("\n");
        if contents.is_empty() {
            warn(
                diagnostics,
                span,
                format!("contents of SVG is empty: {name}"),
            );
            return None;
        }
        Some(contents)
    }

    /// Whether paths resolve like Ruby's (a base directory is known).
    fn resolves(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.base_dir.is_some())
    }

    fn enable_cache(&self, attrs: &Attributes) {
        if attrs.get("cache-uri").is_some() {
            if let Some(source) = &self.source {
                source.provider.enable_uri_cache();
            }
        }
    }

    fn load_named(&self, target: &str) -> Result<(String, String), String> {
        let Some(ImageSource {
            provider, referrer, ..
        }) = &self.source
        else {
            return Err(target.to_owned());
        };
        provider
            .load_asset_text(ResourceRequest {
                target,
                referrer: Some(referrer.as_str()),
            })
            .map(|resource| (resource.name, resource.text))
            .map_err(|error| error.target)
    }

    fn load(&self, target: &str) -> Result<(Option<String>, Vec<u8>), String> {
        let Some(ImageSource {
            provider, referrer, ..
        }) = &self.source
        else {
            return Err(target.to_owned());
        };
        provider
            .load_binary(ResourceRequest {
                target,
                referrer: Some(referrer.as_str()),
            })
            .map(|resource| (resource.content_type, resource.bytes))
            .map_err(|error| error.target)
    }
}

fn warn(diagnostics: &mut Vec<Diagnostic>, span: Span, message: String) {
    diagnostics.push(Diagnostic {
        omitted_span: None,
        report: crate::DiagnosticReport {
            location: crate::DiagnosticLocation::Unlocated,
            ..Default::default()
        },
        span,
        severity: Severity::Warning,
        message,
    });
}

/// Ruby's `Helpers.uriish?`.
fn uriish(s: &str) -> bool {
    s.contains(':') && crate::image::uri_prefix(s).is_some()
}

/// Ruby's `Helpers.extname path, nil`: from the last dot of the last segment.
fn extname(path: &str) -> Option<&str> {
    let dot = path.rfind('.')?;
    (!path[dot..].contains('/')).then(|| &path[dot..])
}

/// `[data].pack 'm0'` (strict base64).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_strict() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&[0xff, 0xfe, 0x00, 0x01]), "//4AAQ==");
    }

    #[test]
    fn extname_is_rubys() {
        assert_eq!(extname("a/b.svg"), Some(".svg"));
        assert_eq!(extname("a.d/b"), None);
        assert_eq!(extname("b"), None);
    }
}
