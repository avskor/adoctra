//! Ruby's `image_uri` with `data-uri`: below secure mode an image source is
//! embedded as a base64 data URI, read through the host's resource provider
//! (relative to the primary document, Ruby's `base_dir`) at conversion.
use crate::{Attributes, Diagnostic, ResourceProvider, ResourceRequest, Severity, Span};

/// The provider and the primary document's canonical name for image reads.
pub(crate) struct Images<'a> {
    source: Option<(&'a dyn ResourceProvider, String)>,
}

impl<'a> Images<'a> {
    pub fn new(source: Option<(&'a dyn ResourceProvider, String)>) -> Self {
        Self { source }
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
        let path = match dir.filter(|dir| !dir.is_empty() && !target.starts_with('/')) {
            Some(dir) => format!("{}/{target}", dir.trim_end_matches('/')),
            None => target.to_owned(),
        };
        match self.load(&path) {
            Ok((_, bytes)) => format!("data:{mimetype};base64,{}", base64(&bytes)),
            Err(name) => {
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

    fn load(&self, target: &str) -> Result<(Option<String>, Vec<u8>), String> {
        let Some((provider, referrer)) = &self.source else {
            return Err(target.to_owned());
        };
        provider
            .load_binary(ResourceRequest {
                target,
                referrer: Some(referrer),
            })
            .map(|resource| (resource.content_type, resource.bytes))
            .map_err(|error| error.target)
    }
}

fn warn(diagnostics: &mut Vec<Diagnostic>, span: Span, message: String) {
    diagnostics.push(Diagnostic {
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
