//! barby's `PngOutputter` on chunky_png 1.4.0: `ChunkyPNG::Color.parse`,
//! the canvas, and `Canvas#to_datastream` with its default encoding (color
//! mode and bit depth from the palette, UP filtering, zlib's default
//! compression).

use super::barby::Encoding;
use super::Dimensions;

include!("colors.rs");

const BLACK: u32 = 0x0000_00ff;
const WHITE: u32 = 0xffff_ffff;

/// `ChunkyPNG::Color.parse` (via `ChunkyPNG::Color(value)`).
pub(crate) fn parse_color(source: &str) -> Result<u32, String> {
    // `/^\d+$/` on any line, then `to_i` of the whole value.
    if super::barby::some_line(source, |line| {
        !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit())
    }) {
        return Ok(super::to_i(source) as u32);
    }
    if let Some(color) = hex_color(source) {
        return Ok(color);
    }
    if html_line(source).is_some() {
        return html_color(source);
    }
    Err(format!(
        "Don't know how to create a color from {}!",
        crate::image::rexml::ruby_inspect(source)
    ))
}

/// `HEX3_COLOR_REGEXP`/`HEX6_COLOR_REGEXP` (with `from_hex`).
fn hex_color(source: &str) -> Option<u32> {
    let lower = source.to_ascii_lowercase();
    let digits = lower
        .strip_prefix('#')
        .or_else(|| lower.strip_prefix("0x"))
        .unwrap_or(&lower);
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        3 => {
            let doubled: String = digits.chars().flat_map(|c| [c, c]).collect();
            Some(u32::from_str_radix(&doubled, 16).ok()? << 8 | 0xff)
        }
        6 => Some(u32::from_str_radix(digits, 16).ok()? << 8 | 0xff),
        8 => Some(u32::from_str_radix(digits, 16).ok()?),
        _ => None,
    }
}

/// `HTML_COLOR_REGEXP` on the first line it matches: the name and the
/// opacity.
fn html_line(source: &str) -> Option<(String, Option<String>)> {
    let name_ok = |name: &str| {
        let b = name.as_bytes();
        b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[b.len() - 1].is_ascii_alphabetic()
            && b.iter()
                .all(|c| c.is_ascii_alphabetic() || *c == b'_' || *c == b' ')
    };
    let mut lines: Vec<&str> = source.split('\n').collect();
    if lines.len() > 1 && lines.last() == Some(&"") {
        lines.pop();
    }
    lines.into_iter().find_map(|line| {
        if name_ok(line) {
            return Some((line.to_owned(), None));
        }
        let (left, right) = line.split_once('@')?;
        let left = left.strip_suffix(' ').unwrap_or(left);
        let right = right.strip_prefix(' ').unwrap_or(right);
        let opacity_ok = right == "1.0"
            || right
                .strip_prefix("0.")
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
        (name_ok(left) && opacity_ok).then(|| (left.to_owned(), Some(right.to_owned())))
    })
}

/// `html_color`.
fn html_color(source: &str) -> Result<u32, String> {
    if let Some((name, opacity)) = html_line(source) {
        let opacity = match opacity {
            Some(value) => (value.parse::<f64>().unwrap_or(0.0) * 255.0).round() as u32,
            None => 0xff,
        };
        let key: String = name
            .chars()
            .filter(char::is_ascii_alphabetic)
            .collect::<String>()
            .to_ascii_lowercase();
        if let Ok(at) = PREDEFINED_COLORS.binary_search_by(|(n, _)| (*n).cmp(key.as_str())) {
            return Ok(PREDEFINED_COLORS[at].1 | opacity);
        }
    }
    Err(format!("Unknown color name {source}!"))
}

/// `PngOutputter#to_png`: the barcode drawn on a canvas, then encoded.
pub(crate) fn barcode_png(
    encoding: &Encoding,
    dims: &Dimensions,
    foreground: Option<u32>,
    background: Option<u32>,
) -> Result<Vec<u8>, String> {
    let foreground = foreground.unwrap_or(BLACK);
    let background = background.unwrap_or(WHITE);
    let margin = dims.margin();
    let width = dims.width(encoding) + 2 * margin;
    let height = dims.height(encoding) + 2 * margin;
    if width * height < 0 {
        return Err("negative array size".to_owned());
    }
    let mut pixels = vec![background; (width * height) as usize];
    let (xdim, ydim) = (dims.xdim(), dims.ydim());
    let rows: Vec<&str> = match encoding {
        Encoding::Linear(bars) => vec![bars.as_str()],
        Encoding::Matrix(rows) => rows.iter().map(String::as_str).collect(),
    };
    let bar_height = match encoding {
        Encoding::Linear(_) => dims.height(encoding),
        Encoding::Matrix(_) => ydim,
    };
    let mut y = margin;
    for row in rows {
        let mut x = margin;
        for b in row.chars() {
            if b == '1' {
                for xx in x..x + xdim {
                    for yy in y..y + bar_height {
                        if !(0..width).contains(&xx) || !(0..height).contains(&yy) {
                            return Err(format!("Coordinates ({xx},{yy}) out of bounds!"));
                        }
                        pixels[(yy * width + xx) as usize] = foreground;
                    }
                }
            }
            x += xdim;
        }
        y += ydim;
    }
    Ok(encode(
        width.max(0) as usize,
        height.max(0) as usize,
        &pixels,
    ))
}

