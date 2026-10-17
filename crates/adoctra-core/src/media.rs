//! The sources of the `video::`/`audio::` block macros, as the pinned Ruby
//! HTML converter builds them (`convert_video`, `convert_audio` and
//! `AbstractNode#media_uri`). Core resolves the URLs once; backends only
//! encode them. Generated query separators are a literal `&`.
use crate::ast::BlockAttrs;
use crate::Attributes;

/// A resolved video: `src` plays the target (an embedded YouTube/Vimeo
/// player when `embedded`), `poster` is the resolved poster of a plain video.
pub(crate) struct Video {
    pub src: String,
    pub poster: Option<String>,
    pub embedded: bool,
}

/// `media_uri`: the target under `imagesdir`, like an image without data URIs.
fn media_uri(target: &str, document: &Attributes) -> String {
    crate::image::web_path(target, document.get("imagesdir"))
}

/// Ruby's `#t=start,end` time anchor of a plain video or audio.
fn time_anchor(attrs: &BlockAttrs) -> String {
    let (start, end) = (attrs.named.get("start"), attrs.named.get("end"));
    if start.is_none() && end.is_none() {
        return String::new();
    }
    let end = end.map(|end| format!(",{end}")).unwrap_or_default();
    format!("#t={}{end}", start.map_or("", String::as_str))
}

/// Resolve the sources of a video or audio block with the document
/// attributes at that point.
pub(crate) fn resolve(block: &mut crate::Block, document: &Attributes) {
    match &mut block.kind {
        crate::BlockKind::Video {
            target,
            src,
            poster,
            embedded,
        } => {
            let video = video(target, &block.attrs, document);
            (*src, *poster, *embedded) = (video.src, video.poster, video.embedded);
        }
        crate::BlockKind::Audio { target, src } => *src = audio(target, &block.attrs, document),
        _ => {}
    }
}

fn option(attrs: &BlockAttrs, name: &str) -> bool {
    attrs.options.iter().any(|o| o == name)
}

/// `asset-uri-scheme` (default `https`) with its colon, empty when blank.
fn scheme(document: &Attributes) -> String {
    match document.get("asset-uri-scheme") {
        Some("") => String::new(),
        Some(scheme) => format!("{scheme}:"),
        None => "https:".to_owned(),
    }
}

/// `convert_audio`'s source.
pub(crate) fn audio(target: &str, attrs: &BlockAttrs, document: &Attributes) -> String {
    format!("{}{}", media_uri(target, document), time_anchor(attrs))
}

/// `convert_video`: the `poster` attribute selects a Vimeo or YouTube player,
/// otherwise the target and a non-empty poster are media paths.
pub(crate) fn video(target: &str, attrs: &BlockAttrs, document: &Attributes) -> Video {
    let named = &attrs.named;
    let embedded = |src| Video {
        src,
        poster: None,
        embedded: true,
    };
    match named.get("poster").map(String::as_str) {
        Some("vimeo") => embedded(vimeo(target, attrs, document)),
        Some("youtube") => embedded(youtube(target, attrs, document)),
        poster => Video {
            src: format!("{}{}", media_uri(target, document), time_anchor(attrs)),
            poster: poster
                .filter(|poster| !poster.is_empty())
                .map(|poster| media_uri(poster, document)),
            embedded: false,
        },
    }
}

fn vimeo(target: &str, attrs: &BlockAttrs, document: &Attributes) -> String {
    let named = &attrs.named;
    // Ruby's `split '/', 2`: an empty hash after the slash still counts.
    let (target, hash) = match target.split_once('/') {
        Some((target, hash)) => (target, Some(hash)),
        None => (target, named.get("hash").map(String::as_str)),
    };
    let mut params = Vec::new();
    if let Some(hash) = hash {
        params.push(format!("h={hash}"));
    }
    for (name, param) in [
        ("autoplay", "autoplay=1"),
        ("loop", "loop=1"),
        ("muted", "muted=1"),
    ] {
        if option(attrs, name) {
            params.push(param.to_owned());
        }
    }
    let query = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let start = named
        .get("start")
        .map(|start| format!("#at={start}"))
        .unwrap_or_default();
    format!(
        "{}//player.vimeo.com/video/{target}{query}{start}",
        scheme(document)
    )
}

