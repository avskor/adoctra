//! Decode include bytes before registering their UTF-8 source snapshot.
//! Names and byte mappings follow the pinned Ruby reference. Codecs without
//! a reference converter remain unreadable; never mistake an unavailable
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
    Cesu8,
    Utf8Mac,
    Utf8Mobile(&'static [(u32, Option<&'static str>)]),
    Ascii,
    Binary,
    Utf16(Option<bool>),
    Utf32(Option<bool>),
    Single(&'static [u32]),
    Iso2022Jp {
        table: &'static [u64],
        kind: Iso2022Kind,
    },
    Multibyte {
        table: &'static [u64],
        kind: MultibyteKind,
    },
    Unavailable,
}

enum MultibyteKind {
    ShiftJis,
    ShiftJisKddi,
    ShiftJisDocomo,
    ShiftJisSoftbank,
    EucJp,
    StatelessIso2022Jp,
    StatelessIso2022JpKddi,
    Big5,
    Gb2312,
    Korean,
    Gbk,
    Gb18030,
}

enum Iso2022State {
    Ascii,
    Jis0208 { modern: bool },
    Katakana,
}

enum Iso2022Kind {
    Standard,
    Windows,
    Kddi,
}

fn decode_utf8_mac(input: &str) -> String {
    fn compose(first: char, second: char) -> Option<char> {
        let first = first as u32;
        let second = second as u32;
        let scalar = if (0x1100..=0x1112).contains(&first) && (0x1161..=0x1175).contains(&second) {
            0xac00 + ((first - 0x1100) * 21 + second - 0x1161) * 28
        } else if (0xac00..=0xd7a3).contains(&first)
            && (first - 0xac00) % 28 == 0
            && (0x11a8..=0x11c2).contains(&second)
        {
            first + second - 0x11a7
        } else {
            let key = (u64::from(first) << 32) | u64::from(second);
            let index = tables::UTF8_MAC_COMPOSITIONS
                .binary_search_by_key(&key, |&(key, _)| key)
                .ok()?;
            tables::UTF8_MAC_COMPOSITIONS[index].1
        };
        char::from_u32(scalar)
    }
    // Ruby composes adjacent pairs greedily. It preserves precomposed input,
    // exclusions and mark order; no decomposition or canonical reordering runs.
    let mut text = String::with_capacity(input.len());
    for character in input.chars() {
        if let Some(composite) = text
            .chars()
            .next_back()
            .and_then(|last| compose(last, character))
        {
            text.pop();
            text.push(composite);
        } else {
            text.push(character);
        }
    }
    text
}

fn decode_cesu8(bytes: &[u8]) -> Option<String> {
    // CESU-8 encodes UTF-16 units as one to three bytes. Supplementary
    // characters require adjacent high/low surrogates; native UTF-8's
    // four-byte form and unpaired surrogates are invalid in Ruby IO.
    fn unit(bytes: &[u8]) -> Option<(u16, usize)> {
        let lead = *bytes.first()?;
        match lead {
            0..=0x7f => Some((u16::from(lead), 1)),
            0xc2..=0xdf => {
                let second = *bytes.get(1)?;
                if !(0x80..=0xbf).contains(&second) {
                    return None;
                }
                Some(((u16::from(lead & 0x1f) << 6) | u16::from(second & 0x3f), 2))
            }
            0xe0..=0xef => {
                let second = *bytes.get(1)?;
                let third = *bytes.get(2)?;
                if !(0x80..=0xbf).contains(&second)
                    || !(0x80..=0xbf).contains(&third)
                    || (lead == 0xe0 && second < 0xa0)
                {
                    return None;
                }
                Some((
                    (u16::from(lead & 0x0f) << 12)
                        | (u16::from(second & 0x3f) << 6)
                        | u16::from(third & 0x3f),
                    3,
                ))
            }
            _ => None,
        }
    }
    let mut text = String::with_capacity(bytes.len());
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let (first, length) = unit(remaining)?;
        remaining = &remaining[length..];
        let scalar = if (0xd800..=0xdbff).contains(&first) {
            let (second, length) = unit(remaining)?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return None;
            }
            remaining = &remaining[length..];
            0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00
        } else {
            u32::from(first)
        };
        text.push(char::from_u32(scalar)?);
    }
    Some(text)
}

fn append_mapping(
    text: &mut String,
    table: &[u64],
    key: u64,
    overrides: &[(u16, Option<&str>)],
) -> Option<()> {
    if let Ok(index) = overrides.binary_search_by_key(&key, |&(key, _)| u64::from(key)) {
        // Overrides can contain multiple Unicode scalars, or explicitly
        // reject a mapping present in the shared scalar table.
        text.push_str(overrides[index].1?);
        return Some(());
    }
    let index = table.binary_search_by_key(&key, |entry| entry >> 32).ok()?;
    text.push(char::from_u32(table[index] as u32)?);
    Some(())
}

/// How the pinned OpenURI reader sees a resource it tags with an external
/// encoding without transcoding (see [`BinaryResource::prepare_uri_include`]).
pub(crate) struct RawCodec {
    /// UTF-8 (also an unknown name) and BINARY: no transcoding happens for a
    /// file either, so the file's deferred UTF-8/BINARY path applies as is.
    pub(crate) deferred: Option<bool>,
    /// `String#include?('::')` raises for an ASCII-incompatible (or dummy)
    /// encoding, so the tag scan fails on the first line.
    pub(crate) ascii_compatible: bool,
    /// Every byte is a valid character, so a regexp never raises on a line
    /// even where no Unicode mapping exists.
    pub(crate) single_byte: bool,
    /// The encoded LF that `each_line` searches for.
    pub(crate) separator: &'static [u8],
}

pub(crate) fn raw_codec(encoding: Option<&str>) -> RawCodec {
    let name = encoding.unwrap_or_default().to_ascii_lowercase();
    let codec = |ascii_compatible, single_byte, separator| RawCodec {
        deferred: None,
        ascii_compatible,
        single_byte,
        separator,
    };
    match tables::lookup(&name).unwrap_or(Encoding::Utf8) {
        Encoding::Utf8 | Encoding::Binary => RawCodec {
            deferred: Some(matches!(tables::lookup(&name), Some(Encoding::Binary))),
            ..codec(true, false, b"\n")
        },
        // Ruby's non-dummy UTF-16/32 split at their encoded LF; the BOM-led
        // dummy UTF-16/UTF-32 split at the LF byte like every other encoding.
        Encoding::Utf16(Some(true)) => codec(false, false, b"\n\0"),
        Encoding::Utf16(Some(false)) => codec(false, false, b"\0\n"),
        Encoding::Utf32(Some(true)) => codec(false, false, b"\n\0\0\0"),
        Encoding::Utf32(Some(false)) => codec(false, false, b"\0\0\0\n"),
        Encoding::Utf16(None) | Encoding::Utf32(None) => codec(false, false, b"\n"),
        Encoding::Iso2022Jp { .. } => codec(false, false, b"\n"),
        // EBCDIC (IBM037) is a dummy encoding in Ruby.
        Encoding::Single(table) => codec(table.len() < 256 || table[0x41] == 0x41, true, b"\n"),
        _ => codec(true, false, b"\n"),
    }
}

