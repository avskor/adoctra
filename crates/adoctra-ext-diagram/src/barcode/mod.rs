//! asciidoctor-diagram's `BarcodeConverter`: barby 0.6.9 barcodes (and
//! rqrcode_core QR codes) as SVG (barby's `SvgOutputter` with
//! asciidoctor-diagram's foreground patch), PNG (barby's `PngOutputter` on
//! chunky_png 1.4.0) or text (`AsciiOutputter`).

pub(crate) mod barby;
mod code128;
mod deflate;
mod png;
mod qr;

use crate::converter::{Converter, OptValue, Options};
use crate::host::DiagramHost;
use crate::source::Source;
use barby::{Encoding, Failure};

/// The barcode types in Ruby's `BARCODE_TYPES` order.
pub(crate) const BARCODE_TYPES: [&str; 16] = [
    "bookland",
    "codabar",
    "code25",
    "code25iata",
    "code25interleaved",
    "code39",
    "code93",
    "code128",
    "code128a",
    "code128b",
    "code128c",
    "ean8",
    "ean13",
    "gs1_128",
    "qrcode",
    "upca",
];

pub(crate) struct Barcode {
    pub kind: &'static str,
}

/// Ruby's `String#to_i`.
fn to_i(text: &str) -> i64 {
    let text = text.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let mut value: i64 = 0;
    let mut previous_digit = false;
    for b in digits.bytes() {
        match b {
            b'0'..=b'9' => {
                value = value.saturating_mul(10).saturating_add(i64::from(b - b'0'));
                previous_digit = true;
            }
            b'_' if previous_digit => previous_digit = false,
            _ => break,
        }
    }
    if negative {
        -value
    } else {
        value
    }
}

fn int(options: &Options, name: &str) -> Option<i64> {
    options.iter().find_map(|(key, value)| match value {
        OptValue::Int(n) if *key == name => Some(*n),
        _ => None,
    })
}

fn string<'o>(options: &'o Options, name: &str) -> Option<&'o str> {
    crate::converter::option(options, name)
}

/// SVG's foreground and background: a hexadecimal value gets a `#`
/// (`/^[0-9a-f]+$/i`, any line).
fn svg_color(value: &str) -> String {
    if barby::some_line(value, |line| {
        !line.is_empty() && line.bytes().all(|b| b.is_ascii_hexdigit())
    }) {
        format!("#{value}")
    } else {
        value.to_owned()
    }
}

impl Barcode {
    fn encoding(&self, code: &str) -> Result<Encoding, String> {
        barby::encode(self.kind, code).map_err(|failure| match failure {
            Failure::Invalid => format!("Invalid {} data", self.kind),
            Failure::Message(message) => message,
        })
    }
}

impl Converter for Barcode {
    fn supported_formats(&self) -> &'static [&'static str] {
        &["svg", "png", "txt"]
    }

    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        let number = |name: &str| match source.attr(name) {
            Some(value) => OptValue::Int(to_i(&value)),
            None => OptValue::Null,
        };
        Ok(vec![
            ("xdim", number("xdim")),
            ("ydim", number("ydim")),
            ("margin", number("margin")),
            ("height", number("height")),
            ("foreground", source.attr("foreground").into()),
            ("background", source.attr("background").into()),
        ])
    }

    /// Ruby changes the options hash it stores: SVG gets the `title` and
    /// `#`-prefixed colors, PNG chunky_png color values.
    fn stored_options(&self, options: &Options, format: &str, source: &Source<'_, '_>) -> Options {
        let mut stored = options.clone();
        for (key, value) in stored.iter_mut() {
            if let ("foreground" | "background", OptValue::Str(color)) = (*key, &*value) {
                *value = match format {
                    "svg" => OptValue::Str(svg_color(color)),
                    "png" => match png::parse_color(color) {
                        Ok(rgba) => OptValue::Int(i64::from(rgba)),
                        Err(_) => value.clone(),
                    },
                    _ => value.clone(),
                };
            }
        }
        if format == "svg" {
            stored.push(("title", OptValue::Str(source.loaded_code().to_owned())));
        }
        stored
    }

    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let code = self.code(source, host)?;
        let encoding = self.encoding(&code)?;
        let dims = Dimensions {
            xdim: int(options, "xdim"),
            ydim: int(options, "ydim"),
            margin: int(options, "margin"),
            height: int(options, "height"),
        };
        match format {
            "txt" => Ok(ascii(&encoding).into_bytes()),
            "svg" => {
                let foreground = string(options, "foreground").map(svg_color);
                let background = string(options, "background").map(svg_color);
                Ok(svg(&encoding, &dims, &code, foreground, background).into_bytes())
            }
            "png" => {
                let color = |name| string(options, name).map(png::parse_color).transpose();
                let foreground = color("foreground")?;
                let background = color("background")?;
                png::barcode_png(&encoding, &dims, foreground, background)
            }
            other => Err(format!("Unsupported format: {other}")),
        }
    }
}