fn youtube(target: &str, attrs: &BlockAttrs, document: &Attributes) -> String {
    let named = &attrs.named;
    let param = |name: &str, key: &str| {
        named
            .get(name)
            .map(|value| format!("&{key}={value}"))
            .unwrap_or_default()
    };
    let flag = |name: &str, param: &'static str| {
        if option(attrs, name) {
            param
        } else {
            ""
        }
    };
    let looped = option(attrs, "loop");
    // `video_id/list_id`, else `video_id1,video_id2,…` as a dynamic playlist;
    // a loop needs the video itself as its playlist.
    let (target, list) = match target.split_once('/') {
        Some((target, list)) => (target, Some(list)),
        None => (target, named.get("list").map(String::as_str)),
    };
    let (target, list) = match list {
        Some(list) => (target, format!("&list={list}")),
        None => {
            let (target, playlist) = match target.split_once(',') {
                Some((target, playlist)) => (target, Some(playlist)),
                None => (target, named.get("playlist").map(String::as_str)),
            };
            match playlist {
                Some(playlist) => (target, format!("&playlist={target},{playlist}")),
                None if looped => (target, format!("&playlist={target}")),
                None => (target, String::new()),
            }
        }
    };
    format!(
        "{}//www.youtube.com/embed/{target}?rel={}{}{}{}{}{}{}{list}{}{}{}{}",
        scheme(document),
        u8::from(option(attrs, "related")),
        param("start", "start"),
        param("end", "end"),
        flag("autoplay", "&autoplay=1"),
        flag("loop", "&loop=1"),
        flag("muted", "&mute=1"),
        flag("nocontrols", "&controls=0"),
        flag("nofullscreen", "&fs=0"),
        flag("modest", "&modestbranding=1"),
        param("theme", "theme"),
        param("lang", "hl"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(named: &[(&str, &str)], options: &[&str]) -> BlockAttrs {
        let mut attrs = BlockAttrs::default();
        for (name, value) in named {
            attrs.named.insert((*name).to_owned(), (*value).to_owned());
        }
        attrs.options = options.iter().map(|o| (*o).to_owned()).collect();
        attrs
    }

    fn document(pairs: &[(&str, &str)]) -> Attributes {
        let mut document = Attributes::new();
        for (name, value) in pairs {
            document.set(*name, *value);
        }
        document
    }

    #[test]
    fn plain_media_resolve_paths_posters_and_time_anchors() {
        let doc = document(&[("imagesdir", "img")]);
        let video = video(
            "my clip.mp4",
            &attrs(&[("poster", "p.png"), ("start", "10"), ("end", "20")], &[]),
            &doc,
        );
        assert_eq!(video.src, "img/my%20clip.mp4#t=10,20");
        assert_eq!(video.poster.as_deref(), Some("img/p.png"));
        assert!(!video.embedded);
        let empty = super::video("a.mp4", &attrs(&[("poster", "")], &[]), &doc);
        assert_eq!((empty.src.as_str(), empty.poster), ("img/a.mp4", None));
        assert_eq!(
            audio("s.ogg", &attrs(&[("end", "3")], &[]), &doc),
            "img/s.ogg#t=,3"
        );
        assert_eq!(
            audio(
                "https://x.org/a b.ogg",
                &attrs(&[("start", "1")], &[]),
                &doc
            ),
            "https://x.org/a%20b.ogg#t=1"
        );
    }

    #[test]
    fn youtube_builds_the_embed_query_in_ruby_order() {
        let doc = document(&[]);
        let all = attrs(
            &[
                ("poster", "youtube"),
                ("start", "5"),
                ("end", "9"),
                ("theme", "light"),
                ("lang", "fr"),
            ],
            &[
                "autoplay",
                "loop",
                "modest",
                "related",
                "muted",
                "nocontrols",
                "nofullscreen",
            ],
        );
        let video = video("id", &all, &doc);
        assert!(video.embedded);
        assert_eq!(
            video.src,
            "https://www.youtube.com/embed/id?rel=1&start=5&end=9&autoplay=1&loop=1&mute=1&controls=0&playlist=id&fs=0&modestbranding=1&theme=light&hl=fr"
        );
        let yt = attrs(&[("poster", "youtube")], &[]);
        assert_eq!(
            super::video("abc/PL1", &yt, &doc).src,
            "https://www.youtube.com/embed/abc?rel=0&list=PL1"
        );
        let looped = attrs(&[("poster", "youtube")], &["loop"]);
        assert_eq!(
            super::video("a,b,c", &looped, &doc).src,
            "https://www.youtube.com/embed/a?rel=0&loop=1&playlist=a,b,c"
        );
        let playlist = attrs(&[("poster", "youtube"), ("playlist", "p1")], &[]);
        assert_eq!(
            super::video("x", &playlist, &doc).src,
            "https://www.youtube.com/embed/x?rel=0&playlist=x,p1"
        );
    }

    #[test]
    fn vimeo_takes_the_hash_options_start_and_scheme() {
        let blank = document(&[("asset-uri-scheme", "")]);
        let all = attrs(
            &[("poster", "vimeo"), ("start", "60")],
            &["autoplay", "loop", "muted"],
        );
        assert_eq!(
            video("12345/hh", &all, &blank).src,
            "//player.vimeo.com/video/12345?h=hh&autoplay=1&loop=1&muted=1#at=60"
        );
        let hash = attrs(&[("poster", "vimeo"), ("hash", "zz")], &[]);
        let http = document(&[("asset-uri-scheme", "http")]);
        assert_eq!(
            video("67890", &hash, &http).src,
            "http://player.vimeo.com/video/67890?h=zz"
        );
        let plain = attrs(&[("poster", "vimeo")], &[]);
        assert_eq!(
            video("1", &plain, &document(&[])).src,
            "https://player.vimeo.com/video/1"
        );
    }
}
