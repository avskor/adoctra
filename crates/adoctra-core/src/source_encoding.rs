//! Decode include bytes before registering their UTF-8 source snapshot.
//! Names and byte mappings follow the pinned Ruby reference. Unimplemented
//! legacy encodings require a host decoder; never mistake a known unsupported
//! encoding for an unknown name (which Ruby ignores).
use crate::{
    BinaryResource, IncludeInput, IncludeResource, Resource, ResourceError, ResourceErrorKind,
    ResourceResult,
};
use std::borrow::Cow;

#[path = "source_encoding_tables.rs"]
mod tables;

enum Encoding {
    Utf8,
    Ascii,
    Binary,
    Utf16(Option<bool>),
    Utf32(Option<bool>),
    Single(&'static [u32]),
    Iso2022Jp(&'static [u64]),
    Multibyte {
        table: &'static [u64],
        kind: MultibyteKind,
    },
    Unavailable,
    Unsupported,
}

enum MultibyteKind {
    ShiftJis,
    EucJp,
    Big5,
    Gb2312,
    Korean,
    Gbk,
    Gb18030,
}

impl BinaryResource {
    /// Defer malformed UTF-8/BINARY to reader selection, without decoding or
    /// replacing its bytes. Other codecs retain strict consumed-prefix checks.
    pub fn prepare_include(
        self,
        encoding: Option<&str>,
        line_limit: Option<usize>,
    ) -> ResourceResult<IncludeInput> {
        let name = encoding.unwrap_or_default().to_ascii_lowercase();
        let kind = tables::lookup(&name).unwrap_or(Encoding::Utf8);
        if matches!(kind, Encoding::Utf8 | Encoding::Binary)
            && !self.bytes.starts_with(&[0xff, 0xfe])
            && !self.bytes.starts_with(&[0xfe, 0xff])
            && self.decode_text(encoding).is_err()
        {
            return Ok(IncludeInput::Deferred {
                resource: self,
                binary: matches!(kind, Encoding::Binary),
            });
        }
        self.decode_include(encoding, line_limit)
            .map(IncludeInput::Decoded)
    }

    /// Decode an include resource. Unknown encoding names fall back to UTF-8,
    /// as in Ruby. UTF-16 BOM detection applies to default/UTF-8 and BINARY;
    /// explicit other encodings take precedence. BOM and CR/LF are retained in
    /// the UTF-8 snapshot for the parser's existing normalization and spans.
    /// Japanese (including ISO-2022-JP), Big5, GB and Korean families use the
    /// reference's byte mappings and state transitions.
    /// Other known legacy encodings unsupported here return `Unsupported`;
    /// a host may implement them in `ResourceProvider::load_with_encoding`.
    pub fn decode(self, encoding: Option<&str>) -> ResourceResult<Resource> {
        self.decode_include(encoding, None)
            .map(|decoded| decoded.resource)
    }

    /// Prefer the complete decoded snapshot. If its tail is invalid and a
    /// finite line selection can stop before it, retain only the intact prefix
    /// and mark the snapshot incomplete. Bytes before the boundary are always
    /// validated, including lines the reader will discard. No lossy decoding
    /// is used. This method does not bound the provider's filesystem/network I/O.
    pub fn decode_include(
        mut self,
        encoding: Option<&str>,
        line_limit: Option<usize>,
    ) -> ResourceResult<IncludeResource> {
        let mut complete = true;
        let text = loop {
            match self.decode_text(encoding) {
                Ok(Cow::Borrowed(_)) => {
                    break String::from_utf8(self.bytes).expect("validated UTF-8")
                }
                Ok(Cow::Owned(text)) => break text,
                Err(error) => {
                    let end = if complete && error.kind == ResourceErrorKind::InvalidEncoding {
                        line_limit.and_then(|limit| self.prefix_end(encoding, limit))
                    } else {
                        None
                    };
                    let Some(end) = end.filter(|&end| end < self.bytes.len()) else {
                        return Err(error);
                    };
                    self.bytes.truncate(end);
                    complete = false;
                }
            }
        };
        Ok(IncludeResource {
            resource: Resource {
                name: self.name,
                text,
            },
            complete,
        })
    }

    /// Locate the last consumed LF in the external encoding. A malformed
    /// sequence before this boundary still fails when decoding the prefix.
    fn prefix_end(&self, encoding: Option<&str>, mut lines: usize) -> Option<usize> {
        if lines == 0 {
            return None;
        }
        let name = encoding.unwrap_or_default().to_ascii_lowercase();
        let encoding = tables::lookup(&name).unwrap_or(Encoding::Utf8);
        let mut lf = [0; 4];
        let width = match encoding {
            Encoding::Utf16(endian) => {
                let little = endian.or_else(|| match self.bytes.get(..2) {
                    Some([0xff, 0xfe]) => Some(true),
                    Some([0xfe, 0xff]) => Some(false),
                    _ => None,
                })?;
                lf[..2].copy_from_slice(&if little {
                    10_u16.to_le_bytes()
                } else {
                    10_u16.to_be_bytes()
                });
                2
            }
            Encoding::Utf32(endian) => {
                let little = endian.or_else(|| match self.bytes.get(..4) {
                    Some([0xff, 0xfe, 0, 0]) => Some(true),
                    Some([0, 0, 0xfe, 0xff]) => Some(false),
                    _ => None,
                })?;
                lf = if little {
                    10_u32.to_le_bytes()
                } else {
                    10_u32.to_be_bytes()
                };
                4
            }
            Encoding::Single(table) if table.len() == 256 => {
                lf[0] = table.iter().position(|&value| value == 10)? as u8;
                1
            }
            Encoding::Unsupported | Encoding::Unavailable => return None,
            // Ruby reads default/BINARY UTF-16 BOM resources as byte lines
            // before normalization. Its selection can fail outside the include
            // error handler; do not manufacture a successful prefix for it.
            Encoding::Utf8 | Encoding::Binary
                if self.bytes.starts_with(&[0xff, 0xfe])
                    || self.bytes.starts_with(&[0xfe, 0xff]) =>
            {
                return None
            }
            _ => {
                lf[0] = b'\n';
                1
            }
        };
        for (index, unit) in self.bytes.chunks_exact(width).enumerate() {
            if unit == &lf[..width] {
                lines -= 1;
                if lines == 0 {
                    return Some((index + 1) * width);
                }
            }
        }
        None
    }

    fn decode_text(&self, encoding: Option<&str>) -> ResourceResult<Cow<'_, str>> {
        let name = encoding.unwrap_or_default().to_ascii_lowercase();
        let mut encoding = tables::lookup(&name).unwrap_or(Encoding::Utf8);
        let bytes = &self.bytes;
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
            Encoding::Utf8 => {
                return std::str::from_utf8(bytes)
                    .map(Cow::Borrowed)
                    .map_err(|_| invalid())
            }
            Encoding::Binary => {
                // Ruby retains ASCII-8BIT until prepare_source_string: a BOM
                // is recognized there, then only ASCII bytes convert to UTF-8.
                if !bytes
                    .strip_prefix(&[0xef, 0xbb, 0xbf])
                    .unwrap_or(bytes)
                    .is_ascii()
                {
                    return Err(invalid());
                }
                return std::str::from_utf8(bytes)
                    .map(Cow::Borrowed)
                    .map_err(|_| invalid());
            }
            Encoding::Ascii => {
                if !bytes.is_ascii() {
                    return Err(invalid());
                }
                return std::str::from_utf8(bytes)
                    .map(Cow::Borrowed)
                    .map_err(|_| invalid());
            }
            Encoding::Single(table) => {
                let mut text = String::with_capacity(bytes.len());
                for &byte in bytes {
                    let value = if table.len() == 128 && byte < 128 {
                        u32::from(byte)
                    } else {
                        table[usize::from(byte) - (256 - table.len())]
                    };
                    text.push(char::from_u32(value).ok_or_else(invalid)?);
                }
                text
            }
            Encoding::Iso2022Jp(table) => {
                let mut text = String::with_capacity(bytes.len());
                let mut remaining = bytes.as_slice();
                let mut jis = false;
                while let Some(&first) = remaining.first() {
                    if first == 0x1b {
                        // Ruby accepts JIS X 0208-1978/1983 and treats Roman
                        // like ASCII (including backslash and tilde). It does
                        // not accept katakana, JIS X 0212 or revision escapes.
                        jis = match remaining.get(..3) {
                            Some([0x1b, b'$', b'@' | b'B']) => true,
                            Some([0x1b, b'(', b'B' | b'J']) => false,
                            _ => return Err(invalid()),
                        };
                        remaining = &remaining[3..];
                    } else if jis {
                        let sequence = remaining.get(..2).ok_or_else(invalid)?;
                        if !sequence.iter().all(|byte| (0x21..=0x7e).contains(byte)) {
                            return Err(invalid());
                        }
                        // All 6,879 mappings equal the corresponding EUC-JP
                        // pair. Reuse its table without vendor substitutions.
                        let key = (u64::from(first + 0x80) << 8) | u64::from(sequence[1] + 0x80);
                        let index = table
                            .binary_search_by_key(&key, |entry| entry >> 32)
                            .map_err(|_| invalid())?;
                        text.push(char::from_u32(table[index] as u32).ok_or_else(invalid)?);
                        remaining = &remaining[2..];
                    } else {
                        if first >= 128 || matches!(first, 0x0e | 0x0f) {
                            return Err(invalid());
                        }
                        text.push(char::from(first));
                        remaining = &remaining[1..];
                    }
                }
                text
            }
            Encoding::Multibyte { table, kind } => {
                let mut text = String::with_capacity(bytes.len());
                let mut remaining = bytes.as_slice();
                while let Some(&first) = remaining.first() {
                    if first < 128 {
                        text.push(char::from(first));
                        remaining = &remaining[1..];
                        continue;
                    }
                    let width = match kind {
                        MultibyteKind::ShiftJis => match first {
                            0xa1..=0xdf => 1,
                            0x81..=0x9f | 0xe0..=0xfc => 2,
                            _ => return Err(invalid()),
                        },
                        MultibyteKind::EucJp => match first {
                            0x8e | 0xa1..=0xfe => 2,
                            0x8f => 3,
                            _ => return Err(invalid()),
                        },
                        MultibyteKind::Big5 => match first {
                            0x81..=0xfe => 2,
                            _ => return Err(invalid()),
                        },
                        MultibyteKind::Korean => match first {
                            0x81..=0xfe => 2,
                            _ => return Err(invalid()),
                        },
                        MultibyteKind::Gb2312 => match first {
                            0xa1..=0xfe => 2,
                            _ => return Err(invalid()),
                        },
                        MultibyteKind::Gbk => match first {
                            0x80 => 1,
                            0x81..=0xfe => 2,
                            _ => return Err(invalid()),
                        },
                        MultibyteKind::Gb18030 => match first {
                            0x81..=0xfe if remaining.get(1).is_some_and(u8::is_ascii_digit) => 4,
                            0x81..=0xfe => 2,
                            _ => return Err(invalid()),
                        },
                    };
                    let sequence = remaining.get(..width).ok_or_else(invalid)?;
                    if width == 4 {
                        if !(0x81..=0xfe).contains(&sequence[2]) || !sequence[3].is_ascii_digit() {
                            return Err(invalid());
                        }
                        let pointer = (((u32::from(first - 0x81) * 10
                            + u32::from(sequence[1] - b'0'))
                            * 126
                            + u32::from(sequence[2] - 0x81))
                            * 10)
                            + u32::from(sequence[3] - b'0');
                        let index = tables::GB18030_RANGES
                            .partition_point(|&(start, _, _)| start <= pointer);
                        let &(start, end, scalar) = index
                            .checked_sub(1)
                            .and_then(|index| tables::GB18030_RANGES.get(index))
                            .filter(|&&(_, end, _)| pointer <= end)
                            .ok_or_else(invalid)?;
                        debug_assert!(pointer <= end);
                        text.push(char::from_u32(scalar + pointer - start).ok_or_else(invalid)?);
                        remaining = &remaining[4..];
                        continue;
                    }
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
        Ok(Cow::Owned(text))
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
    fn finite_include_reads_recover_only_intact_prefixes_and_keep_valid_full_sources() {
        for (encoding, good, bad) in [
            (None, &b"\xef\xbb\xbf\xd0\x96\r\n"[..], &b"\xff"[..]),
            (Some("unknown-codec"), &b"good\n"[..], &b"\xff"[..]),
            (Some("ASCII"), &b"good\n"[..], &b"\x80"[..]),
            (Some("BINARY"), &b"good\n"[..], &b"\x80"[..]),
            (Some("CP1252"), &b"caf\xe9\r\n"[..], &b"\x81"[..]),
            (Some("Shift_JIS"), &b"\x95\x5c\r\n"[..], &b"\x81"[..]),
            (Some("EUC-JP"), &b"\x8f\xa2\xaf\r\n"[..], &b"\x8f\xa2"[..]),
            (
                Some("ISO-2022-JP"),
                &b"\x1b$B$\"\x1b(B\r\n"[..],
                &b"\x1b$"[..],
            ),
            (Some("UTF-16LE"), &b"\x00\x0a\x0a\x00"[..], &b"\x00\xd8"[..]),
            (Some("UTF-16BE"), &b"\x0a\x00\x00\x0a"[..], &b"\xd8"[..]),
            (
                Some("UTF-16"),
                &b"\xff\xfe\x16\x04\x0a\x00"[..],
                &b"\x00\xd8"[..],
            ),
            (
                Some("UTF-32LE"),
                &b"\x0a\x00\x01\x00\x0a\x00\x00\x00"[..],
                &b"\x00\x00\x11\x00"[..],
            ),
            (
                Some("UTF-32BE"),
                &b"\x00\x01\x00\x0a\x00\x00\x00\x0a"[..],
                &b"\x00"[..],
            ),
            (
                Some("UTF-32"),
                &b"\x00\x00\xfe\xff\x00\x00\x00\x0a"[..],
                &b"\x00"[..],
            ),
        ] {
            let resource = |bytes: Vec<u8>| BinaryResource {
                name: "/part.adoc".into(),
                bytes,
                content_type: None,
            };
            let prefix = decode(good, encoding).unwrap().text;
            let broken = [good, bad].concat();
            let result = resource(broken.clone())
                .decode_include(encoding, Some(1))
                .unwrap();
            assert!(!result.complete, "{encoding:?}");
            assert_eq!(result.resource.text, prefix);
            assert_eq!(result.resource.name, "/part.adoc");
            for bound in [None, Some(0), Some(2), Some(usize::MAX)] {
                assert_eq!(
                    resource(broken.clone())
                        .decode_include(encoding, bound)
                        .unwrap_err()
                        .kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{encoding:?} {bound:?}"
                );
            }
            let good_full = [good, good].concat();
            let expected = decode(&good_full, encoding).unwrap().text;
            let full = resource(good_full)
                .decode_include(encoding, Some(1))
                .unwrap();
            assert!(full.complete);
            assert_eq!(full.resource.text, expected);
        }
        // Unsupported codecs and Ruby's implicit UTF-16 BOM selection boundary
        // must not silently fall back to a successful ASCII-prefix decode.
        for (encoding, bytes, kind) in [
            (
                Some("CP50220"),
                &b"good\n\xff"[..],
                ResourceErrorKind::Unsupported,
            ),
            (
                Some("Windows-1258"),
                &b"good\n\xff"[..],
                ResourceErrorKind::InvalidEncoding,
            ),
            (
                None,
                &b"\xff\xfe\x16\x04\x0a\x00\x00\xd8"[..],
                ResourceErrorKind::InvalidEncoding,
            ),
        ] {
            let resource = BinaryResource {
                name: "/part.adoc".into(),
                bytes: bytes.to_vec(),
                content_type: None,
            };
            assert_eq!(
                resource.decode_include(encoding, Some(1)).unwrap_err().kind,
                kind
            );
        }
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
            decode(b"ASCII", Some("CP50220")).unwrap_err().kind,
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
    fn multibyte_decoding_matches_exhaustive_ruby_fingerprints() {
        // Ruby 3.3.8, pinned Asciidoctor environment. Regenerate independently:
        // ruby scripts/generate_encodings.rb --fingerprints
        // All single bytes and pairs, plus every 8f-prefixed triple for EUC.
        for (name, expected, euc) in [
            ("Shift_JIS", 0xfcf4d82721c2f680_u64, false),
            ("Windows-31J", 0x800435c6fe305fcd, false),
            ("EUC-JP", 0x588386992351e229, true),
            ("eucJP-ms", 0x2d57700417373809, true),
            ("CP51932", 0x8a0af9cc4847e1c0, true),
            ("Big5", 0xc6c57394d274964c, false),
            ("Big5-HKSCS", 0x797a7cd7b15ceea4, false),
            ("Big5-UAO", 0x600344743be98c71, false),
            ("CP950", 0x03e1eff48f477005, false),
            ("CP951", 0x0d4e04e04e4ace29, false),
            ("GB2312", 0x39f3704f52914d93, false),
            ("GB12345", 0x7c1b0668bc543322, false),
            ("GBK", 0x461fd7b2edc98ce2, false),
            ("GB18030", 0xc15d224b9301efc0, false),
            ("EUC-KR", 0xfda2bfb2de08cf1d, false),
            ("CP949", 0x620f7b1e5bb833c1, false),
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
    fn iso2022_jp_states_aliases_and_errors_follow_ruby() {
        for name in ["ISO-2022-JP", "ISO2022-JP", "iSo2022-jP"] {
            for (bytes, expected) in [
                (&b""[..], ""),
                (&b"\x1b$B\x1b$@\x1b(B\x1b(J"[..], ""),
                (&b"\x1b$B$\""[..], "あ"),
                (
                    &b"\x1b$@$\"\x1b(J\\~\x1b$B$#\x1b(B fin\r\n"[..],
                    "あ\\~ぃ fin\r\n",
                ),
                (&b"\x00\t\r\n\x7f"[..], "\0\t\r\n\u{7f}"),
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap().text,
                    expected,
                    "{name}/{bytes:?}"
                );
            }
            for bytes in [
                &b"\x1b"[..],
                &b"\x1b$"[..],
                &b"\x1b("[..],
                &b"\x1b$B$"[..],
                &b"\x1b(I!"[..],
                &b"\x1b$(D"[..],
                &b"\x1b&@\x1b$B"[..],
                &b"\x0e"[..],
                &b"\x0f"[..],
                &b"\x80"[..],
                &b"\xff"[..],
                &b"\x1b$B$\"\n"[..],
                &b"\x1b$B$\n"[..],
                &b"\x1b$B$\x1b(B"[..],
                &b"\x1b$B~~"[..],
                &b"\xef\xbb\xbf"[..],
                &b"\xff\xfeA\x00"[..],
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{bytes:?}"
                );
            }
        }
        assert_eq!(
            decode(b"ASCII", Some("ISO2022-JP2")).unwrap_err().kind,
            ResourceErrorKind::InvalidEncoding
        );
        for name in [
            "CP50220",
            "CP50221",
            "ISO-2022-JP-KDDI",
            "stateless-ISO-2022-JP",
        ] {
            assert_eq!(
                decode(b"ASCII", Some(name)).unwrap_err().kind,
                ResourceErrorKind::Unsupported
            );
        }
    }

    #[test]
    fn iso2022_jp_matches_exhaustive_ruby_state_fingerprint() {
        // Ruby 3.3.8, generate_encodings.rb --fingerprints. 656,645 inputs:
        // all bytes/pairs and all escape candidates in each of five states.
        let mut hash = 0xcbf29ce484222325_u64;
        let mut feed = |byte| {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        };
        let mut check = |bytes: &[u8]| match decode(bytes, Some("ISO-2022-JP")) {
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
        for prefix in [
            &b""[..],
            &b"\x1b$B"[..],
            &b"\x1b$@"[..],
            &b"\x1b(B"[..],
            &b"\x1b(J"[..],
        ] {
            check(prefix);
            for a in 0..=255 {
                check(&[prefix, &[a]].concat());
            }
            for a in 0..=255 {
                for b in 0..=255 {
                    check(&[prefix, &[a, b]].concat());
                }
            }
            for a in 0..=255 {
                for b in 0..=255 {
                    check(&[prefix, &[0x1b, a, b, b'$', b'"']].concat());
                }
            }
        }
        assert_eq!(hash, 0xfdd8ceaeaea5a195);
    }

    #[test]
    fn gb18030_four_byte_space_matches_exhaustive_ruby_fingerprint() {
        // Independently recorded by generate_encodings.rb --fingerprints:
        // 1,587,600 syntactic quadruples, 158,760 truncated prefixes and
        // 3,072 mutations which also exercise invalid grammar and mixed input.
        let mut hash = 0xcbf29ce484222325_u64;
        let mut feed = |byte| {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        };
        let mut check = |bytes: &[u8]| match decode(bytes, Some("GB18030")) {
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
        for a in 0x81..=0xfe {
            for b in b'0'..=b'9' {
                for c in 0x81..=0xfe {
                    check(&[a, b, c]);
                    for d in b'0'..=b'9' {
                        check(&[a, b, c, d]);
                    }
                }
            }
        }
        for base in [
            [0x81, 0x30, 0x81, 0x30],
            [0x90, 0x30, 0x81, 0x30],
            [0xe3, 0x32, 0x9a, 0x35],
        ] {
            for index in 0..4 {
                for byte in 0..=255 {
                    let mut input = base;
                    input[index] = byte;
                    check(&input);
                }
            }
        }
        assert_eq!(hash, 0x8fda35a10e1c6317);
    }

    #[test]
    fn gb_families_preserve_reference_variants_aliases_and_four_byte_boundaries() {
        for (name, bytes, expected) in [
            ("GB2312", &b"\xd6\xd0\xb9\xfa"[..], "中国"),
            ("eUc-Cn", &b"\xd6\xd0\xb9\xfa"[..], "中国"),
            ("eucCN", &b"\xd6\xd0\xb9\xfa"[..], "中国"),
            ("GB12345", &b"\xd6\xd0\xb9\xfa"[..], "中國"),
            ("GBK", &b"\x80\x81\x40"[..], "€丂"),
            ("cP936", &b"\x80\x81\x40"[..], "€丂"),
            (
                "GB18030",
                &b"\xa2\xe3\xa8\xbc\x81\x35\xf4\x37"[..],
                "€\u{e7c7}ḿ",
            ),
            (
                "gB18030",
                &b"\x81\x30\x81\x30\x84\x31\xa4\x39\x90\x30\x81\x30\xe3\x32\x9a\x35"[..],
                "\u{80}\u{ffff}\u{10000}\u{10ffff}",
            ),
        ] {
            assert_eq!(decode(bytes, Some(name)).unwrap().text, expected, "{name}");
        }
        for name in ["GB2312", "GB12345", "GBK", "GB18030"] {
            assert_eq!(
                decode(b"ASCII \\~\r\n", Some(name)).unwrap().text,
                "ASCII \\~\r\n"
            );
        }
    }

    #[test]
    fn gb_codecs_reject_incomplete_invalid_and_unmapped_sequences() {
        for name in ["GB2312", "GB12345", "GBK", "GB18030"] {
            for bad in [
                &b"\xff"[..],
                &b"\xa1"[..],
                &b"\xa1\x7f"[..],
                &b"\xa1\x20"[..],
            ] {
                assert_eq!(
                    decode(bad, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}: {bad:?}"
                );
            }
        }
        for (name, bad) in [
            ("GB2312", &b"\x81\x40"[..]),
            ("GB12345", &b"\xa8\xbc"[..]),
            ("GBK", &b"\xa8\xbc"[..]),
            ("GB18030", &b"\x80"[..]),
            ("GB18030", &b"\x81\x30"[..]),
            ("GB18030", &b"\x81\x30\x81"[..]),
            ("GB18030", &b"\x81\x30\x80\x30"[..]),
            ("GB18030", &b"\x81\x30\x81\x0a"[..]),
            ("GB18030", &b"\x84\x31\xa5\x30"[..]),
            ("GB18030", &b"\x8f\x39\xfe\x39"[..]),
            ("GB18030", &b"\xe3\x32\x9a\x36"[..]),
            ("GB18030", &b"\xfe\x39\xfe\x39"[..]),
        ] {
            assert_eq!(
                decode(bad, Some(name)).unwrap_err().kind,
                ResourceErrorKind::InvalidEncoding,
                "{name}: {bad:?}"
            );
        }
    }

    #[test]
    fn big5_families_keep_ruby_mappings_and_aliases_distinct() {
        for (name, bytes, expected) in [
            (
                "Big5",
                &b"\xa1\x45\xa1\xe3\xa3\xe1\xc6\xa1"[..],
                "‧～€\u{f6b1}",
            ),
            (
                "bIg5-HkScS:2008",
                &b"\xa1\x45\xa1\xe3\x8e\x40\xc6\xa1"[..],
                "•∼𣻗①",
            ),
            ("Big5-UAO", &b"\x81\x40\x88\x62\xfe\x40"[..], "丗论氹"),
            (
                "CP950",
                &b"\x81\x40\xfe\x40\xc6\xa1"[..],
                "\u{eeb8}\u{e274}\u{f6b1}",
            ),
            ("cp951", &b"\x81\x40\xfe\x40\xc6\xa1"[..], "\u{eeb8}鑂①"),
        ] {
            assert_eq!(decode(bytes, Some(name)).unwrap().text, expected, "{name}");
        }
        for name in [
            "Big5",
            "Big5-HKSCS",
            "Big5-HKSCS:2008",
            "Big5-UAO",
            "CP950",
            "CP951",
        ] {
            assert_eq!(
                decode(b"ASCII \\~\r\n\xa4\xa4\xa4\xe5\xa4\x5c", Some(name))
                    .unwrap()
                    .text,
                "ASCII \\~\r\n中文么",
                "{name}"
            );
        }
        // Unlike web Big5 decoders, Ruby has no decomposed mapping for these.
        for bad in [
            b"\x88\x62",
            b"\x88\x64",
            b"\x88\xa3",
            b"\x88\xa5",
            b"\xa3\xe1",
        ] {
            assert_eq!(
                decode(bad, Some("Big5-HKSCS")).unwrap_err().kind,
                ResourceErrorKind::InvalidEncoding
            );
        }
        // Ruby does not recognize this spelling; unknown names remain UTF-8.
        assert_eq!(
            decode("中文".as_bytes(), Some("csBig5")).unwrap().text,
            "中文"
        );
    }

    #[test]
    fn big5_codecs_reject_incomplete_invalid_and_unmapped_bytes() {
        for name in ["Big5", "Big5-HKSCS", "Big5-UAO", "CP950", "CP951"] {
            for bad in [
                &b"\x81"[..],
                &b"\x80"[..],
                &b"\xff"[..],
                &b"\xa4\x7f"[..],
                &b"\xa4\x80"[..],
                &b"\xa4\x20"[..],
                &b"\xa4\xa4\xa4"[..],
            ] {
                let error = decode(bad, Some(name)).unwrap_err();
                assert_eq!(
                    error.kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{bad:?}"
                );
            }
        }
        for (name, bytes) in [("Big5", b"\x81\x40"), ("Big5-HKSCS", b"\x81\x40")] {
            assert_eq!(
                decode(bytes, Some(name)).unwrap_err().kind,
                ResourceErrorKind::InvalidEncoding
            );
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
    #[test]
    fn korean_variants_aliases_and_invalid_sequences_follow_ruby() {
        for name in ["EUC-KR", "eucKR", "CP949"] {
            assert_eq!(
                decode(b"\xb0\xa1\xa2\xe6\r\n", Some(name)).unwrap().text,
                "가€\r\n"
            );
            for bad in [&b"\x80"[..], &b"\xff"[..], &b"\x81"[..], &b"\xb0\x20"[..]] {
                assert_eq!(
                    decode(bad, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding
                );
            }
        }
        assert_eq!(
            decode(b"\x81\x41\xc6\x52", Some("CP949")).unwrap().text,
            "갂힣"
        );
        assert_eq!(
            decode(b"\x81\x41", Some("EUC-KR")).unwrap_err().kind,
            ResourceErrorKind::InvalidEncoding
        );
    }
}
