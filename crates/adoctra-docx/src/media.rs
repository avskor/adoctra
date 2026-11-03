//! Images: reading them through the host's resource provider (never ambient
//! I/O), their formats and pixel sizes, the size Word draws them at, and the
//! DrawingML of an inline or floating picture.

use std::collections::BTreeMap;
use std::sync::Arc;

use adoctra_core::{ResourceProvider, ResourceRequest};

use crate::package::Relationships;
use crate::xml::escape_attr;

/// Relationship type of an image part.
pub const REL_IMAGE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";

/// EMUs per pixel at 96 DPI.
const EMU_PER_PX: u64 = 9525;
/// The size of an image whose size nothing tells (pixels).
const DEFAULT_PX: (u32, u32) = (400, 300);

/// A 1×1 transparent PNG: the fallback picture Word requires beside an SVG.
const BLANK_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0B, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x60, 0x00, 0x02, 0x00,
    0x00, 0x05, 0x00, 0x01, 0xE9, 0xFA, 0xDC, 0xD8, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44,
    0xAE, 0x42, 0x60, 0x82,
];

/// An image format Word draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Tiff,
    Svg,
}

impl Format {
    /// The format of `bytes`, by signature.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"GIF8") {
            Some(Self::Gif)
        } else if bytes.starts_with(b"BM") {
            Some(Self::Bmp)
        } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
            Some(Self::Tiff)
        } else {
            let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_lowercase();
            head.contains("<svg").then_some(Self::Svg)
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Svg => "svg",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Tiff => "image/tiff",
            Self::Svg => "image/svg+xml",
        }
    }
}

/// The pixel size of an image, from its header (or an SVG's `width`,
/// `height` or `viewBox`).
pub fn pixel_size(format: Format, bytes: &[u8]) -> Option<(u32, u32)> {
    let be16 = |at: usize| Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let le16 = |at: usize| Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
    let le32 = |at: usize| Some(i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let size = match format {
        Format::Png => (be32(16)?, be32(20)?),
        Format::Gif => (u32::from(le16(6)?), u32::from(le16(8)?)),
        Format::Bmp => (le32(18)?.unsigned_abs(), le32(22)?.unsigned_abs()),
        Format::Jpeg => {
            // The first start-of-frame marker holds the size.
            let mut at = 2;
            loop {
                if *bytes.get(at)? != 0xFF {
                    return None;
                }
                let marker = *bytes.get(at + 1)?;
                let len = usize::from(be16(at + 2)?);
                if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                    break (u32::from(be16(at + 7)?), u32::from(be16(at + 5)?));
                }
                at += 2 + len;
            }
        }
        Format::Svg => svg_size(&String::from_utf8_lossy(bytes))?,
        Format::Tiff => return None,
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

fn svg_size(text: &str) -> Option<(u32, u32)> {
    let open = crate::xml::tags(text, 0).find(|t| t.name == "svg" || t.name.ends_with(":svg"))?;
    let length = |name: &str| open.attr(name).and_then(|v| css_px(&v));
    if let (Some(w), Some(h)) = (length("width"), length("height")) {
        return Some((w.round() as u32, h.round() as u32));
    }
    let view_box = open.attr("viewBox")?;
    let numbers: Vec<f64> = view_box
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter_map(|n| n.parse().ok())
        .collect();
    match numbers[..] {
        [_, _, w, h] => {
            // One given dimension scales the view box.
            match (length("width"), length("height")) {
                (Some(width), None) => Some((width.round() as u32, (width * h / w).round() as u32)),
                (None, Some(height)) => {
                    Some(((height * w / h).round() as u32, height.round() as u32))
                }
                _ => Some((w.round() as u32, h.round() as u32)),
            }
        }
        _ => None,
    }
}

/// A CSS absolute length in pixels (unitless, `px`, `pt`, `pc`, `in`, `cm`,
/// `mm`); `None` for a relative one.
pub fn css_px(value: &str) -> Option<f64> {
    let value = value.trim();
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(value.len());
    let number: f64 = value[..split].parse().ok()?;
    let factor = match value[split..].trim() {
        "" | "px" => 1.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        _ => return None,
    };
    (number > 0.0).then_some(number * factor)
}

/// The size (EMU) an image is drawn at: `pdfwidth`, `scaledwidth` or
/// `width` (pixels, an absolute length, or a percentage of the available
/// width) and `height`, keeping the aspect ratio of the pixel size when one
/// dimension is given, never wider than `available` (twips).
pub fn draw_size(
    attrs: &BTreeMap<String, String>,
    pixels: Option<(u32, u32)>,
    available: u32,
) -> (u64, u64) {
    let available_px = f64::from(available) / 15.0;
    let width = ["pdfwidth", "scaledwidth", "width"]
        .iter()
        .find_map(|key| attrs.get(*key))
        .and_then(|w| match w.trim().strip_suffix('%') {
            Some(pc) => pc
                .trim()
                .parse::<f64>()
                .ok()
                .map(|pc| available_px * pc / 100.0),
            None => css_px(w),
        });
    let height = attrs.get("height").and_then(|h| css_px(h));
    let (pw, ph) = pixels.map_or(
        (f64::from(DEFAULT_PX.0), f64::from(DEFAULT_PX.1)),
        |(w, h)| (f64::from(w), f64::from(h)),
    );
    let (mut w, mut h) = match (width, height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, w * ph / pw),
        (None, Some(h)) => (h * pw / ph, h),
        (None, None) => (pw, ph),
    };
    if w > available_px {
        h *= available_px / w;
        w = available_px;
    }
    let emu = |px: f64| (px.max(1.0) * EMU_PER_PX as f64).round() as u64;
    (emu(w), emu(h))
}

/// An image in the package: its relationship (and an SVG's fallback) and
/// pixel size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub embed: String,
    /// The SVG's relationship, `embed` being the blank PNG fallback.
    pub svg: Option<String>,
    /// An external (linked) picture: `embed` is an `r:link`.
    pub linked: bool,
    pub pixels: Option<(u32, u32)>,
}

