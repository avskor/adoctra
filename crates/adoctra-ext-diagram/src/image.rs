//! Post-processing of generated images (asciidoctor-diagram's `PNG`, `GIF`
//! and `SVG` modules): the intrinsic size and, for SVG, the root attributes
//! Ruby adds.

/// A size as Ruby keeps it: an integer, or a float after a `pt` factor or a
/// decimal value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Number {
    Int(i64),
    Float(f64),
}

impl Number {
    /// Ruby's `(value * factor).to_i`.
    pub fn scaled(self, factor: f64) -> i64 {
        match self {
            Number::Int(value) if factor == 1.0 => value,
            Number::Int(value) => (value as f64 * factor) as i64,
            Number::Float(value) => (value * factor) as i64,
        }
    }

    /// The JSON (and Ruby `to_s`) spelling.
    pub fn json(self) -> String {
        match self {
            Number::Int(value) => value.to_string(),
            Number::Float(value) => ruby_float(value),
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        if text.contains('.') {
            text.parse().ok().map(Number::Float)
        } else {
            text.parse().ok().map(Number::Int)
        }
    }

    fn times(self, factor: f64) -> Self {
        match self {
            Number::Int(value) if factor == 1.0 => Number::Int(value),
            Number::Int(value) => Number::Float(value as f64 * factor),
            Number::Float(value) => Number::Float(value * factor),
        }
    }

    fn minus(self, other: Self) -> Self {
        match (self, other) {
            (Number::Int(a), Number::Int(b)) => Number::Int(a - b),
            (a, b) => Number::Float(a.as_f64() - b.as_f64()),
        }
    }

    fn as_f64(self) -> f64 {
        match self {
            Number::Int(value) => value as f64,
            Number::Float(value) => value,
        }
    }
}

/// Ruby's `Float#to_s` for ordinary magnitudes (always a decimal point).
fn ruby_float(value: f64) -> String {
    let text = format!("{value}");
    if text.contains(['.', 'e', 'i', 'N']) {
        text
    } else {
        format!("{text}.0")
    }
}

/// A processed image with its size.
pub(crate) struct Processed {
    pub data: Vec<u8>,
    pub width: Option<Number>,
    pub height: Option<Number>,
}

pub(crate) fn post_process(
    format: &str,
    data: Vec<u8>,
    optimise: bool,
) -> Result<Processed, String> {
    match format {
        "png" => png(data),
        "gif" => gif(data),
        "svg" => svg(data, optimise),
        "pdf" => Ok(Processed {
            data,
            width: None,
            height: None,
        }),
        // Formats a converter offers without `IMAGE_PARAMS` (bpmn's `jpeg`,
        // gnuplot's `literal`): Ruby's `params[:encoding]` on `nil`.
        _ => Err("undefined method `[]' for nil".to_owned()),
    }
}

fn png(data: Vec<u8>) -> Result<Processed, String> {
    const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if data.len() < 8 || data[..8] != SIGNATURE {
        return Err("Invalid PNG signature".to_owned());
    }
    let u32_at = |at: usize| {
        data.get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let length = u32_at(8).unwrap_or(0);
    let kind = data
        .get(12..16)
        .map(|b| String::from_utf8_lossy(b).into_owned());
    if kind.as_deref() != Some("IHDR") {
        return Err(format!(
            "Unexpected PNG chunk type '{}'; expected 'IHDR'",
            kind.unwrap_or_default()
        ));
    }
    if length != 13 {
        return Err(format!(
            "Unexpected PNG chunk length '{length}'; expected '13'"
        ));
    }
    let width = u32_at(16).map(|w| Number::Int(w.into()));
    let height = u32_at(20).map(|h| Number::Int(h.into()));
    Ok(Processed {
        data,
        width,
        height,
    })
}

fn gif(data: Vec<u8>) -> Result<Processed, String> {
    if !(data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) {
        return Err("Invalid GIF signature".to_owned());
    }
    let u16_at = |at: usize| {
        data.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let width = u16_at(6).map(|w| Number::Int(w.into()));
    let height = u16_at(8).map(|h| Number::Int(h.into()));
    Ok(Processed {
        data,
        width,
        height,
    })
}

/// `SVG.post_process_image`: Ruby reads the size from `width`/`height`
/// (`pt` × 1.33) or the `viewBox`, adds `xmlns`, `preserveAspectRatio` and
/// a missing `viewBox` to the root, drops comments when optimising and
/// writes the document again with REXML's default formatter (see
/// [`rexml`]).
fn svg(data: Vec<u8>, optimise: bool) -> Result<Processed, String> {
    let text = String::from_utf8(data).map_err(|_| "Invalid SVG encoding".to_owned())?;
    let mut nodes = rexml::parse(&text)?;
    let root = nodes
        .iter_mut()
        .find_map(|node| match node {
            rexml::Node::Start { attributes, .. } => Some(attributes),
            _ => None,
        })
        .ok_or_else(|| "Invalid SVG: no root element".to_owned())?;
    let get = |attributes: &Vec<(String, String)>, name: &str| {
        attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| unescape(value))
    };
    if get(root, "xmlns").is_none() {
        root.push(("xmlns".to_owned(), "http://www.w3.org/2000/svg".to_owned()));
    }
    if get(root, "preserveAspectRatio").is_none() {
        root.push(("preserveAspectRatio".to_owned(), "xMidYMid meet".to_owned()));
    }
    let mut width = None;
    let mut height = None;
    if let (Some(w), Some(h)) = (
        get(root, "width").as_deref().and_then(length),
        get(root, "height").as_deref().and_then(length),
    ) {
        width = Some(w);
        height = Some(h);
    }
    let viewbox = get(root, "viewBox");
    if width.is_none() && height.is_none() {
        if let Some([min_x, min_y, w, h]) = viewbox.as_deref().and_then(view_box) {
            width = Some(w.minus(min_x));
            height = Some(h.minus(min_y));
        }
    }
    if let (None, Some(w), Some(h)) = (&viewbox, width, height) {
        root.push((
            "viewBox".to_owned(),
            format!("0 0 {} {}", w.json(), h.json()),
        ));
    }
    if optimise {
        nodes.retain(|node| !matches!(node, rexml::Node::Comment(_)));
    }
    Ok(Processed {
        data: rexml::write(&nodes).into_bytes(),
        width,
        height,
    })
}

fn unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The subset of REXML's tree parser and `Formatters::Default` an SVG
/// round trip needs: the XML declaration and DOCTYPE written with REXML's
/// quotes, attributes sorted by local name in single quotes (`'` as
/// `&apos;`), empty elements self-closed, text, comments, CDATA and
/// instructions as read, and whitespace outside the root element kept only
/// before a DOCTYPE.
mod rexml {
    #[derive(Debug, Clone, PartialEq)]
    pub enum Node {
        Decl(Vec<(String, String)>),
        DocType(String),
        Instruction(String, Option<String>),
        Comment(String),
        CData(String),
        Text(String),
        Start {
            name: String,
            attributes: Vec<(String, String)>,
            empty: bool,
        },
        End(String),
    }

    fn pseudo_attributes(text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(eq) = rest.find('=') {
            let name = rest[..eq].trim().to_owned();
            let after = rest[eq + 1..].trim_start();
            let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                break;
            };
            let Some(close) = after[1..].find(quote) else {
                break;
            };
            out.push((name, after[1..1 + close].to_owned()));
            rest = &after[close + 2..];
        }
        out
    }

    fn attributes(text: &str) -> Result<Vec<(String, String)>, String> {
        let mut out = Vec::new();
        let mut rest = text.trim_start();
        while !rest.is_empty() {
            let eq = rest.find('=').ok_or("Invalid SVG attribute")?;
            let name = rest[..eq].trim().to_owned();
            let after = rest[eq + 1..].trim_start();
            let quote = after
                .chars()
                .next()
                .filter(|c| *c == '"' || *c == '\'')
                .ok_or("Invalid SVG attribute")?;
            let close = after[1..].find(quote).ok_or("Invalid SVG attribute")?;
            out.push((name, after[1..1 + close].to_owned()));
            rest = after[close + 2..].trim_start();
        }
        Ok(out)
    }

    fn tag_end(text: &str) -> Option<usize> {
        let mut quote = None;
        for (i, c) in text.char_indices() {
            match (quote, c) {
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), c) if c == q => quote = None,
                (None, '>') => return Some(i),
                _ => {}
            }
        }
        None
    }

    pub fn parse(text: &str) -> Result<Vec<Node>, String> {
        let invalid = || "Invalid SVG markup".to_owned();
        let mut nodes = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            if let Some(body) = rest.strip_prefix("<?") {
                let end = body.find("?>").ok_or_else(invalid)?;
                let inner = &body[..end];
                let (target, content) = match inner.find(char::is_whitespace) {
                    Some(at) => (&inner[..at], Some(inner[at..].trim_start().to_owned())),
                    None => (inner, None),
                };
                nodes.push(if target == "xml" {
                    Node::Decl(pseudo_attributes(content.as_deref().unwrap_or("")))
                } else {
                    Node::Instruction(target.to_owned(), content)
                });
                rest = &body[end + 2..];
            } else if let Some(body) = rest.strip_prefix("<!--") {
                let end = body.find("-->").ok_or_else(invalid)?;
                nodes.push(Node::Comment(body[..end].to_owned()));
                rest = &body[end + 3..];
            } else if let Some(body) = rest.strip_prefix("<![CDATA[") {
                let end = body.find("]]>").ok_or_else(invalid)?;
                nodes.push(Node::CData(body[..end].to_owned()));
                rest = &body[end + 3..];
            } else if let Some(body) = rest.strip_prefix("<!DOCTYPE") {
                let end = tag_end(body).ok_or_else(invalid)?;
                nodes.push(Node::DocType(body[..end].to_owned()));
                rest = &body[end + 1..];
            } else if let Some(body) = rest.strip_prefix("</") {
                let end = body.find('>').ok_or_else(invalid)?;
                nodes.push(Node::End(body[..end].trim().to_owned()));
                rest = &body[end + 1..];
            } else if let Some(body) = rest.strip_prefix('<') {
                let end = tag_end(body).ok_or_else(invalid)?;
                let mut inner = &body[..end];
                let empty = inner.ends_with('/');
                if empty {
                    inner = &inner[..inner.len() - 1];
                }
                let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
                nodes.push(Node::Start {
                    name: inner[..name_end].to_owned(),
                    attributes: attributes(&inner[name_end..])?,
                    empty,
                });
                rest = &body[end + 1..];
            } else {
                let end = rest.find('<').unwrap_or(rest.len());
                nodes.push(Node::Text(rest[..end].to_owned()));
                rest = &rest[end..];
            }
        }
        Ok(nodes)
    }

    /// `DocType#write` for `name [PUBLIC "p" "s" | SYSTEM "s"]`.
    fn doctype(text: &str) -> String {
        let mut words = Vec::new();
        let mut rest = text.trim();
        while !rest.is_empty() {
            if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let close = rest[1..].find(quote).map_or(rest.len(), |c| c + 1);
                words.push(rest[1..close.min(rest.len())].to_owned());
                rest = rest.get(close + 1..).unwrap_or("").trim_start();
            } else {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                words.push(rest[..end].to_owned());
                rest = rest[end..].trim_start();
            }
        }
        // `ReferenceWriter`: double quotes, except a system literal with
        // only double quotes inside.
        let quoted = |literal: &str, system: bool| {
            let quote = if system && !literal.contains('\'') && literal.contains('"') {
                '\''
            } else {
                '"'
            };
            format!(" {quote}{literal}{quote}")
        };
        let mut out = String::from("<!DOCTYPE");
        if let Some(name) = words.first() {
            out.push(' ');
            out.push_str(name);
        }
        match words.get(1).map(String::as_str) {
            Some("PUBLIC") => {
                out.push_str(" PUBLIC");
                if let Some(public) = words.get(2) {
                    out.push_str(&quoted(public, false));
                }
                if let Some(system) = words.get(3) {
                    out.push_str(&quoted(system, true));
                }
            }
            Some("SYSTEM") => {
                out.push_str(" SYSTEM");
                if let Some(system) = words.get(2) {
                    out.push_str(&quoted(system, true));
                }
            }
            _ => {}
        }
        out.push('>');
        out
    }

    fn local_name(name: &str) -> &str {
        name.rsplit(':').next().unwrap_or(name)
    }

    pub fn write(nodes: &[Node]) -> String {
        let mut out = String::new();
        let mut depth = 0usize;
        let mut root_done = false;
        let mut after_doctype = false;
        let mut i = 0;
        while i < nodes.len() {
            let top = depth == 0;
            match &nodes[i] {
                Node::Decl(fields) => {
                    out.push_str("<?xml");
                    for (name, value) in fields {
                        out.push_str(&format!(" {name}='{value}'"));
                    }
                    out.push_str("?>");
                }
                Node::DocType(text) => {
                    out.push_str(&doctype(text));
                    after_doctype = true;
                }
                Node::Instruction(target, content) => {
                    out.push_str("<?");
                    out.push_str(target);
                    if let Some(content) = content {
                        out.push(' ');
                        out.push_str(content);
                    }
                    out.push_str("?>");
                }
                Node::Comment(text) => {
                    out.push_str("<!--");
                    out.push_str(text);
                    out.push_str("-->");
                }
                Node::CData(text) => {
                    out.push_str("<![CDATA[");
                    out.push_str(text);
                    out.push_str("]]>");
                }
                Node::Text(text) => {
                    let keep = !top || (!root_done && !after_doctype);
                    if keep && (!top || text.trim().is_empty()) {
                        out.push_str(text);
                    }
                }
                Node::Start {
                    name,
                    attributes,
                    empty,
                } => {
                    out.push('<');
                    out.push_str(name);
                    let mut sorted: Vec<&(String, String)> = attributes.iter().collect();
                    sorted.sort_by(|a, b| local_name(&a.0).cmp(local_name(&b.0)));
                    for (key, value) in sorted {
                        out.push_str(&format!(" {key}='{}'", value.replace('\'', "&apos;")));
                    }
                    let closes =
                        !*empty && matches!(nodes.get(i + 1), Some(Node::End(end)) if end == name);
                    if *empty || closes {
                        out.push_str("/>");
                        if closes {
                            i += 1;
                        }
                        if depth == 0 {
                            root_done = true;
                        }
                    } else {
                        out.push('>');
                        depth += 1;
                    }
                }
                Node::End(name) => {
                    out.push_str("</");
                    out.push_str(name);
                    out.push('>');
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        root_done = true;
                    }
                }
            }
            i += 1;
        }
        out
    }
}

