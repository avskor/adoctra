//! The `image::target[…]` block macro, as the pinned Ruby parser reads it
//! (`BlockMediaMacroRx`, shared with `video::`/`audio::`, and the `:image`
//! branch of `next_block`) and the HTML
//! converter resolves it (`AbstractBlock#alt`, `AbstractNode#image_uri` with
//! `PathResolver#web_path`). Only the secure-mode behavior applies: there are
//! no data URIs and no inline/interactive SVG.
use crate::substitution_text::Text;
use crate::AttributePart;
use std::ops::Range;

/// The context of a block media macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Media {
    Image,
    Video,
    Audio,
}

impl Media {
    /// The names of the positional attributes (Ruby's `posattrs`).
    pub fn posattrs(self) -> &'static [&'static str] {
        match self {
            Media::Image => &["alt", "width", "height"],
            Media::Video => &["poster", "width", "height"],
            Media::Audio => &[],
        }
    }
}

/// `^(image|video|audio)::(\S|\S.*?\S)\[(.+)?\]$`: the target ends at the
/// first `[` preceded by a non-blank character, and the line ends with `]`.
/// Returns the context, the target and the attribute list ranges within `line`.
pub(crate) fn match_macro(line: &str) -> Option<(Media, Range<usize>, Range<usize>)> {
    let (media, prefix) = [
        (Media::Image, "image::"),
        (Media::Video, "video::"),
        (Media::Audio, "audio::"),
    ]
    .into_iter()
    .find(|(_, prefix)| line.starts_with(prefix))?;
    let rest = &line[prefix.len()..];
    let close = rest.strip_suffix(']')?.len();
    if !rest.chars().next().is_some_and(|c| !is_blank(c)) {
        return None;
    }
    let open = rest[..close]
        .match_indices('[')
        .map(|(at, _)| at)
        .find(|&at| at > 0 && !rest[..at].chars().next_back().is_some_and(is_blank))?;
    let base = prefix.len();
    Some((media, base..base + open, base + open + 1..base + close))
}

/// Ruby's `\s` (and the complement of `\S`).
fn is_blank(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// `Helpers.basename(target, true).tr('_-', ' ')`: the default alt text.
pub(crate) fn default_alt(target: &str) -> String {
    let trimmed = target.trim_end_matches('/');
    let base = if trimmed.is_empty() && !target.is_empty() {
        "/"
    } else {
        trimmed.rsplit('/').next().unwrap_or(trimmed)
    };
    // Helpers.extname looks at the whole path: a dot followed by a slash
    // is no extension.
    let ext = target
        .rfind('.')
        .filter(|&dot| !target[dot..].contains('/'))
        .map_or("", |dot| &target[dot..]);
    let base = match base.strip_suffix(ext) {
        Some(stem) if !ext.is_empty() && !stem.is_empty() => stem,
        _ => base,
    };
    base.replace(['_', '-'], " ")
}

/// `Helpers.extname?`: a dot in the last path segment.
pub(crate) fn has_extension(path: &str) -> bool {
    path.rfind('.')
        .is_some_and(|dot| !path[dot..].contains('/'))
}

/// `AbstractBlock#alt`: special characters are the backend's; a value other
/// than the default alt also gets replacements, whose generated entities stay
/// raw like Ruby's substituted string.
pub(crate) fn alt_parts(alt: &str, default: bool) -> Vec<AttributePart> {
    let parts = vec![AttributePart::Text(alt.to_owned())];
    if default || alt.is_empty() {
        parts
    } else {
        crate::replacements::apply_parts(parts)
    }
}

/// `TrailingDigitsRx`: a `scaledwidth` without units is a percentage.
pub(crate) fn scaled_width(value: &str) -> String {
    if value.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{value}%")
    } else {
        value.to_owned()
    }
}

/// `UriSniffRx` (`\A\p{Alpha}[\p{Alnum}.+-]+:/{0,2}`): the length of a URI
/// scheme prefix, including up to two slashes.
/// Ruby's `UriSniffRx` (`uriish?`): the end of a scheme prefix.
pub(crate) fn uri_prefix(s: &str) -> Option<usize> {
    let mut chars = s.char_indices();
    if !chars.next()?.1.is_alphabetic() {
        return None;
    }
    for (count, (at, c)) in chars.enumerate() {
        if c == ':' && count > 0 {
            let slashes = s[at + 1..]
                .bytes()
                .take(2)
                .take_while(|&b| b == b'/')
                .count();
            return Some(at + 1 + slashes);
        }
        if !(c.is_alphanumeric() || matches!(c, '.' | '+' | '-')) {
            return None;
        }
    }
    None
}

/// `AbstractNode#normalize_web_path` for an image `target` under `imagesdir`:
/// a URI keeps its form with spaces encoded; a relative path is joined to the
/// directory, `.`/`..` segments are resolved and spaces encoded.
pub(crate) fn web_path(target: &str, imagesdir: Option<&str>) -> String {
    normalize_web_path(target, imagesdir, true)
}

