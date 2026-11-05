//! `HttpConverter`: `diagram-server-url` with `diagram-server-type`
//! `plantuml` (a PlantUML server) or `kroki_io` (Kroki). The code is
//! compressed into the GET path, or POSTed when the path is longer than
//! `max-get-size` (1024) and the host is not plantuml.com.

use super::{Converter, OptValue, Options};
use crate::host::{DiagramHost, HttpRequest};
use crate::source::Source;
use std::io::Write;

pub(crate) struct Http<'c> {
    pub base_uri: String,
    pub server_type: String,
    pub converter: &'c dyn Converter,
}

const DEFAULT_MAX_GET_SIZE: &str = "1024";

impl Http<'_> {
    pub fn collect_options(source: &Source<'_, '_>) -> Options {
        let size = source
            .global_attr("max-get-size")
            .unwrap_or_else(|| DEFAULT_MAX_GET_SIZE.to_owned());
        vec![("max_get_size", OptValue::Int(ruby_to_i(&size)))]
    }

    pub fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let code = self.converter.server_code(source, host)?;
        let uri = Uri::parse(&self.base_uri)?;
        let mut path = uri.path.clone();
        if !path.ends_with('/') {
            path.push('/');
        }
        let data = match self.server_type.as_str() {
            "plantuml" => {
                path.push_str(format);
                plantuml_encode(&raw_deflate(code.as_bytes()))
            }
            "kroki_io" => {
                path.push_str(&source.diagram_type);
                path.push('/');
                path.push_str(format);
                urlsafe_base64(&zlib_deflate(code.as_bytes()))
            }
            other => return Err(format!("Unsupported server type: {other}")),
        };
        let get_path = format!("{path}/{data}");
        let max: usize = options
            .iter()
            .find_map(|(key, value)| match (key, value) {
                (&"max_get_size", OptValue::Int(size)) => usize::try_from(*size).ok(),
                _ => None,
            })
            .unwrap_or(1024);
        let plantuml_com = uri.host.to_lowercase().ends_with("plantuml.com");
        let request = if !plantuml_com && get_path.len() > max {
            HttpRequest {
                url: uri.with_path(&path),
                body: Some(code.into_bytes()),
                content_type: Some("text/plain; charset=utf-8".to_owned()),
            }
        } else {
            HttpRequest {
                url: uri.with_path(&get_path),
                body: None,
                content_type: None,
            }
        };
        let client = host
            .http()
            .ok_or_else(|| "HTTP access to diagram servers is not allowed".to_owned())?;
        client.send(&request)
    }
}

/// Ruby's `String#to_i`: the leading integer, else 0.
fn ruby_to_i(text: &str) -> i64 {
    let text = text.trim_start();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    sign * digits[..end].parse::<i64>().unwrap_or(0)
}

struct Uri {
    scheme: String,
    host: String,
    port: Option<String>,
    path: String,
}

impl Uri {
    fn parse(uri: &str) -> Result<Self, String> {
        let (scheme, rest) = uri
            .split_once("://")
            .ok_or_else(|| format!("bad URI(is not URI?): {uri:?}"))?;
        let (authority, path) = match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, ""),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
                (host, Some(port.to_owned()))
            }
            _ => (authority, None),
        };
        let path = path.split(['?', '#']).next().unwrap_or("").to_owned();
        Ok(Self {
            scheme: scheme.to_owned(),
            host: host.to_owned(),
            port,
            path,
        })
    }

    fn with_path(&self, path: &str) -> String {
        match &self.port {
            Some(port) => format!("{}://{}:{port}{path}", self.scheme, self.host),
            None => format!("{}://{}{path}", self.scheme, self.host),
        }
    }
}

fn raw_deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

fn zlib_deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `[bin].pack("m0")`: standard Base64 with padding.
fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(STANDARD[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// asciidoctor-diagram's `Base64.urlsafe_encode` (padding kept).
fn urlsafe_base64(data: &[u8]) -> String {
    base64(data).replace('+', "-").replace('/', "_")
}

/// The PlantUML text encoding: URL-safe Base64 translated to PlantUML's
/// alphabet (`0-9A-Za-z-_`).
fn plantuml_encode(data: &[u8]) -> String {
    const FROM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    const TO: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    urlsafe_base64(data)
        .bytes()
        .map(|b| match FROM.iter().position(|&f| f == b) {
            Some(i) => TO[i] as char,
            None => b as char,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn base64_matches_ruby_pack() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(urlsafe_base64(&[0xfb, 0xff]), "-_8=");
        // Ruby: "Zm9v".tr("A-Za-z0-9", "0-9A-Za-z")
        assert_eq!(plantuml_encode(b"foo"), "Pczl");
    }

    #[test]
    fn compressed_payloads_round_trip() {
        let code = b"digraph { a -> b }";
        let mut inflated = Vec::new();
        flate2::read::ZlibDecoder::new(&zlib_deflate(code)[..])
            .read_to_end(&mut inflated)
            .unwrap();
        assert_eq!(inflated, code);
        inflated.clear();
        flate2::read::DeflateDecoder::new(&raw_deflate(code)[..])
            .read_to_end(&mut inflated)
            .unwrap();
        assert_eq!(inflated, code);
    }

    #[test]
    fn uris_split_like_ruby() {
        let uri = Uri::parse("https://kroki.io:8000/base?x=1").unwrap();
        assert_eq!(uri.host, "kroki.io");
        assert_eq!(uri.path, "/base");
        assert_eq!(uri.with_path("/a/b"), "https://kroki.io:8000/a/b");
        assert_eq!(ruby_to_i(" 12ab"), 12);
        assert_eq!(ruby_to_i("x"), 0);
    }
}