const GRAYSCALE: u8 = 0;
const TRUECOLOR: u8 = 2;
const INDEXED: u8 = 3;
const GRAYSCALE_ALPHA: u8 = 4;
const TRUECOLOR_ALPHA: u8 = 6;

fn opaque(c: u32) -> bool {
    c & 0xff == 0xff
}
fn grayscale(c: u32) -> bool {
    let (r, g, b) = (c >> 24, (c >> 16) & 0xff, (c >> 8) & 0xff);
    r == g && g == b
}

/// `Canvas#to_datastream({})`.
fn encode(width: usize, height: usize, pixels: &[u32]) -> Vec<u8> {
    let mut palette: Vec<u32> = pixels.to_vec();
    palette.sort_unstable();
    palette.dedup();
    let all_opaque = palette.iter().all(|c| opaque(*c));
    // `best_color_settings`.
    let (mode, depth) = if palette == [BLACK, WHITE] {
        (GRAYSCALE, 1)
    } else if palette.iter().all(|c| grayscale(*c)) {
        if all_opaque {
            (GRAYSCALE, 8)
        } else {
            (GRAYSCALE_ALPHA, 8)
        }
    } else if palette.len() <= 256 {
        let depth = match palette.len() {
            0..=2 => 1,
            3..=4 => 2,
            5..=16 => 4,
            _ => 8,
        };
        (INDEXED, depth)
    } else if all_opaque {
        (TRUECOLOR, 8)
    } else {
        (TRUECOLOR_ALPHA, 8)
    };
    let index = |c: u32| palette.binary_search(&c).unwrap_or(0) as u8;
    let pack_bits = |row: &[u32], bits: usize, value: &dyn Fn(u32) -> u8| -> Vec<u8> {
        let per = 8 / bits;
        row.chunks(per)
            .map(|chunk| {
                (0..per).fold(0u8, |acc, i| {
                    let v = chunk.get(i).map_or(0, |c| value(*c));
                    acc | v << (8 - bits * (i + 1))
                })
            })
            .collect()
    };
    let (pixel_size, line_width) = match mode {
        TRUECOLOR => (3, width * 3),
        TRUECOLOR_ALPHA => (4, width * 4),
        GRAYSCALE_ALPHA => (2, width * 2),
        _ => (1, (width * depth).div_ceil(8)),
    };
    let mut stream = Vec::with_capacity(height * (line_width + 1));
    for row in pixels.chunks(width.max(1)).take(height) {
        stream.push(0);
        match (mode, depth) {
            (GRAYSCALE, 1) => stream.extend(pack_bits(row, 1, &|c| ((c & 0xffff) >> 15) as u8)),
            (GRAYSCALE, _) => stream.extend(row.iter().map(|c| (c >> 8) as u8)),
            (GRAYSCALE_ALPHA, _) => stream.extend(
                row.iter()
                    .flat_map(|c| ((*c & 0xffff) as u16).to_be_bytes()),
            ),
            (INDEXED, 8) => stream.extend(row.iter().map(|c| index(*c))),
            (INDEXED, bits) => stream.extend(pack_bits(row, bits, &index)),
            (TRUECOLOR, _) => {
                stream.extend(row.iter().flat_map(|c| c.to_be_bytes().into_iter().take(3)))
            }
            _ => stream.extend(row.iter().flat_map(|c| c.to_be_bytes())),
        }
    }
    // FILTER_UP, from the last line up (each against the raw line above).
    let _ = pixel_size;
    for y in (0..height).rev() {
        let pos = y * (line_width + 1);
        for i in (1..=line_width).rev() {
            let above = if y == 0 {
                0
            } else {
                stream[pos - (line_width + 1) + i]
            };
            stream[pos + i] = stream[pos + i].wrapping_sub(above);
        }
        stream[pos] = 2;
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut header = Vec::new();
    header.extend((width as u32).to_be_bytes());
    header.extend((height as u32).to_be_bytes());
    header.extend([depth as u8, mode, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    if mode == INDEXED {
        let plte: Vec<u8> = palette
            .iter()
            .flat_map(|c| c.to_be_bytes().into_iter().take(3))
            .collect();
        chunk(&mut out, b"PLTE", &plte);
        if !all_opaque {
            let trns: Vec<u8> = palette.iter().map(|c| *c as u8).collect();
            chunk(&mut out, b"tRNS", &trns);
        }
    }
    chunk(&mut out, b"IDAT", &super::deflate::deflate(&stream));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], content: &[u8]) {
    out.extend((content.len() as u32).to_be_bytes());
    out.extend(kind);
    out.extend(content);
    let crc = crc32(kind.iter().chain(content));
    out.extend(crc.to_be_bytes());
}

fn crc32<'a>(bytes: impl Iterator<Item = &'a u8>) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse_like_chunky_png() {
        assert_eq!(parse_color("ff0000"), Ok(0xff0000ff));
        assert_eq!(parse_color("#abc"), Ok(0xaabbccff));
        assert_eq!(parse_color("0x11223344"), Ok(0x11223344));
        assert_eq!(parse_color("123"), Ok(123));
        assert_eq!(
            parse_color("Navy Blue"),
            Err("Unknown color name Navy Blue!".to_owned())
        );
        assert_eq!(parse_color("red @ 0.5"), Ok(0xff000080));
        assert_eq!(parse_color("Dark_Red"), Ok(0x8b0000ff));
        assert_eq!(
            parse_color("#zz"),
            Err("Don't know how to create a color from \"#zz\"!".to_owned())
        );
    }
}
