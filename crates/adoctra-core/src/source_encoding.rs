//! Decode include bytes before registering their UTF-8 source snapshot.
//! Names and single-byte mappings follow the pinned Ruby reference. Multibyte
//! legacy encodings require a host decoder; never mistake a known unsupported
//! encoding for an unknown name (which Ruby ignores).
use crate::{BinaryResource, Resource, ResourceError, ResourceErrorKind, ResourceResult};

#[path = "source_encoding_tables.rs"]
mod tables;

enum Encoding {
    Utf8,
    Ascii,
    Binary,
    Utf16(Option<bool>),
    Utf32(Option<bool>),
    Single(&'static [u32]),
    Unavailable,
    Unsupported,
}

impl BinaryResource {
    /// Decode an include resource. Unknown encoding names fall back to UTF-8,
    /// as in Ruby. UTF-16 BOM detection applies to default/UTF-8 and BINARY;
    /// explicit other encodings take precedence. BOM and CR/LF are retained in
    /// the UTF-8 snapshot for the parser's existing normalization and spans.
    /// Known legacy multibyte encodings unsupported here return `Unsupported`;
    /// a host may implement them in `ResourceProvider::load_with_encoding`.
    pub fn decode(self, encoding: Option<&str>) -> ResourceResult<Resource> {
        let name = encoding.unwrap_or_default().to_ascii_lowercase();
        let mut encoding = tables::lookup(&name).unwrap_or(Encoding::Utf8);
        let bytes = self.bytes;
        let invalid = || {
            ResourceError::new(
                ResourceErrorKind::InvalidEncoding,
                &self.name,
                format!(
                    "invalid source encoding: {}",
                    if name.is_empty() { "UTF-8" } else { &name }
                ),
            )
        };
        // Ruby's Helpers.prepare_source_string also detects UTF-16 when the
        // file was opened with the default UTF-8 external encoding.
        if matches!(encoding, Encoding::Utf8 | Encoding::Binary) {
            if bytes.starts_with(&[0xff, 0xfe]) {
                encoding = Encoding::Utf16(Some(true));
            } else if bytes.starts_with(&[0xfe, 0xff]) {
                encoding = Encoding::Utf16(Some(false));
            }
        }
        let text = match encoding {
            Encoding::Utf8 => String::from_utf8(bytes).map_err(|_| invalid())?,
            Encoding::Binary => {
                // Ruby retains ASCII-8BIT until prepare_source_string: a BOM
                // is recognized there, then only ASCII bytes convert to UTF-8.
                if !bytes
                    .strip_prefix(&[0xef, 0xbb, 0xbf])
                    .unwrap_or(&bytes)
                    .is_ascii()
                {
                    return Err(invalid());
                }
                String::from_utf8(bytes).map_err(|_| invalid())?
            }
            Encoding::Ascii => {
                if !bytes.is_ascii() {
                    return Err(invalid());
                }
                String::from_utf8(bytes).map_err(|_| invalid())?
            }
            Encoding::Single(table) => {
                let mut text = String::with_capacity(bytes.len());
                for byte in bytes {
                    let value = if table.len() == 128 && byte < 128 {
                        u32::from(byte)
                    } else {
                        table[usize::from(byte) - (256 - table.len())]
                    };
                    text.push(char::from_u32(value).ok_or_else(invalid)?);
                }
                text
            }
            Encoding::Utf16(endian) => {
                if bytes.len() % 2 != 0 {
                    return Err(invalid());
                }
                let little = endian
                    .or_else(|| match bytes.get(..2) {
                        Some([0xff, 0xfe]) => Some(true),
                        Some([0xfe, 0xff]) => Some(false),
                        _ if bytes.is_empty() => Some(false),
                        _ => None,
                    })
                    .ok_or_else(invalid)?;
                let units = bytes.chunks_exact(2).map(|b| {
                    if little {
                        u16::from_le_bytes([b[0], b[1]])
                    } else {
                        u16::from_be_bytes([b[0], b[1]])
                    }
                });
                char::decode_utf16(units)
                    .collect::<Result<String, _>>()
                    .map_err(|_| invalid())?
            }
            Encoding::Utf32(endian) => {
                if bytes.len() % 4 != 0 {
                    return Err(invalid());
                }
                let little = endian
                    .or_else(|| match bytes.get(..4) {
                        Some([0xff, 0xfe, 0, 0]) => Some(true),
                        Some([0, 0, 0xfe, 0xff]) => Some(false),
                        _ if bytes.is_empty() => Some(false),
                        _ => None,
                    })
                    .ok_or_else(invalid)?;
                let mut text = String::with_capacity(bytes.len());
                for b in bytes.chunks_exact(4) {
                    let word = [b[0], b[1], b[2], b[3]];
                    let value = if little {
                        u32::from_le_bytes(word)
                    } else {
                        u32::from_be_bytes(word)
                    };
                    text.push(char::from_u32(value).ok_or_else(invalid)?);
                }
                text
            }
            Encoding::Unavailable => return Err(invalid()),
            Encoding::Unsupported => {
                return Err(ResourceError::new(
                    ResourceErrorKind::Unsupported,
                    &self.name,
                    format!("source encoding requires a host decoder: {name}"),
                ))
            }
        };
        Ok(Resource {
            name: self.name,
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8], encoding: Option<&str>) -> ResourceResult<Resource> {
        BinaryResource {
            name: "/part.adoc".into(),
            bytes: bytes.to_vec(),
            content_type: None,
        }
        .decode(encoding)
    }

    #[test]
    fn include_decoding_handles_unicode_bom_and_strict_errors() {
        let text = "\u{feff}Ж🙂\r\n";
        for little in [false, true] {
            let utf16: Vec<_> = text
                .encode_utf16()
                .flat_map(|v| {
                    if little {
                        v.to_le_bytes()
                    } else {
                        v.to_be_bytes()
                    }
                })
                .collect();
            let utf32: Vec<_> = text
                .chars()
                .flat_map(|v| {
                    if little {
                        (v as u32).to_le_bytes()
                    } else {
                        (v as u32).to_be_bytes()
                    }
                })
                .collect();
            for label in [
                None,
                Some("UTF-8"),
                Some("UTF-16"),
                Some("BINARY"),
                Some(if little { "UTF-16LE" } else { "UCS-2BE" }),
            ] {
                assert_eq!(decode(&utf16, label).unwrap().text, text);
            }
            for label in ["UTF-32", if little { "UCS-4LE" } else { "UTF-32BE" }] {
                assert_eq!(decode(&utf32, Some(label)).unwrap().text, text);
            }
        }
        for label in [
            None,
            Some("UTF-8"),
            Some("CP65001"),
            Some("iso-1000-1"),
            Some(""),
        ] {
            assert_eq!(decode(text.as_bytes(), label).unwrap().text, text);
        }
        for (bytes, label) in [
            (&b"\xff"[..], "UTF-8"),
            (&b"\xff"[..], "ASCII"),
            (&b"\xff"[..], "BINARY"),
            (&b"\x00\xd8"[..], "UTF-16LE"),
            (&b"\x00"[..], "UTF-16BE"),
            (&b"\x41\x00"[..], "UTF-16"),
            (&b"\x00\x00\x11\x00"[..], "UTF-32LE"),
            (&b"\x00\xd8\x00\x00"[..], "UTF-32LE"),
            (&b"\x00"[..], "UTF-32BE"),
            (&b"\x00\x00\x00A"[..], "UTF-32"),
        ] {
            let error = decode(bytes, Some(label)).unwrap_err();
            assert_eq!(error.kind, ResourceErrorKind::InvalidEncoding, "{label}");
            assert_eq!(error.target, "/part.adoc");
        }
        assert_eq!(decode(b"", Some("UTF-16")).unwrap().text, "");
        assert_eq!(decode(b"", Some("UTF-32")).unwrap().text, "");
        assert_eq!(
            decode(b"\xef\xbb\xbfplain", Some("BINARY")).unwrap().text,
            "\u{feff}plain"
        );
        assert_eq!(
            decode(b"\xef\xbb\xbf\xd0\x96", Some("BINARY"))
                .unwrap_err()
                .kind,
            ResourceErrorKind::InvalidEncoding
        );
    }

    #[test]
    fn include_decoding_keeps_ruby_single_byte_semantics_and_aliases() {
        for (label, bytes, expected) in [
            (
                "iso8859-1",
                &b"O\xf9 est l'h\xf4pital ?\r\n"[..],
                "Où est l'hôpital ?\r\n",
            ),
            ("ISO-8859-1", &b"\x80\x81"[..], "\u{80}\u{81}"),
            ("cp1251", &b"\xcf\xf0\xe8\xe2\xe5\xf2"[..], "Привет"),
            ("Windows-1252", &b"\x80"[..], "€"),
            ("CP878", &b"\xf0\xd2\xc9\xd7\xc5\xd4"[..], "Привет"),
            ("ebcdic-cp-us", &b"\xc1\xc2\xc3"[..], "ABC"),
            ("ISO-8859-1", &b"\xef\xbb\xbfhi"[..], "ï»¿hi"),
        ] {
            assert_eq!(
                decode(bytes, Some(label)).unwrap().text,
                expected,
                "{label}"
            );
        }
        assert_eq!(
            decode(b"\x81", Some("CP1252")).unwrap_err().kind,
            ResourceErrorKind::InvalidEncoding
        );
        // These are recognized names, not misspellings that may fall back to UTF-8.
        assert_eq!(
            decode(b"ASCII", Some("Shift_JIS")).unwrap_err().kind,
            ResourceErrorKind::Unsupported
        );
        assert_eq!(
            decode(b"ASCII", Some("Windows-1258")).unwrap_err().kind,
            ResourceErrorKind::InvalidEncoding
        );
    }
}