/// Where images come from and the media parts added for them.
pub struct Media {
    provider: Option<Arc<dyn ResourceProvider>>,
    referrer: Option<String>,
    /// Added parts: name, bytes, extension, content type.
    pub parts: Vec<(String, Vec<u8>, &'static str, &'static str)>,
    loaded: BTreeMap<String, Result<Picture, String>>,
    blank: Option<String>,
    next_doc_pr: u32,
}

impl Media {
    pub fn new(provider: Option<Arc<dyn ResourceProvider>>, referrer: Option<String>) -> Self {
        Self {
            provider,
            referrer,
            parts: Vec::new(),
            loaded: BTreeMap::new(),
            blank: None,
            next_doc_pr: 1,
        }
    }

    /// A fresh drawing object id.
    pub fn doc_pr(&mut self) -> u32 {
        let id = self.next_doc_pr;
        self.next_doc_pr += 1;
        id
    }

    /// The picture for an image source (a web path relative to the document,
    /// a data URI or a URI): read once, added once. A URI the provider does
    /// not read is linked. The error is the reason for a warning.
    pub fn picture(
        &mut self,
        src: &str,
        rels: &mut Relationships,
        allow_uri: bool,
    ) -> Result<Picture, String> {
        if let Some(found) = self.loaded.get(src) {
            return found.clone();
        }
        let result = self.load(src, rels, allow_uri);
        self.loaded.insert(src.to_string(), result.clone());
        result
    }

    fn load(
        &mut self,
        src: &str,
        rels: &mut Relationships,
        allow_uri: bool,
    ) -> Result<Picture, String> {
        let bytes = if let Some(data) = src.strip_prefix("data:") {
            let (_, payload) = data.split_once(";base64,").ok_or("unsupported data URI")?;
            base64_decode(payload).ok_or("invalid base64 data URI")?
        } else if is_uri(src) && !allow_uri {
            let id = rels.add(REL_IMAGE, src, true);
            return Ok(Picture {
                embed: id,
                svg: None,
                linked: true,
                pixels: None,
            });
        } else {
            let provider = self
                .provider
                .as_ref()
                .ok_or("images cannot be read without a resource provider")?;
            provider
                .load_binary(ResourceRequest {
                    target: src,
                    referrer: self.referrer.as_deref(),
                })
                .map_err(|_| "image not found or unreadable")?
                .bytes
        };
        let format = Format::sniff(&bytes).ok_or("unsupported image format")?;
        let pixels = pixel_size(format, &bytes);
        let id = self.add_part(bytes, format, rels);
        if format == Format::Svg {
            let blank = match &self.blank {
                Some(blank) => blank.clone(),
                None => {
                    let blank = self.add_part(BLANK_PNG.to_vec(), Format::Png, rels);
                    self.blank = Some(blank.clone());
                    blank
                }
            };
            return Ok(Picture {
                embed: blank,
                svg: Some(id),
                linked: false,
                pixels,
            });
        }
        Ok(Picture {
            embed: id,
            svg: None,
            linked: false,
            pixels,
        })
    }

