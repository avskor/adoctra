//! Decode include bytes before registering their UTF-8 source snapshot.
//! Names and byte mappings follow the pinned Ruby reference. Unimplemented
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
    Japanese { table: &'static [u64], sjis: bool },
    Unavailable,
    Unsupported,
}

impl BinaryResource {
    /// Decode an include resource. Unknown encoding names fall back to UTF-8,
    /// as in Ruby. UTF-16 BOM detection applies to default/UTF-8 and BINARY;
    /// explicit other encodings take precedence. BOM and CR/LF are retained in
    /// the UTF-8 snapshot for the parser's existing normalization and spans.
    /// Japanese Shift_JIS/EUC-JP families use the reference's byte mappings.
    /// Other known legacy encodings unsupported here return `Unsupported`;
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
            Encoding::Japanese { table, sjis } => {
                let mut text = String::with_capacity(bytes.len());
                let mut remaining = bytes.as_slice();
                while let Some(&first) = remaining.first() {
                    if first < 128 {
                        text.push(char::from(first));
                        remaining = &remaining[1..];
                        continue;
                    }
                    let width = if sjis {
                        match first {
                            0xa1..=0xdf => 1,
                            0x81..=0x9f | 0xe0..=0xfc => 2,
                            _ => return Err(invalid()),
                        }
                    } else {
                        match first {
                            0x8e | 0xa1..=0xfe => 2,
                            0x8f => 3,
                            _ => return Err(invalid()),
                        }
                    };
                    let sequence = remaining.get(..width).ok_or_else(invalid)?;
                    let key = sequence
                        .iter()
                        .fold(0_u64, |key, byte| (key << 8) | u64::from(*byte));
                    let index = table
                        .binary_search_by_key(&key, |entry| entry >> 32)
                        .map_err(|_| invalid())?;
                    text.push(char::from_u32(table[index] as u32).ok_or_else(invalid)?);
                    remaining = &remaining[width..];
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
            decode(b"ASCII", Some("GB18030")).unwrap_err().kind,
            ResourceErrorKind::Unsupported
        );
        assert_eq!(
            decode(b"ASCII", Some("Windows-1258")).unwrap_err().kind,
            ResourceErrorKind::InvalidEncoding
        );
    }

    #[test]
    fn japanese_codecs_preserve_reference_variants_and_aliases() {
        for (label, bytes, expected) in [
            (
                "Shift_JIS",
                &b"\x81\x5c\x81\x60\x81\x61\x81\x7c\x81\x91\x81\x92\x81\xca"[..],
                "—〜‖−¢£¬",
            ),
            (
                "sHiFt_JiS",
                &b"\x93\xfa\x96\x7b\x8c\xea \xb1\\~\r\n"[..],
                "日本語 ｱ\\~\r\n",
            ),
            (
                "EUC-JP",
                &b"\xa1\xc1\xa1\xc2\xa1\xdd\xa1\xf1\xa1\xf2\xa2\xcc\x8f\xa2\xaf"[..],
                "〜‖−¢£¬˘",
            ),
            (
                "eucJP-ms",
                &b"\xa1\xc1\xa1\xc2\xa1\xdd\xa1\xf1\xa1\xf2\xa2\xcc\x8f\xf3\xf3"[..],
                "～∥－￠￡￢ⅰ",
            ),
            (
                "CP51932",
                &b"\xa1\xc1\xa1\xc2\xa1\xdd\xa1\xf1\xa1\xf2\xa2\xcc"[..],
                "～∥－￠￡￢",
            ),
        ] {
            assert_eq!(
                decode(bytes, Some(label)).unwrap().text,
                expected,
                "{label}"
            );
        }
        // Ruby's SJIS alias means Windows-31J, not strict Shift_JIS.
        for name in ["Windows-31J", "CP932", "csWindows31J", "PCK", "SJIS"] {
            assert_eq!(
                decode(b"\x81\x5c\x81\x60\xfa\x40\xf0\x40", Some(name))
                    .unwrap()
                    .text,
                "―～ⅰ\u{e000}",
                "{name}"
            );
        }
        for name in ["eucJP", "EUC-JP-MS", "eucJP-ms", "CP51932"] {
            assert_eq!(
                decode(b"\xc6\xfc\xcb\xdc\xb8\xec \x8e\xb1\\~\r\n", Some(name))
                    .unwrap()
                    .text,
                "日本語 ｱ\\~\r\n",
                "{name}"
            );
        }
    }

    #[test]
    fn japanese_decoding_matches_exhaustive_ruby_fingerprints() {
        // Ruby 3.3.8, pinned Asciidoctor environment. Regenerate independently:
        // ruby scripts/generate_encodings.rb --fingerprints
        // All single bytes and pairs, plus every 8f-prefixed triple for EUC.
        for (name, expected, euc) in [
            ("Shift_JIS", 0xfcf4d82721c2f680_u64, false),
            ("Windows-31J", 0x800435c6fe305fcd, false),
            ("EUC-JP", 0x588386992351e229, true),
            ("eucJP-ms", 0x2d57700417373809, true),
            ("CP51932", 0x8a0af9cc4847e1c0, true),
        ] {
            let mut hash = 0xcbf29ce484222325_u64;
            let mut feed = |byte| {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            };
            let mut check = |bytes: &[u8]| match decode(bytes, Some(name)) {
                Ok(resource) => {
                    feed(1_u8);
                    for byte in resource.text.bytes() {
                        feed(byte);
                    }
                    feed(255);
                }
                Err(error) => {
                    assert_eq!(error.kind, ResourceErrorKind::InvalidEncoding);
                    feed(0);
                }
            };
            check(&[]);
            for a in 0..=255 {
                check(&[a]);
            }
            for a in 0..=255 {
                for b in 0..=255 {
                    check(&[a, b]);
                }
            }
            if euc {
                for a in 0..=255 {
                    for b in 0..=255 {
                        check(&[0x8f, a, b]);
                    }
                }
            }
            assert_eq!(hash, expected, "{name}");
        }
    }

    #[test]
    fn japanese_codecs_reject_incomplete_invalid_and_unmapped_sequences() {
        for (name, bad) in [
            ("Shift_JIS", &b"\x81"[..]),
            ("Shift_JIS", &b"\x81\x7f"[..]),
            ("Shift_JIS", &b"\xfa\x40"[..]),
            ("Windows-31J", &b"\xfc\xfc"[..]),
            ("Windows-31J", &b"\x80"[..]),
            ("EUC-JP", &b"\x8e"[..]),
            ("EUC-JP", &b"\x8e\xe0"[..]),
            ("EUC-JP", &b"\x8f\xa2"[..]),
            ("EUC-JP", &b"\x8f\xf3\xf3"[..]),
            ("eucJP-ms", &b"\x8f\xa0\xa1"[..]),
            ("CP51932", &b"\x8f\xa2\xaf"[..]),
        ] {
            for prefix in [&b""[..], &b"good\r\n"[..]] {
                let bytes = [prefix, bad].concat();
                let error = decode(&bytes, Some(name)).unwrap_err();
                assert_eq!(
                    error.kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}: {bad:?}"
                );
                assert_eq!(error.target, "/part.adoc");
            }
        }
    }
}