/// The outputters' size options (`nil` when not given).
pub(crate) struct Dimensions {
    pub xdim: Option<i64>,
    pub ydim: Option<i64>,
    pub margin: Option<i64>,
    pub height: Option<i64>,
}

impl Dimensions {
    pub fn xdim(&self) -> i64 {
        self.xdim.unwrap_or(1)
    }
    pub fn ydim(&self) -> i64 {
        self.ydim.unwrap_or_else(|| self.xdim())
    }
    pub fn margin(&self) -> i64 {
        self.margin.unwrap_or(10)
    }
    /// `height`: the rows times `ydim` for a matrix, else `@height || 100`.
    pub fn height(&self, encoding: &Encoding) -> i64 {
        match encoding {
            Encoding::Matrix(rows) => self.ydim() * rows.len() as i64,
            Encoding::Linear(_) => self.height.unwrap_or(100),
        }
    }
    /// `length * xdim`.
    pub fn width(&self, encoding: &Encoding) -> i64 {
        let length = match encoding {
            Encoding::Matrix(rows) => rows.first().map_or(0, |row| row.len()),
            Encoding::Linear(bars) => bars.len(),
        };
        length as i64 * self.xdim()
    }
}

/// `AsciiOutputter#to_ascii` with its defaults.
fn ascii(encoding: &Encoding) -> String {
    match encoding {
        Encoding::Linear(bars) => {
            let line: String = bars
                .chars()
                .map(|b| if b == '1' { '#' } else { ' ' })
                .collect();
            vec![line; 10].join("\n")
        }
        Encoding::Matrix(rows) => rows
            .iter()
            .map(|row| {
                row.chars()
                    .map(|b| if b == '1' { " X " } else { "   " })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Runs of equal modules: `(bar, count)`.
fn groups(bars: &str) -> Vec<(bool, i64)> {
    let mut out: Vec<(bool, i64)> = Vec::new();
    for b in bars.chars() {
        let bar = b == '1';
        match out.last_mut() {
            Some((last, count)) if *last == bar => *count += 1,
            _ => out.push((bar, 1)),
        }
    }
    out
}

/// `SvgOutputter#to_svg`: rectangles for an odd `xdim`, else a path.
fn svg(
    encoding: &Encoding,
    dims: &Dimensions,
    title: &str,
    foreground: Option<String>,
    background: Option<String>,
) -> String {
    let xdim = dims.xdim();
    let ydim = dims.ydim();
    let margin = dims.margin();
    let height = dims.height(encoding);
    let full_width = dims.width(encoding) + 2 * margin;
    let full_height = height + 2 * margin;
    let foreground = foreground.unwrap_or_else(|| "#000".to_owned());
    let background = background.unwrap_or_else(|| "#fff".to_owned());
    let rows: Vec<&str> = match encoding {
        Encoding::Linear(bars) => vec![bars.as_str()],
        Encoding::Matrix(rows) => rows.iter().map(String::as_str).collect(),
    };
    let matrix = matches!(encoding, Encoding::Matrix(_));
    let mut bars = String::new();
    if xdim.rem_euclid(2) == 1 {
        let mut y = margin;
        for row in &rows {
            let mut x = margin;
            for (bar, amount) in groups(row) {
                let width = xdim * amount;
                if bar {
                    let bar_height = if matrix { ydim } else { height };
                    bars.push_str(&format!(
                        "<rect x=\"{x}\" y=\"{y}\" width=\"{width}px\" height=\"{bar_height}px\" />\n"
                    ));
                }
                x += width;
            }
            y += ydim;
        }
    } else {
        let mut data = String::new();
        let mut y = margin;
        let start = margin + xdim.div_euclid(2);
        for row in &rows {
            let mut x = start;
            for b in row.chars() {
                if b == '1' {
                    if matrix {
                        data.push_str(&format!("M{x} {y}V {}", y + ydim));
                    } else {
                        data.push_str(&format!("M{x} {y}V{}", y + height));
                    }
                }
                x += xdim;
            }
            y += ydim;
        }
        bars = format!("<path stroke=\"{foreground}\" stroke-width=\"{xdim}\" d=\"{data}\" />");
    }
    let title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         width=\"{full_width}px\" height=\"{full_height}px\" viewBox=\"0 0 {full_width} {full_height}\" \
         version=\"1.1\" preserveAspectRatio=\"none\" >\n\
         <title>{title}</title>\n\
         <g id=\"canvas\" >\n\
         <rect x=\"0\" y=\"0\" width=\"{full_width}px\" height=\"{full_height}px\" fill=\"{background}\" />\n\
         <g id=\"barcode\" fill=\"{foreground}\">\n\
         {bars}\n\
         </g></g>\n\
         </svg>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruby_to_i_and_colors() {
        assert_eq!(to_i(" 12px"), 12);
        assert_eq!(to_i("-3"), -3);
        assert_eq!(to_i("1_000"), 1000);
        assert_eq!(to_i("x"), 0);
        assert_eq!(svg_color("ff0000"), "#ff0000");
        assert_eq!(svg_color("red"), "red");
    }
}