    fn add_part(&mut self, bytes: Vec<u8>, format: Format, rels: &mut Relationships) -> String {
        let name = format!("media/image{}.{}", self.parts.len() + 1, format.extension());
        let id = rels.add(REL_IMAGE, &name, false);
        self.parts.push((
            format!("word/{name}"),
            bytes,
            format.extension(),
            format.content_type(),
        ));
        id
    }
}

fn is_uri(s: &str) -> bool {
    s.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    })
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// How a picture sits in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Inline,
    /// Floating at the left or right margin, text wrapping around it.
    Float(&'static str),
}

/// The `w:drawing` run of a picture.
pub fn drawing(
    picture: &Picture,
    (cx, cy): (u64, u64),
    id: u32,
    name: &str,
    alt: &str,
    placement: Placement,
) -> String {
    let blip_ref = if picture.linked { "r:link" } else { "r:embed" };
    let extension = match &picture.svg {
        Some(svg) => format!(
            r#"<a:extLst><a:ext uri="{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}"><asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main" r:embed="{svg}"/></a:ext></a:extLst>"#
        ),
        None => String::new(),
    };
    let name = escape_attr(name);
    let alt = escape_attr(alt);
    let graphic = format!(
        r#"<wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{id}" name="{name}" descr="{alt}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip {blip_ref}="{}">{extension}</a:blip><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>"#,
        picture.embed
    );
    let doc_pr = format!(r#"<wp:docPr id="{id}" name="{name}" descr="{alt}"/>"#);
    let body = match placement {
        Placement::Inline => format!(
            r#"<wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/>{doc_pr}{graphic}</wp:inline>"#
        ),
        Placement::Float(side) => format!(
            r#"<wp:anchor distT="0" distB="114300" distL="114300" distR="114300" simplePos="0" relativeHeight="{id}" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:align>{side}</wp:align></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapSquare wrapText="bothSides"/>{doc_pr}{graphic}</wp:anchor>"#
        ),
    };
    format!("<w:r><w:drawing>{body}</w:drawing></w:r>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b
    }

    #[test]
    fn reads_formats_and_pixel_sizes() {
        assert_eq!(Format::sniff(&png(3, 4)), Some(Format::Png));
        assert_eq!(pixel_size(Format::Png, &png(300, 200)), Some((300, 200)));
        let gif = b"GIF89a\x40\x01\xc8\x00".to_vec();
        assert_eq!(pixel_size(Format::Gif, &gif), Some((320, 200)));
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x64, 0x00, 0xC8]);
        assert_eq!(Format::sniff(&jpeg), Some(Format::Jpeg));
        assert_eq!(pixel_size(Format::Jpeg, &jpeg), Some((200, 100)));
        let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 60"/>"#;
        assert_eq!(Format::sniff(svg), Some(Format::Svg));
        assert_eq!(pixel_size(Format::Svg, svg), Some((120, 60)));
        let sized = br#"<svg width="2in" viewBox="0 0 100 50">"#;
        assert_eq!(pixel_size(Format::Svg, sized), Some((192, 96)));
        assert_eq!(Format::sniff(b"RIFF....WEBP"), None);
        assert_eq!(pixel_size(Format::Png, b"\x89PNG"), None);
    }

    #[test]
    fn converts_css_lengths() {
        assert_eq!(css_px("120"), Some(120.0));
        assert_eq!(css_px("1in"), Some(96.0));
        assert_eq!(css_px("72pt"), Some(96.0));
        assert_eq!(css_px("2.54cm").map(f64::round), Some(96.0));
        assert_eq!(css_px("50%"), None);
        assert_eq!(css_px("10vw"), None);
    }

    #[test]
    fn sizes_keep_the_aspect_ratio_and_fit_the_text_width() {
        let attrs = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        let px = |n: u64| n * EMU_PER_PX;
        assert_eq!(
            draw_size(&attrs(&[]), Some((200, 100)), 9000),
            (px(200), px(100))
        );
        assert_eq!(
            draw_size(&attrs(&[("width", "100")]), Some((200, 100)), 9000),
            (px(100), px(50))
        );
        assert_eq!(
            draw_size(&attrs(&[("height", "50")]), Some((200, 100)), 9000),
            (px(100), px(50))
        );
        // A percentage of the 600 px text width (9000 twips).
        assert_eq!(
            draw_size(&attrs(&[("scaledwidth", "50%")]), Some((200, 100)), 9000),
            (px(300), px(150))
        );
        // pdfwidth wins over width.
        assert_eq!(
            draw_size(
                &attrs(&[("width", "10"), ("pdfwidth", "1in")]),
                Some((200, 100)),
                9000
            ),
            (px(96), px(48))
        );
        // Too wide: scaled down to the text width.
        assert_eq!(
            draw_size(&attrs(&[]), Some((1200, 600)), 9000),
            (px(600), px(300))
        );
        assert_eq!(draw_size(&attrs(&[]), None, 9000), (px(400), px(300)));
    }

    #[test]
    fn the_svg_fallback_is_a_valid_png() {
        assert_eq!(Format::sniff(BLANK_PNG), Some(Format::Png));
        assert_eq!(pixel_size(Format::Png, BLANK_PNG), Some((1, 1)));
        // Chunk lengths add up to the whole file: IHDR, IDAT, IEND.
        let mut at = 8;
        let mut names = Vec::new();
        while at < BLANK_PNG.len() {
            let len = u32::from_be_bytes(BLANK_PNG[at..at + 4].try_into().unwrap()) as usize;
            names.push(String::from_utf8_lossy(&BLANK_PNG[at + 4..at + 8]).into_owned());
            at += 12 + len;
        }
        assert_eq!(at, BLANK_PNG.len());
        assert_eq!(names, ["IHDR", "IDAT", "IEND"]);
    }

    #[test]
    fn decodes_base64() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVs\nbG8").unwrap(), b"hello");
        assert!(base64_decode("a*b").is_none());
    }

    #[test]
    fn data_uris_are_embedded_and_uris_linked() {
        let mut media = Media::new(None, None);
        let mut rels = Relationships::default();
        let src = format!("data:image/png;base64,{}", encode(&png(10, 20)));
        let picture = media.picture(&src, &mut rels, false).unwrap();
        assert_eq!(picture.pixels, Some((10, 20)));
        assert_eq!(media.parts[0].0, "word/media/image1.png");
        // The same source is added once.
        assert_eq!(media.picture(&src, &mut rels, false).unwrap(), picture);
        assert_eq!(media.parts.len(), 1);
        let linked = media
            .picture("https://example.org/a.png", &mut rels, false)
            .unwrap();
        assert!(linked.linked);
        assert!(rels
            .to_xml()
            .contains(r#"Target="https://example.org/a.png" TargetMode="External""#));
        // Without a provider, a path cannot be read.
        assert_eq!(
            media.picture("a.png", &mut rels, false).unwrap_err(),
            "images cannot be read without a resource provider"
        );
    }

    #[test]
    fn svg_gets_a_png_fallback() {
        let mut media = Media::new(None, None);
        let mut rels = Relationships::default();
        let src = format!(
            "data:image/svg+xml;base64,{}",
            encode(br#"<svg viewBox="0 0 4 2"/>"#)
        );
        let picture = media.picture(&src, &mut rels, false).unwrap();
        assert!(picture.svg.is_some());
        assert_eq!(media.parts.len(), 2);
        assert_eq!(media.parts[1].0, "word/media/image2.png");
        let xml = drawing(&picture, (100, 50), 1, "a", "alt", Placement::Inline);
        assert!(xml.contains("asvg:svgBlip"));
        assert!(xml.contains(r#"<wp:inline distT="0""#));
        let float = drawing(
            &picture,
            (100, 50),
            2,
            "a",
            "alt",
            Placement::Float("right"),
        );
        assert!(float.contains("<wp:align>right</wp:align>") && float.contains("<wp:wrapSquare"));
    }

    fn encode(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n =
                chunk.iter().fold(0u32, |n, b| (n << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(A[(n >> (18 - 6 * i)) as usize & 63] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}