impl BinaryResource {
    /// Prepare a URI include like the pinned Ruby's OpenURI reader: the
    /// response is tagged with the external encoding but not transcoded, so
    /// lines are split in that encoding and selection runs before only the
    /// selected lines are converted (`prepare_source_array`, whose BOM check
    /// looks at the first selected line). A whole read converts at once
    /// (`prepare_source_string`). Unselected undecodable lines never fail.
    pub fn prepare_uri_include(self, encoding: Option<&str>) -> IncludeInput {
        IncludeInput::Unconverted {
            resource: self,
            encoding: encoding.map(str::to_owned),
        }
    }

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
            && (self.bytes.starts_with(&[0xff, 0xfe])
                || self.bytes.starts_with(&[0xfe, 0xff])
                || self.decode_text(encoding).is_err())
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
    /// Reference codecs without a UTF-8 converter remain unreadable; a host
    /// may override decoding through `ResourceProvider::load_with_encoding`.
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
            Encoding::Unavailable => return None,
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
                    let end = (index + 1) * width;
                    if matches!(encoding, Encoding::Utf8Mac) {
                        // Ruby buffers LF until the next complete scalar (or
                        // clean EOF). An invalid first scalar after LF prevents
                        // gets from returning the selected line. Validate this
                        // lookahead without adding unread text to the snapshot.
                        let tail = &self.bytes[end..];
                        let lookahead = &tail[..tail.len().min(4)];
                        if matches!(std::str::from_utf8(lookahead), Err(error) if error.valid_up_to() == 0)
                        {
                            return None;
                        }
                    }
                    return Some(end);
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
            Encoding::Cesu8 => decode_cesu8(bytes).ok_or_else(invalid)?,
            Encoding::Utf8Mac => {
                decode_utf8_mac(std::str::from_utf8(bytes).map_err(|_| invalid())?)
            }
            Encoding::Utf8Mobile(overrides) => {
                // Mobile UTF8 codecs are strict transcoders, not the default
                // UTF-8 reader's deferred selection path. Other Unicode scalars
                // pass through unchanged; explicit undefined PUA values fail.
                let input = std::str::from_utf8(bytes).map_err(|_| invalid())?;
                let mut text = String::with_capacity(input.len());
                for character in input.chars() {
                    match overrides.binary_search_by_key(&(character as u32), |&(key, _)| key) {
                        Ok(index) => text.push_str(overrides[index].1.ok_or_else(invalid)?),
                        Err(_) => text.push(character),
                    }
                }
                text
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
            Encoding::Iso2022Jp { table, kind } => {
                let windows = matches!(kind, Iso2022Kind::Windows);
                let kddi = matches!(kind, Iso2022Kind::Kddi);
                let mut text = String::with_capacity(bytes.len());
                let mut remaining = bytes.as_slice();
                let mut state = Iso2022State::Ascii;
                while let Some(&first) = remaining.first() {
                    if first == 0x1b {
                        // Ruby accepts JIS X 0208-1978/1983 and treats Roman
                        // like ASCII (including backslash and tilde). It does
                        // not accept JIS X 0212 or revision escapes. Windows
                        // variants additionally designate halfwidth katakana.
                        state = match remaining.get(..3) {
                            Some([0x1b, b'$', b'@']) => Iso2022State::Jis0208 { modern: false },
                            Some([0x1b, b'$', b'B']) => Iso2022State::Jis0208 { modern: true },
                            Some([0x1b, b'(', b'B' | b'J']) => Iso2022State::Ascii,
                            Some([0x1b, b'(', b'I']) if windows => Iso2022State::Katakana,
                            _ => return Err(invalid()),
                        };
                        remaining = &remaining[3..];
                    } else if windows && matches!(first, 0x0e | 0x0f) {
                        // SI always selects ASCII, rather than restoring the
                        // designation preceding SO. Shifts inside a JIS pair
                        // are rejected by the pair grammar below.
                        state = if first == 0x0e {
                            Iso2022State::Katakana
                        } else {
                            Iso2022State::Ascii
                        };
                        remaining = &remaining[1..];
                    } else if matches!(state, Iso2022State::Jis0208 { .. }) {
                        let sequence = remaining.get(..2).ok_or_else(invalid)?;
                        if !sequence.iter().all(|byte| (0x21..=0x7e).contains(byte)) {
                            return Err(invalid());
                        }
                        // The 1978 designation itself is valid, even for KDDI.
                        // KDDI rejects its characters; Windows only rejects
                        // NEC/IBM extension rows, retaining base Unicode remaps.
                        if matches!(state, Iso2022State::Jis0208 { modern: false })
                            && (kddi || (windows && matches!(first, 0x2d | 0x79..=0x7c)))
                        {
                            return Err(invalid());
                        }
                        // JIS pairs reuse EUC-JP's table for ISO-2022-JP,
                        // CP51932's for CP50220/CP50221, preserving variants.
                        let key = (u64::from(first + 0x80) << 8) | u64::from(sequence[1] + 0x80);
                        let overrides: &[(u16, Option<&str>)] =
                            if kddi { &tables::KDDI_OVERRIDES } else { &[] };
                        append_mapping(&mut text, table, key, overrides).ok_or_else(invalid)?;
                        remaining = &remaining[2..];
                    } else if (windows && (0xa1..=0xdf).contains(&first))
                        || (matches!(state, Iso2022State::Katakana)
                            && (0x21..=0x5f).contains(&first))
                    {
                        text.push(
                            char::from_u32(0xff61 + u32::from((first & 0x7f) - 0x21)).unwrap(),
                        );
                        remaining = &remaining[1..];
                    } else {
                        if matches!(state, Iso2022State::Katakana) {
                            return Err(invalid());
                        }
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
                        // Ruby IO invokes this transcoder even for ASCII-only
                        // strings; String#encode's ASCII fast path would let
                        // these forbidden stateless controls pass incorrectly.
                        if matches!(kind, MultibyteKind::StatelessIso2022Jp)
                            && matches!(first, 0x0e | 0x0f | 0x1b)
                        {
                            return Err(invalid());
                        }
                        text.push(char::from(first));
                        remaining = &remaining[1..];
                        continue;
                    }
                    let width = match kind {
                        MultibyteKind::ShiftJis
                        | MultibyteKind::ShiftJisKddi
                        | MultibyteKind::ShiftJisDocomo
                        | MultibyteKind::ShiftJisSoftbank => match first {
                            0xa1..=0xdf => 1,
                            0x81..=0x9f | 0xe0..=0xfc => 2,
                            _ => return Err(invalid()),
                        },
                        MultibyteKind::EucJp => match first {
                            0x8e | 0xa1..=0xfe => 2,
                            0x8f => 3,
                            _ => return Err(invalid()),
                        },
                        MultibyteKind::StatelessIso2022Jp => match first {
                            0x90 | 0x92 => 3,
                            _ => return Err(invalid()),
                        },
                        MultibyteKind::StatelessIso2022JpKddi => match first {
                            0x92 => 3,
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
                    let kddi = matches!(kind, MultibyteKind::StatelessIso2022JpKddi);
                    let sequence = if matches!(kind, MultibyteKind::StatelessIso2022Jp) || kddi {
                        // Ordinary stateless JP uses 90/92, EUC-JP mappings,
                        // and rejects ESC/SO/SI in the ASCII branch. KDDI uses
                        // only 92, preserves ASCII controls, and also defines
                        // IBM rows 93-97. Its overrides validate these and reject
                        // kana/undefined entries in the shared CP51932 table.
                        if !kddi
                            && !sequence[1..]
                                .iter()
                                .all(|byte| (0xa1..=0xfe).contains(byte))
                        {
                            return Err(invalid());
                        }
                        &sequence[1..]
                    } else {
                        sequence
                    };
                    let key = sequence
                        .iter()
                        .fold(0_u64, |key, byte| (key << 8) | u64::from(*byte));
                    let overrides: &[(u16, Option<&str>)] = match kind {
                        MultibyteKind::StatelessIso2022JpKddi => &tables::KDDI_OVERRIDES,
                        MultibyteKind::ShiftJisKddi => &tables::SJIS_KDDI_OVERRIDES,
                        MultibyteKind::ShiftJisDocomo => &tables::SJIS_DOCOMO_OVERRIDES,
                        MultibyteKind::ShiftJisSoftbank => &tables::SJIS_SOFTBANK_OVERRIDES,
                        _ => &[],
                    };
                    append_mapping(&mut text, table, key, overrides).ok_or_else(invalid)?;
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
                // Ruby's dummy UTF-16 converter consumes the BOM it reads the
                // byte order from; a following U+FEFF is text (which the
                // first-selected-line BOM check then removes).
                let bytes = match endian {
                    None => bytes.get(2..).unwrap_or_default(),
                    Some(_) => bytes,
                };
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
                // Like UTF-16, the dummy UTF-32 converter consumes its BOM.
                let bytes = match endian {
                    None => bytes.get(4..).unwrap_or_default(),
                    Some(_) => bytes,
                };
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
                Some("stateless-ISO-2022-JP"),
                &b"\x90\xa4\xa2\r\n"[..],
                &b"\x92\xa4"[..],
            ),
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
        // Unavailable codecs and Ruby's implicit UTF-16 BOM selection boundary
        // must not silently fall back to a successful ASCII-prefix decode.
        for (encoding, bytes, kind) in [
            (
                Some("UTF-7"),
                &b"good\n\xff"[..],
                ResourceErrorKind::InvalidEncoding,
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
                Some("BINARY"),
                Some(if little { "UTF-16LE" } else { "UCS-2BE" }),
            ] {
                assert_eq!(decode(&utf16, label).unwrap().text, text);
            }
            let label = if little { "UCS-4LE" } else { "UTF-32BE" };
            assert_eq!(decode(&utf32, Some(label)).unwrap().text, text);
            // The dummy converters consume the BOM they read the order from,
            // and only that one: a second BOM stays U+FEFF.
            let content = text.strip_prefix('\u{feff}').unwrap();
            assert_eq!(decode(&utf16, Some("UTF-16")).unwrap().text, content);
            assert_eq!(decode(&utf32, Some("UTF-32")).unwrap().text, content);
            let twice = [&utf16[..2], &utf16[..]].concat();
            assert_eq!(decode(&twice, Some("UTF-16")).unwrap().text, text);
            let twice = [&utf32[..4], &utf32[..]].concat();
            assert_eq!(decode(&twice, Some("UTF-32")).unwrap().text, text);
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
            decode(b"ASCII", Some("UTF-7")).unwrap_err().kind,
            ResourceErrorKind::InvalidEncoding
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
    fn stateless_jp_triples_reuse_jis_mappings_without_escape_states() {
        for name in ["stateless-ISO-2022-JP", "STATELESS-ISO-2022-JP"] {
            for (bytes, expected) in [
                (&b"\x92\xc6\xfc\x90\xcb\xdc\x92\xb8\xec"[..], "日本語"),
                (&b"\x90\xa1\xc1\x92\xa1\xc1"[..], "〜〜"),
                (&b"\0\t\r\n\x7f"[..], "\0\t\r\n\u{7f}"),
                (&b""[..], ""),
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap().text,
                    expected,
                    "{name}/{bytes:?}"
                );
            }
            for bytes in [
                &b"\x1b$B$\"\x1b(B\x0e1\x0f"[..],
                &b"\x92\xa4\xa2\x1b\x90\xa4\xa4\n"[..],
                &b"\x1b"[..],
                &b"\x0e"[..],
                &b"\x0f"[..],
                &b"\x90"[..],
                &b"\x92"[..],
                &b"\x90\xa4"[..],
                &b"\x92\xa4"[..],
                &b"\x90$\xa2"[..],
                &b"\x92\xa4\""[..],
                &b"\x90\xa4\n"[..],
                &b"\x91\xa4\xa2"[..],
                &b"\x90\xfe\xfe"[..],
                &b"\x92\xad\xa1"[..],
                &b"\xa4\xa2"[..],
                &b"\x8e\xb1"[..],
                &b"\x8f\xa2\xaf"[..],
                &b"\x90\x8e\xb1"[..],
                &b"\x92\x8f\xa2\xaf"[..],
                &b"\xef\xbb\xbf"[..],
                &b"\xff\xfeA\0"[..],
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{bytes:?}"
                );
            }
        }
    }

    #[test]
    fn kddi_variants_decode_compounds_and_keep_distinct_byte_grammars() {
        for (name, flag, keycap, supplementary) in [
            (
                "ISO-2022-JP-KDDI",
                &b"\x1b$Bv'\x1b(B"[..],
                &b"\x1b$Bv}\x1b(B"[..],
                &b"\x1b$Bu_\x1b(B"[..],
            ),
            (
                "stateless-ISO-2022-JP-KDDI",
                &b"\x92\xf6\xa7"[..],
                &b"\x92\xf6\xfd"[..],
                &b"\x92\xf5\xdf"[..],
            ),
        ] {
            assert_eq!(decode(flag, Some(name)).unwrap().text, "🇯🇵");
            assert_eq!(
                decode(keycap, Some(&name.to_uppercase())).unwrap().text,
                "1\u{20e3}"
            );
            assert_eq!(decode(supplementary, Some(name)).unwrap().text, "🅿");
        }
        assert_eq!(
            decode(
                b"\x92\x93\xa1\x92\xa1\xc1\x92\xad\xa1",
                Some("stateless-ISO-2022-JP-KDDI")
            )
            .unwrap()
            .text,
            "ⅰ～①"
        );
        // Unlike the non-KDDI stateless transcoder, KDDI preserves controls.
        let literal = b"\x1b$B$\"\x1b(B\x0e1\x0f";
        assert_eq!(
            decode(literal, Some("stateless-ISO-2022-JP-KDDI"))
                .unwrap()
                .text
                .as_bytes(),
            literal
        );
        for (bytes, expected) in [
            (&b"\x1b$@"[..], ""),
            (&b"\x1b$@\x1b(BA"[..], "A"),
            (&b"\x1b$@\x1b$Bv'\x1b(B"[..], "🇯🇵"),
            (&b"\x1b$Bv'\x1b$@"[..], "🇯🇵"),
        ] {
            assert_eq!(
                decode(bytes, Some("ISO-2022-JP-KDDI")).unwrap().text,
                expected
            );
        }
        for bytes in [
            &b"\x1b$@$\""[..],
            &b"\x1b(I1"[..],
            &b"\x0e"[..],
            &b"\x0f"[..],
            &b"\x1b$Bu+\x1b(B"[..], // Reverse UTF-8 encoding of 😀 is not a valid decode mapping.
            &b"\x1b$Bv'\n"[..],
            &b"\x1b$Bv\x1b(B"[..],
            &b"\x92\xf6\xa7"[..],
        ] {
            assert_eq!(
                decode(bytes, Some("ISO-2022-JP-KDDI")).unwrap_err().kind,
                ResourceErrorKind::InvalidEncoding,
                "{bytes:?}"
            );
        }
        for bytes in [
            &b"\x90\xa4\xa2"[..],
            &b"\x90\xf6\xa7"[..],
            &b"\x92"[..],
            &b"\x92\xf6"[..],
            &b"\x92\x8e\xb1"[..],
            &b"\x92\xf5\xab"[..],
            &b"\x92\xfe\xfe"[..],
            &b"\x8e\xb1"[..],
        ] {
            assert_eq!(
                decode(bytes, Some("stateless-ISO-2022-JP-KDDI"))
                    .unwrap_err()
                    .kind,
                ResourceErrorKind::InvalidEncoding,
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn jis2004_aliases_supplementary_mappings_and_undefined_combinations_follow_ruby() {
        for name in ["EUC-JIS-2004", "EUC-JISX0213", "eUc-JiS-2004"] {
            for (bytes, expected) in [
                (&b"\xc6\xfc\xcb\xdc\xb8\xec"[..], "日本語"),
                (&b"\x8f\xa1\xa1"[..], "𠂉"),
                (&b"\x8f\xfe\xf6"[..], "𪚲"),
                (&b"\xa2\xaf\xa2\xb2"[..], "＇～"),
                (&b"\x8e\xb1\x8e\xde"[..], "ｱﾞ"),
                (&b"\x1b$B\x0e\x0f"[..], "\u{1b}$B\u{e}\u{f}"),
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap().text,
                    expected,
                    "{name}/{bytes:?}"
                );
            }
            // Pinned Ruby rejects these 25 combining mappings. Do not fill
            // them from other codecs' tables or compose their Unicode output.
            for pair in [
                0xa4f7_u16, 0xa4f8, 0xa4f9, 0xa4fa, 0xa4fb, 0xa5f7, 0xa5f8, 0xa5f9, 0xa5fa, 0xa5fb,
                0xa5fc, 0xa5fd, 0xa5fe, 0xa6f8, 0xabc4, 0xabc8, 0xabc9, 0xabca, 0xabcb, 0xabcc,
                0xabcd, 0xabce, 0xabcf, 0xabe5, 0xabe6,
            ] {
                assert_eq!(
                    decode(&pair.to_be_bytes(), Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{pair:x}"
                );
            }
            for bytes in [
                &b"\xa4"[..],
                &b"\x8f"[..],
                &b"\x8f\xa1"[..],
                &b"\x8e"[..],
                &b"\x8e\xe0"[..],
                &b"\xa0\xa1"[..],
                &b"\xa4 "[..],
                &b"\x8f\xa1\n"[..],
                &b"\x8f\xa2\xaf"[..],
                &b"\xef\xbb\xbf"[..],
                &b"\xff\xfeA\0"[..],
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{bytes:?}"
                );
            }
        }
        assert_eq!(decode(b"\x8f\xa2\xaf", Some("EUC-JP")).unwrap().text, "˘");
    }

    #[test]
    fn sjis_kddi_controls_compounds_private_use_and_undefined_emoji_follow_ruby_io() {
        for name in ["SJIS-KDDI", "sjis-kddi"] {
            for (bytes, expected) in [
                (&b"\x93\xfa\x96\x7b\x8c\xea"[..], "日本語"),
                (&b"\xf6\xa5"[..], "🇯🇵"),
                (&b"\xf6\xfb"[..], "1\u{20e3}"),
                (&b"\xf7\xc9"[..], "0\u{20e3}"),
                (&b"\xf6\x7e"[..], "🅿"),
                (&b"\xf6\x60"[..], "☀"),
                (&b"\xf0\x40\xfa\x40\x81\x60"[..], "\u{e000}ⅰ～"),
                (&b"\xb1\xde"[..], "ｱﾞ"),
                (&b"\x1b$B\x0e1\x0f"[..], "\u{1b}$B\u{e}1\u{f}"),
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap().text,
                    expected,
                    "{name}/{bytes:?}"
                );
            }
            for bytes in [
                &b"\x81"[..],
                &b"\xf6"[..],
                &b"\xf6 "[..],
                &b"\xf6\x7f"[..],
                &b"\xf6\xfd"[..],
                &b"\x80"[..],
                &b"\xa0"[..],
                &b"\xfd"[..],
                &b"\xfe"[..],
                &b"\xff"[..],
                &b"\xf3\xf3"[..],
                &b"\xf6\x4a"[..],
                &b"\xef\xbb\xbf"[..],
                &b"\xff\xfeA\0"[..],
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{bytes:?}"
                );
            }
        }
        assert_eq!(
            decode(b"\xf6\xa5", Some("Windows-31J")).unwrap().text,
            "\u{e4cc}"
        );
        // Reverse encoding of 😀 is not evidence that Ruby can decode F64A.
        assert!(decode(b"\xf6\x4a", Some("Windows-31J")).is_ok());
    }

    #[test]
    fn sjis_docomo_keycaps_supplementary_and_undefined_mappings_follow_ruby_io() {
        for name in ["SJIS-DoCoMo", "sjis-docomo"] {
            for (bytes, expected) in [
                (&b"\x93\xfa\x96\x7b\x8c\xea"[..], "日本語"),
                (&b"\xf9\x87"[..], "1\u{20e3}"),
                (&b"\xf9\x85"[..], "#\u{20e3}"),
                (&b"\xf9\x90"[..], "0\u{20e3}"),
                (&b"\xf8\xcd"[..], "🅿"),
                (&b"\xf8\x9f\xf9\x91"[..], "☀❤"),
                (&b"\xf0\x40\xfa\x40\x81\x60"[..], "\u{e000}ⅰ～"),
                (&b"\xb1\xde"[..], "ｱﾞ"),
                (&b"\x1b$B\x0e1\x0f"[..], "\u{1b}$B\u{e}1\u{f}"),
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap().text,
                    expected,
                    "{name}/{bytes:?}"
                );
            }
            for bytes in [
                &b"\x81"[..],
                &b"\xf9"[..],
                &b"\xf9 "[..],
                &b"\xf9\x7f"[..],
                &b"\xf9\xfd"[..],
                &b"\x80"[..],
                &b"\xa0"[..],
                &b"\xfd"[..],
                &b"\xfe"[..],
                &b"\xff"[..],
                &b"\xf9\xfc"[..],
                &b"\xf9\x96"[..],
                &b"\xef\xbb\xbf"[..],
                &b"\xff\xfeA\0"[..],
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{bytes:?}"
                );
            }
        }
        assert_eq!(
            decode(b"\xf9\x87", Some("Windows-31J")).unwrap().text,
            "\u{e6e2}"
        );
        assert!(decode(b"\xf9\x96", Some("Windows-31J")).is_ok());
    }

    #[test]
    fn softbank_duplicate_forms_and_literal_escape_sequences_follow_ruby_io() {
        for name in ["SJIS-SoftBank", "sjis-softbank"] {
            for (bytes, expected) in [
                (&b"\xf6\xe4\xfb\xab"[..], "🇯🇵🇯🇵"),
                (&b"\xf2\xe5\xf7\xbc"[..], "1\u{20e3}1\u{20e3}"),
                (&b"\xf2\xd9\xf7\xb0"[..], "#\u{20e3}#\u{20e3}"),
                (&b"\xf1\xd4\xf7\x90"[..], "🅿🅿"),
                (&b"\xf0\x8b\xf9\x8b"[..], "☀☀"),
                (&b"\xf0\x62\xf9\x62"[..], "❤❤"),
                (&b"\xf0\x40\x81\x60\xb1\xde"[..], "\u{e000}～ｱﾞ"),
                (
                    &b"\x1b$G!\x0f\x1b$P!\x0f"[..],
                    "\u{1b}$G!\u{f}\u{1b}$P!\u{f}",
                ),
                (&b"\x1b$G!"[..], "\u{1b}$G!"),
                (&b"\x1b$G"[..], "\u{1b}$G"),
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap().text,
                    expected,
                    "{name}/{bytes:?}"
                );
            }
            for bytes in [
                &b"\x81"[..],
                &b"\xf6"[..],
                &b"\xf6 "[..],
                &b"\xf6\x7f"[..],
                &b"\xf6\xfd"[..],
                &b"\x80"[..],
                &b"\xa0"[..],
                &b"\xfd"[..],
                &b"\xfe"[..],
                &b"\xff"[..],
                &b"\xf9\x9a"[..],
                &b"\xef\xbb\xbf"[..],
                &b"\xff\xfeA\0"[..],
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{bytes:?}"
                );
            }
        }
        assert_eq!(
            decode(b"\xf6\xe4", Some("Windows-31J")).unwrap().text,
            "\u{e50b}"
        );
        assert_eq!(
            decode(b"\xfb\xab", Some("Windows-31J")).unwrap().text,
            "\u{8b53}"
        );
        assert!(decode(b"\xf9\x9a", Some("Windows-31J")).is_ok());
    }

    #[test]
    fn utf8_mac_prefix_requires_one_valid_scalar_after_buffered_lf() {
        for tail in [
            &b"A\xff"[..],
            &b"\xc2\xa2\xff"[..],
            &b"\xee\x80\x80\xff"[..],
            &b"\xf0\x9f\x98\x80\xff"[..],
            &b"\xef\xbb\xbf\xff"[..],
        ] {
            let decoded = BinaryResource {
                name: "/part.adoc".into(),
                bytes: ["*e\u{301}*\r\n".as_bytes(), tail].concat(),
                content_type: None,
            }
            .decode_include(Some("UTF-8-HFS"), Some(1))
            .unwrap();
            assert_eq!(decoded.resource.text, "*é*\r\n");
            assert!(!decoded.complete);
        }
        for tail in [
            &b"\xff"[..],
            &b"\xee\x80\xff"[..],
            &b"\xed\xa0\x80"[..],
            &b"\xf4\x90\x80\x80"[..],
        ] {
            let error = BinaryResource {
                name: "/part.adoc".into(),
                bytes: ["*e\u{301}*\r\n".as_bytes(), tail].concat(),
                content_type: None,
            }
            .decode_include(Some("UTF8-MAC"), Some(1))
            .unwrap_err();
            assert_eq!(error.kind, ResourceErrorKind::InvalidEncoding);
        }
        let eof = BinaryResource {
            name: "/part.adoc".into(),
            bytes: "*e\u{301}*\r\n".as_bytes().to_vec(),
            content_type: None,
        }
        .decode_include(Some("UTF8-MAC"), Some(1))
        .unwrap();
        assert_eq!(eof.resource.text, "*é*\r\n");
        assert!(eof.complete);
    }

    #[test]
    fn utf8_mac_composes_adjacent_pairs_and_hangul_with_reference_exclusions() {
        for name in ["UTF8-MAC", "UTF-8-MAC", "utf-8-hfs", "uTf8-mAc"] {
            for (input, expected) in [
                ("e\u{301} A\u{30a}\u{301}", "é Ǻ"),
                ("a\u{315}\u{300}", "a\u{315}\u{300}"),
                ("a\u{300}\u{315}", "à\u{315}"),
                ("e\u{301}\u{323}", "é\u{323}"),
                ("\u{3b9}\u{308}\u{301}", "\u{3ca}\u{301}"),
                ("\u{3b1}\u{313}\u{300}\u{345}", "\u{1f82}"),
                ("\u{1100}\u{1161}\u{11a8} \u{ac00}\u{11a8}", "각 각"),
                ("\u{1112}\u{1175}\u{11c2}", "\u{d7a3}"),
                (
                    "\u{1100}\u{1160} \u{ac01}\u{11a8}",
                    "\u{1100}\u{1160} 각\u{11a8}",
                ),
                (
                    "é 가 Å K Ω \u{344} \u{f900} \u{2f800}",
                    "é 가 Å K Ω \u{344} \u{f900} \u{2f800}",
                ),
                (
                    "\u{915}\u{93c} \u{5e9}\u{5c1}",
                    "\u{915}\u{93c} \u{5e9}\u{5c1}",
                ),
                ("e😀\u{301} e\n\u{301}", "e😀\u{301} e\n\u{301}"),
                (
                    "\u{feff}e\u{301}\u{e6f1}\u{ffff}\u{10ffff}",
                    "\u{feff}é\u{e6f1}\u{ffff}\u{10ffff}",
                ),
                ("\0\u{1b}$G!\u{e}\u{f}", "\0\u{1b}$G!\u{e}\u{f}"),
            ] {
                assert_eq!(
                    decode(input.as_bytes(), Some(name)).unwrap().text,
                    expected,
                    "{name}/{input:?}"
                );
            }
            for input in [
                &b"\xff"[..],
                &b"\xed\xa0\x80"[..],
                &b"\xc0\x80"[..],
                &b"\xf4\x90\x80\x80"[..],
                &b"\xff\xfeA\0"[..],
            ] {
                assert_eq!(
                    decode(input, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding
                );
            }
        }
        assert_eq!(
            decode("e\u{301}".as_bytes(), Some("UTF-8")).unwrap().text,
            "e\u{301}"
        );
    }

    #[test]
    fn cesu8_requires_canonical_units_and_adjacent_surrogate_pairs() {
        for name in ["CESU-8", "cEsU-8"] {
            for (bytes, expected) in [
                (&b""[..], ""),
                (&b"\0\x1b$G!\x0e\x0f"[..], "\0\u{1b}$G!\u{e}\u{f}"),
                (
                    &b"\xc2\xa2\xe0\xa0\x80\xed\x9f\xbf\xee\x80\x80\xef\xbf\xbf"[..],
                    "¢\u{800}\u{d7ff}\u{e000}\u{ffff}",
                ),
                (&b"\xed\xa0\x80\xed\xb0\x80"[..], "\u{10000}"),
                (&b"\xed\xaf\xbf\xed\xbf\xbf"[..], "\u{10ffff}"),
                (&b"\xef\xbb\xbf\xed\xa0\xbd\xed\xb8\x80"[..], "\u{feff}😀"),
                (
                    "日本語 \u{e6f1} 1\u{20e3}".as_bytes(),
                    "日本語 \u{e6f1} 1\u{20e3}",
                ),
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap().text,
                    expected,
                    "{name}/{bytes:?}"
                );
            }
            for bytes in [
                &b"\xff"[..],
                &b"\x80"[..],
                &b"\xc0\x80"[..],
                &b"\xc1\xbf"[..],
                &b"\xe0\x80\x80"[..],
                &b"\xe0\x9f\xbf"[..],
                &b"\xe1\x7f\x80"[..],
                &b"\xed\xa0\x80"[..],
                &b"\xed\xb0\x80"[..],
                &b"\xed\xa0\x80\xed\xa0\x80"[..],
                &b"\xed\xb0\x80\xed\xb0\x80"[..],
                &b"\xed\xb0\x80\xed\xa0\x80"[..],
                &b"\xed\xa0\x80A\xed\xb0\x80"[..],
                &b"\xed\xa0\x80\n\xed\xb0\x80"[..],
                &b"\xed\xa0\x80\xef\xbb\xbf\xed\xb0\x80"[..],
                &b"\xed\xa0\x80\xed\xb0"[..],
                &b"\xed\xa0"[..],
                &b"\xc2"[..],
                &b"\xf0\x90\x80\x80"[..],
                &b"\xf0\x9f\x98\x80"[..],
                &b"\xf4\x8f\xbf\xbf"[..],
                &b"\xf8\x88\x80\x80\x80"[..],
                &b"\xff\xfeA\0"[..],
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{bytes:?}"
                );
            }
        }
        assert!(decode(b"\xed\xa0\xbd\xed\xb8\x80", Some("UTF-8")).is_err());
        assert_eq!(decode("😀".as_bytes(), Some("UTF-8")).unwrap().text, "😀");
    }

    #[test]
    fn utf8_softbank_private_use_is_independent_from_sjis_and_preserves_unicode() {
        for name in ["UTF8-SoftBank", "utf8-softbank"] {
            for (input, expected) in [
                ("\u{e04a}\u{e50b}\u{e21c}\u{e14f}", "☀🇯🇵1\u{20e3}🅿"),
                (
                    "日本語 😀 🇯🇵 1\u{fe0f}\u{20e3}❤\u{fe0f}",
                    "日本語 😀 🇯🇵 1\u{fe0f}\u{20e3}❤\u{fe0f}",
                ),
                (
                    "\u{e000}\u{e53f}\u{fffe}\u{ffff}\u{10ffff}",
                    "\u{e000}\u{e53f}\u{fffe}\u{ffff}\u{10ffff}",
                ),
                ("\u{feff}\u{e04a}", "\u{feff}☀"),
                ("\u{1b}$G!\u{e}\u{f}", "\u{1b}$G!\u{e}\u{f}"),
            ] {
                assert_eq!(
                    decode(input.as_bytes(), Some(name)).unwrap().text,
                    expected,
                    "{name}/{input:?}"
                );
            }
            for input in ["\u{e001}", "\u{e059}", "\u{e53e}"] {
                assert_eq!(
                    decode(input.as_bytes(), Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding
                );
            }
            for input in [
                &b"\xff"[..],
                &b"\xed\xa0\x80"[..],
                &b"\xf4\x90\x80\x80"[..],
                &b"\xee\x94"[..],
                &b"\xff\xfeA\0"[..],
            ] {
                assert_eq!(
                    decode(input, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding
                );
            }
        }
        for name in ["UTF-8", "UTF8-DoCoMo", "UTF8-KDDI"] {
            assert_eq!(
                decode("\u{e21c}".as_bytes(), Some(name)).unwrap().text,
                "\u{e21c}"
            );
        }
        assert_eq!(
            decode(b"\xf2\xe5", Some("SJIS-SoftBank")).unwrap().text,
            "1\u{20e3}"
        );
        assert!(decode(b"\xf2\xe5", Some("UTF8-SoftBank")).is_err());
    }

    #[test]
    fn utf8_kddi_duplicate_banks_preserve_unicode_and_reject_undefined_values() {
        for name in ["UTF8-KDDI", "utf8-kddi"] {
            for (input, expected) in [
                ("\u{e488}\u{ef60}", "☀☀"),
                ("\u{e4cc}\u{efa5}", "🇯🇵🇯🇵"),
                ("\u{e522}\u{effb}", "1\u{20e3}1\u{20e3}"),
                ("\u{e4a6}\u{ef7e}", "🅿🅿"),
                (
                    "日本語 😀 ☀ 🇯🇵 1\u{fe0f}\u{20e3}",
                    "日本語 😀 ☀ 🇯🇵 1\u{fe0f}\u{20e3}",
                ),
                (
                    "\u{e000}\u{fffe}\u{ffff}\u{10ffff}",
                    "\u{e000}\u{fffe}\u{ffff}\u{10ffff}",
                ),
                ("\u{feff}\u{e488}", "\u{feff}☀"),
                ("\u{1b}$G!\u{e}\u{f}", "\u{1b}$G!\u{e}\u{f}"),
            ] {
                assert_eq!(
                    decode(input.as_bytes(), Some(name)).unwrap().text,
                    expected,
                    "{name}/{input:?}"
                );
            }
            for input in ["\u{e468}", "\u{e472}", "\u{f0fc}"] {
                assert_eq!(
                    decode(input.as_bytes(), Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding
                );
            }
            for input in [
                &b"\xff"[..],
                &b"\xed\xa0\x80"[..],
                &b"\xf4\x90\x80\x80"[..],
                &b"\xee\x93"[..],
                &b"\xff\xfeA\0"[..],
            ] {
                assert_eq!(
                    decode(input, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding
                );
            }
        }
        assert_eq!(
            decode("\u{e4cc}".as_bytes(), Some("UTF-8")).unwrap().text,
            "\u{e4cc}"
        );
        assert_eq!(
            decode("\u{e4cc}".as_bytes(), Some("UTF8-DoCoMo"))
                .unwrap()
                .text,
            "\u{e4cc}"
        );
    }

    #[test]
    fn utf8_docomo_maps_private_use_and_keeps_other_unicode_with_strict_validation() {
        for name in ["UTF8-DoCoMo", "utf8-docomo"] {
            for (input, expected) in [
                ("\u{e63e}\u{e6e2}\u{e66c}", "☀1\u{20e3}🅿"),
                (
                    "日本語 😀 🇯🇵 1\u{fe0f}\u{20e3}❤\u{fe0f}",
                    "日本語 😀 🇯🇵 1\u{fe0f}\u{20e3}❤\u{fe0f}",
                ),
                (
                    "\u{e000}\u{fffe}\u{ffff}\u{10ffff}",
                    "\u{e000}\u{fffe}\u{ffff}\u{10ffff}",
                ),
                ("\u{feff}\u{e63e}", "\u{feff}☀"),
                ("\u{1b}$G!\u{e}\u{f}", "\u{1b}$G!\u{e}\u{f}"),
            ] {
                assert_eq!(
                    decode(input.as_bytes(), Some(name)).unwrap().text,
                    expected,
                    "{name}/{input:?}"
                );
            }
            for input in ["\u{e643}", "\u{e6f1}", "\u{e757}"] {
                assert_eq!(
                    decode(input.as_bytes(), Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding
                );
            }
            for bytes in [
                &b"\xff"[..],
                &b"\x80"[..],
                &b"\xee\x9b"[..],
                &b"\xf0\x9f\x98"[..],
                &b"\xc0\xaf"[..],
                &b"\xe0\x80\x80"[..],
                &b"\xed\xa0\x80"[..],
                &b"\xf4\x90\x80\x80"[..],
                &b"\xf8\x88\x80\x80\x80"[..],
                &b"\xfc\x84\x80\x80\x80\x80"[..],
                &b"\xff\xfeA\0"[..],
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{bytes:?}"
                );
            }
        }
        assert_eq!(
            decode("\u{e6e2}".as_bytes(), Some("UTF-8")).unwrap().text,
            "\u{e6e2}"
        );
        assert!(decode(b"\xff\xfeA\0", Some("UTF-8")).is_ok());
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
            ("UTF8-DoCoMo", 0x4118e5f8e2a4ea5a, false),
            ("UTF8-KDDI", 0x469521cbfc82bb8, false),
            ("UTF8-SoftBank", 0x3938041392c7609b, false),
            ("CESU-8", 0x7536daf613d09f75, false),
            ("UTF8-MAC", 0xa8ccedc2cb405131, false),
            ("SJIS-SoftBank", 0x2f3af6a0c46dc354, false),
            ("SJIS-DoCoMo", 0x9e10ca6ad6283a3f, false),
            ("SJIS-KDDI", 0x6f033b26c39879f5, false),
            ("EUC-JIS-2004", 0xcce2b1a75dcf5e2a, true),
            ("CP949", 0x620f7b1e5bb833c1, false),
            ("stateless-ISO-2022-JP", 0x91fbafd49916f8e1, false),
            ("stateless-ISO-2022-JP-KDDI", 0x5aa46317606ed4c0, false),
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
            let prefixes: &[u8] = if euc {
                &[0x8f]
            } else if name.starts_with("stateless-ISO-2022-JP") {
                // An independent Ruby oracle checks 196,865 inputs, including
                // all invalid triples for both 1978/1983 prefixes.
                &[0x90, 0x92]
            } else {
                &[]
            };
            for &prefix in prefixes {
                for a in 0..=255 {
                    for b in 0..=255 {
                        check(&[prefix, a, b]);
                    }
                }
            }
            if name == "SJIS-SoftBank" {
                // All legacy-looking selector/payload pairs are still ordinary
                // SJIS input, with and without SI and at truncated EOFs.
                for a in 0..=255 {
                    check(&[27, 36, a]);
                }
                for a in 0..=255 {
                    for b in 0..=255 {
                        check(&[27, 36, a, b]);
                        check(&[27, 36, a, b, 15]);
                    }
                }
            }
            if name == "CESU-8" {
                // 2,495,258 independent Ruby IO probes, including every valid
                // surrogate pair and every forbidden native UTF-8 supplementary scalar.
                fn unit(value: u16) -> Vec<u8> {
                    match value {
                        0..=0x7f => vec![value as u8],
                        0x80..=0x7ff => vec![0xc0 | (value >> 6) as u8, 0x80 | (value & 63) as u8],
                        _ => vec![
                            0xe0 | (value >> 12) as u8,
                            0x80 | ((value >> 6) & 63) as u8,
                            0x80 | (value & 63) as u8,
                        ],
                    }
                }
                for value in 0..=u16::MAX {
                    check(&unit(value));
                }
                for high in 0xd800..=0xdbff {
                    for low in 0xdc00..=0xdfff {
                        check(&[unit(high), unit(low)].concat());
                    }
                }
                for value in 0x10000..=0x10ffff {
                    let mut bytes = [0; 4];
                    check(
                        char::from_u32(value)
                            .unwrap()
                            .encode_utf8(&mut bytes)
                            .as_bytes(),
                    );
                }
                for high in [0xd800, 0xdbff] {
                    for value in 0..=u16::MAX {
                        check(&[unit(high), unit(value)].concat());
                    }
                }
                for low in [0xdc00, 0xdfff] {
                    for value in 0..=u16::MAX {
                        check(&[unit(value), unit(low)].concat());
                    }
                }
                for base in [
                    b"\xed\xa0\x80\xed\xb0\x80",
                    b"\xed\xaf\xbf\xed\xbf\xbf",
                    b"\xed\xa0\xbd\xed\xb8\x80",
                ] {
                    for length in 0..base.len() {
                        check(&base[..length]);
                    }
                    for index in 0..base.len() {
                        for byte in 0..=255 {
                            let mut input = *base;
                            input[index] = byte;
                            check(&input);
                        }
                    }
                }
                for bytes in [
                    &b"\xf4\x90\x80\x80"[..],
                    &b"\xf8\x88\x80\x80\x80"[..],
                    &b"\xfc\x84\x80\x80\x80\x80"[..],
                    &b"\xed\xa0\x80A\xed\xb0\x80"[..],
                    &b"\xed\xb0\x80\xed\xa0\x80"[..],
                    &b"\xed\xa0\x80\xed\xa0\x80"[..],
                    &b"\xed\xb0\x80\xed\xb0\x80"[..],
                ] {
                    check(bytes);
                }
            }
            if matches!(
                name,
                "UTF8-DoCoMo" | "UTF8-KDDI" | "UTF8-SoftBank" | "UTF8-MAC"
            ) {
                // 1,183,513 independent IO probes: every Unicode scalar,
                // short byte inputs, UTF-8 boundary mutations and truncations.
                for scalar in 0..=0x10ffff {
                    if let Some(character) = char::from_u32(scalar) {
                        let mut buffer = [0; 4];
                        check(character.encode_utf8(&mut buffer).as_bytes());
                    }
                }
                for base in [
                    &b"\xc2\xa2"[..],
                    &b"\xe0\xa0\x80"[..],
                    &b"\xed\x9f\xbf"[..],
                    &b"\xee\x98\xbe"[..],
                    &b"\xee\x9b\xb1"[..],
                    &b"\xf0\x90\x80\x80"[..],
                    &b"\xf4\x8f\xbf\xbf"[..],
                ] {
                    for length in 0..base.len() {
                        check(&base[..length]);
                    }
                    for index in 0..base.len() {
                        for byte in 0..=255 {
                            let mut input = base.to_vec();
                            input[index] = byte;
                            check(&input);
                        }
                    }
                }
                check(b"\xf8\x88\x80\x80\x80");
                check(b"\xfc\x84\x80\x80\x80\x80");
            }
            if name == "UTF8-MAC" {
                for &probe in &tables::UTF8_MAC_PROBES {
                    for input in [
                        probe.to_owned(),
                        format!("x{probe}"),
                        format!("{probe}y"),
                        format!("{probe}\u{301}"),
                        format!("{probe}\u{323}"),
                        probe.chars().rev().collect(),
                    ] {
                        check(input.as_bytes());
                    }
                }
                for leading in 0x1100..=0x1112 {
                    for vowel in 0x1161..=0x1175 {
                        let prefix = format!(
                            "{}{}",
                            char::from_u32(leading).unwrap(),
                            char::from_u32(vowel).unwrap()
                        );
                        check(prefix.as_bytes());
                        for trailing in 0x11a8..=0x11c2 {
                            check(
                                format!("{prefix}{}", char::from_u32(trailing).unwrap()).as_bytes(),
                            );
                            let syllable = 0xac00 + ((leading - 0x1100) * 21 + vowel - 0x1161) * 28;
                            check(
                                format!(
                                    "{}{}",
                                    char::from_u32(syllable).unwrap(),
                                    char::from_u32(trailing).unwrap()
                                )
                                .as_bytes(),
                            );
                        }
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
        assert_eq!(
            decode(b"ASCII", Some("UTF-7")).unwrap_err().kind,
            ResourceErrorKind::InvalidEncoding
        );
    }

    #[test]
    fn cp5022_variants_decode_katakana_shifts_and_vendor_mappings() {
        for name in ["CP50220", "CP50221", "cp50220", "cP50221"] {
            for (bytes, expected) in [
                (&b"\x1b$B!@!A-!y!|~\x1b(B"[..], "＼～①纊＂"),
                (&b"\x1b$@!A\x1b$B-!\x1b(B"[..], "～①"),
                (&b"\x1b$B-!\x1b$@!A\x1b(B"[..], "①～"),
                (&b"\x1b(I1^\x1b(B"[..], "ｱﾞ"),
                (&b"\x0e1^\x0f"[..], "ｱﾞ"),
                (&b"\xb1\xde"[..], "ｱﾞ"),
                (&b"\x1b(I\xb1\xde"[..], "ｱﾞ"),
                (&b"\x1b$B$\"\x0e!\x0f$\""[..], "あ｡$\""),
                (&b"\x1b$B\x0e!_\x0f"[..], "｡ﾟ"),
                (&b"\x0e!\x1b$B$\"\x1b(J\\~"[..], "｡あ\\~"),
                (&b"\x0e!\x0e!\x0f\x0fA"[..], "｡｡A"),
                (&b"\xb1A\x1b(I\xb1!\x0fZ"[..], "ｱAｱ｡Z"),
                (&b"\x1b(I!_"[..], "｡ﾟ"),
                (&b"\x1b$B\x0e\x0f\x1b(I\x1b(B"[..], ""),
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
                &b"\x1b$B$\x0e"[..],
                &b"\x1b$B$\x0f"[..],
                &b"\x1b$B$\x1b(B"[..],
                &b"\x1b$(D"[..],
                &b"\x1b&@\x1b$B"[..],
                &b"\x1b$B~~"[..],
                &b"\x1b$@-!"[..],
                &b"\x1b$@y!"[..],
                &b"\x1b$B-!\x1b$@-!"[..],
                &b"\x80"[..],
                &b"\xa0"[..],
                &b"\xe0"[..],
                &b"\xe0\xe0"[..],
                &b"\x8e\xb1"[..],
                &b"\x1b$B\xb1\xb1"[..],
                &b"\x1b$B$\"\n"[..],
                &b"\x1b(I1\n"[..],
                &b"\x1b(I "[..],
                &b"\x1b(I\x7f"[..],
                &b"\x1b(I`"[..],
            ] {
                assert_eq!(
                    decode(bytes, Some(name)).unwrap_err().kind,
                    ResourceErrorKind::InvalidEncoding,
                    "{name}/{bytes:?}"
                );
            }
        }
    }

    #[test]
    fn iso2022_jp_matches_exhaustive_ruby_state_fingerprint() {
        // Ruby 3.3.8, generate_encodings.rb --fingerprints. ISO: 656,645 inputs
        // in five designations; each CP variant: 1,575,948 inputs in twelve
        // designation/shift combinations. All bytes/pairs/escape candidates.
        for (name, expected) in [
            ("ISO-2022-JP", 0xfdd8ceaeaea5a195_u64),
            ("CP50220", 0x3b6e1f9768937b18),
            ("CP50221", 0x3b6e1f9768937b18),
            ("ISO-2022-JP-KDDI", 0x2c1e45c319c2b453),
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
            let mut prefixes = vec![
                &b""[..],
                &b"\x1b$B"[..],
                &b"\x1b$@"[..],
                &b"\x1b(B"[..],
                &b"\x1b(J"[..],
            ];
            if name.starts_with("CP5022") {
                prefixes.extend([
                    &b"\x1b(I"[..],
                    &b"\x0e"[..],
                    &b"\x0f"[..],
                    &b"\x1b$B\x0e"[..],
                    &b"\x1b$B\x0f"[..],
                    &b"\x1b(I\x0f"[..],
                    &b"\x1b(J\x0e"[..],
                ]);
            }
            for prefix in prefixes {
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
            assert_eq!(hash, expected, "{name}");
        }
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