/// Ruby's `AbstractNode#normalize_web_path`: with `preserve_uri_target`, a
/// URI target keeps its form (spaces encoded); otherwise the target is always
/// joined to `start` (`PathResolver#web_path`), e.g. a stylesheet name under
/// `stylesdir`.
pub fn normalize_web_path(target: &str, start: Option<&str>, preserve_uri_target: bool) -> String {
    if preserve_uri_target && target.contains(':') && uri_prefix(target).is_some() {
        return target.replace(' ', "%20");
    }
    let imagesdir = start;
    let mut path = target.to_owned();
    let mut prefix = "";
    let joined;
    if let Some(start) = imagesdir.filter(|start| !start.is_empty() && !target.starts_with('/')) {
        let separator = if start.ends_with('/') { "" } else { "/" };
        joined = format!("{start}{separator}{target}");
        match uri_prefix(&joined).filter(|_| joined.contains(':')) {
            Some(len) => {
                prefix = &joined[..len];
                path = joined[len..].to_owned();
            }
            None => path.clone_from(&joined),
        }
    }
    let root = if path.starts_with('/') {
        Some("/")
    } else if path.starts_with("./") {
        Some("./")
    } else {
        None
    };
    let rest = &path[root.map_or(0, str::len)..];
    // Ruby's String#split drops trailing empty segments.
    let mut segments: Vec<&str> = rest.split('/').collect();
    while segments.last() == Some(&"") {
        segments.pop();
    }
    let mut resolved: Vec<&str> = Vec::new();
    for segment in segments.into_iter().filter(|s| *s != ".") {
        if segment != ".." {
            resolved.push(segment);
        } else if resolved.is_empty() {
            if matches!(root, None | Some("./")) {
                resolved.push(segment);
            }
        } else if resolved.last() == Some(&"..") {
            resolved.push(segment);
        } else {
            resolved.pop();
        }
    }
    let path = format!("{}{}", root.unwrap_or(""), resolved.join("/")).replace(' ', "%20");
    format!("{prefix}{path}")
}

/// The target's early a-stage source: the processed text at `start`.
pub(crate) fn target_text(target: &str, start: usize) -> Text {
    let mut source = Text::default();
    source.append_source(
        target,
        true,
        0,
        crate::Span::new(start, start + target.len()),
    );
    source
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ruby_media_macro_lines() {
        let line = "image::a b.png[x]y]";
        let (media, target, list) = match_macro(line).unwrap();
        assert_eq!(media, Media::Image);
        assert_eq!((&line[target], &line[list]), ("a b.png", "x]y"));
        let line = "image::a.png[]";
        let (_, target, list) = match_macro(line).unwrap();
        assert_eq!((&line[target], &line[list]), ("a.png", ""));
        // A blank before the bracket moves the match to a later bracket.
        let line = "image::a [b][c]";
        let (_, target, list) = match_macro(line).unwrap();
        assert_eq!((&line[target], &line[list]), ("a [b]", "c"));
        let line = "video::v.mp4[poster.png]";
        let (media, target, list) = match_macro(line).unwrap();
        assert_eq!(media, Media::Video);
        assert_eq!((&line[target], &line[list]), ("v.mp4", "poster.png"));
        let line = "audio::s.ogg[]";
        let (media, target, _) = match_macro(line).unwrap();
        assert_eq!((media, &line[target]), (Media::Audio, "s.ogg"));
        for line in [
            "image::f.png [x]",
            "image:: f.png[x]",
            "image::[x]",
            "image::f.png[x] ",
            "image:f.png[x]",
            "text image::f.png[]",
            "Video::v.mp4[]",
            "audio:s.ogg[]",
        ] {
            assert_eq!(match_macro(line), None, "{line}");
        }
    }

    #[test]
    fn default_alt_is_the_basename_without_extension() {
        assert_eq!(default_alt("my_cool-pic.png"), "my cool pic");
        assert_eq!(default_alt("dir/{nope}.png"), "{nope}");
        assert_eq!(default_alt("dir.v2/file"), "file");
        assert_eq!(default_alt("dir/sub/"), "sub");
        assert_eq!(default_alt(".png"), ".png");
        assert_eq!(default_alt("https://x.org/a.b.svg"), "a.b");
    }

    #[test]
    fn alt_replacements_keep_ruby_entities_raw() {
        assert_eq!(
            alt_parts("(C) it's", false),
            [
                AttributePart::Raw("&#169;".into()),
                AttributePart::Text(" it".into()),
                AttributePart::Raw("&#8217;".into()),
                AttributePart::Text("s".into()),
            ]
        );
        assert_eq!(
            alt_parts("a_(C)", true),
            [AttributePart::Text("a_(C)".into())]
        );
    }

    #[test]
    fn web_paths_follow_the_path_resolver() {
        assert_eq!(web_path("../a b.png", Some("img/")), "a%20b.png");
        assert_eq!(web_path("a.png", Some("one")), "one/a.png");
        assert_eq!(web_path("/abs/./x/../y.png", Some("img")), "/abs/y.png");
        assert_eq!(
            web_path("https://ex.com/a b.png", Some("img")),
            "https://ex.com/a%20b.png"
        );
        assert_eq!(
            web_path("a.png", Some("https://cdn.org/i/")),
            "https://cdn.org/i/a.png"
        );
        assert_eq!(web_path("../../a.png", Some("./i")), "./../a.png");
        assert_eq!(web_path("/../a.png", None), "/a.png");
        assert_eq!(web_path("a//b/", None), "a//b");
        assert_eq!(web_path("c:x.png", None), "c:x.png");
    }

    #[test]
    fn scaled_width_defaults_to_percent() {
        assert_eq!(scaled_width("50"), "50%");
        assert_eq!(scaled_width("5cm"), "5cm");
    }
}