/// `WIDTH_HEIGHT_REGEX` and `to_px_factor`.
fn length(value: &str) -> Option<Number> {
    let value = value.trim();
    let digits = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(digits);
    let unit = unit.trim();
    if !valid_number(number) || !unit.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let factor = if unit == "pt" { 1.33 } else { 1.0 };
    Some(Number::parse(number)?.times(factor))
}

fn valid_number(text: &str) -> bool {
    let mut parts = text.splitn(2, '.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next();
    !int.is_empty()
        && int.chars().all(|c| c.is_ascii_digit())
        && frac.map_or(true, |f| {
            !f.is_empty() && f.chars().all(|c| c.is_ascii_digit())
        })
}

/// `VIEWBOX_REGEX` (whitespace-separated; the minimums may be negative).
fn view_box(value: &str) -> Option<[Number; 4]> {
    let fields: Vec<&str> = value.split_whitespace().collect();
    let [a, b, c, d] = fields.as_slice() else {
        return None;
    };
    let signed = |f: &str| {
        let digits = f.strip_prefix('-').unwrap_or(f);
        valid_number(digits).then(|| Number::parse(f)).flatten()
    };
    let plain = |f: &str| valid_number(f).then(|| Number::parse(f)).flatten();
    Some([signed(a)?, signed(b)?, plain(c)?, plain(d)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_size_comes_from_ihdr() {
        let mut data = vec![137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13];
        data.extend_from_slice(b"IHDR");
        data.extend_from_slice(&83u32.to_be_bytes());
        data.extend_from_slice(&155u32.to_be_bytes());
        let processed = post_process("png", data, true).unwrap();
        assert_eq!(processed.width, Some(Number::Int(83)));
        assert_eq!(processed.height, Some(Number::Int(155)));
        assert_eq!(
            post_process("png", b"nope".to_vec(), true).err().as_deref(),
            Some("Invalid PNG signature")
        );
    }

    fn svg_text(svg: &str, optimise: bool) -> (String, Option<Number>, Option<Number>) {
        let processed = post_process("svg", svg.as_bytes().to_vec(), optimise).unwrap();
        (
            String::from_utf8(processed.data).unwrap(),
            processed.width,
            processed.height,
        )
    }

    #[test]
    fn svg_size_and_root_attributes_follow_ruby() {
        // Expected texts are REXML 3.3.9's output for the same steps.
        let (text, width, height) = svg_text(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n\
             <!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\"\n \"http://x/svg11.dtd\">\n\
             <!-- c -->\n<svg width=\"50pt\" height=\"20pt\"\n xmlns:xlink=\"u\">\n<g>\n\
             <!-- n -->\n<e></e></g>\n</svg>\n",
            true,
        );
        assert_eq!(width, Some(Number::Float(50.0 * 1.33)));
        assert_eq!(width.unwrap().scaled(1.0), 66);
        assert_eq!(height.unwrap().scaled(1.0), 26);
        assert_eq!(
            text,
            "<?xml version='1.0' encoding='UTF-8' standalone='no'?>\n\
             <!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://x/svg11.dtd\">\
             <svg height='20pt' preserveAspectRatio='xMidYMid meet' viewBox='0 0 66.5 26.6' \
             width='50pt' xmlns:xlink='u' xmlns='http://www.w3.org/2000/svg'>\n<g>\n\n<e/></g>\n</svg>"
        );
        let (text, width, height) = svg_text(
            "<?plantuml 1.2?><svg b=\"2\" a=\"x&amp;y\" c=\"it's\" d='q\"' viewBox=\"-5 0 100 40\">\
             <t>a &amp; b &lt; c</t><![CDATA[x<y]]></svg>",
            false,
        );
        assert_eq!(
            (width, height),
            (Some(Number::Int(105)), Some(Number::Int(40)))
        );
        assert_eq!(
            text,
            "<?plantuml 1.2?><svg a='x&amp;y' b='2' c='it&apos;s' d='q\"' \
             preserveAspectRatio='xMidYMid meet' viewBox='-5 0 100 40' \
             xmlns='http://www.w3.org/2000/svg'><t>a &amp; b &lt; c</t><![CDATA[x<y]]></svg>"
        );
        // Whitespace before the root is kept without a DOCTYPE; comments
        // stay unless optimising.
        let (text, ..) = svg_text("<?xml version=\"1.0\"?>\n\n  <!-- c -->\n<svg/>\n", false);
        assert_eq!(
            text,
            "<?xml version='1.0'?>\n\n  <!-- c -->\n<svg preserveAspectRatio='xMidYMid meet' \
             xmlns='http://www.w3.org/2000/svg'/>"
        );
    }

    #[test]
    fn numbers_print_like_ruby() {
        assert_eq!(Number::Float(66.5).json(), "66.5");
        assert_eq!(Number::Float(67.0).json(), "67.0");
        assert_eq!(Number::Int(67).json(), "67");
        assert_eq!(Number::Float(66.99).scaled(2.0), 133);
    }
}
