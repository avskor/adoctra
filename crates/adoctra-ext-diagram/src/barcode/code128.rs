//! barby's `Code128` (and `Code128A`/`B`/`C`, and asciidoctor-diagram's
//! GS1-128 on top of it): the shortest encoding, nested segments after a
//! code change, the checksum.
//!
//! Symbols are ASCII bytes, with barby's control bytes (`\xC1`–`\xCB`) as
//! `0x100 +` byte. barby matches its data with binary (`/n`) regular
//! expressions, which Ruby refuses on a UTF-8 string with other characters.

use super::barby::Failure;

const FNC1: u16 = 0x1c1;
const FNC2: u16 = 0x1c2;
const FNC3: u16 = 0x1c3;
const FNC4: u16 = 0x1c4;
const CODEA: u16 = 0x1c5;
const CODEB: u16 = 0x1c6;
const CODEC: u16 = 0x1c7;
const SHIFT: u16 = 0x1c8;
const STARTA: u16 = 0x1c9;
const STARTB: u16 = 0x1ca;
const STARTC: u16 = 0x1cb;

const ENCODINGS: [&str; 106] = [
    "11011001100",
    "11001101100",
    "11001100110",
    "10010011000",
    "10010001100",
    "10001001100",
    "10011001000",
    "10011000100",
    "10001100100",
    "11001001000",
    "11001000100",
    "11000100100",
    "10110011100",
    "10011011100",
    "10011001110",
    "10111001100",
    "10011101100",
    "10011100110",
    "11001110010",
    "11001011100",
    "11001001110",
    "11011100100",
    "11001110100",
    "11101101110",
    "11101001100",
    "11100101100",
    "11100100110",
    "11101100100",
    "11100110100",
    "11100110010",
    "11011011000",
    "11011000110",
    "11000110110",
    "10100011000",
    "10001011000",
    "10001000110",
    "10110001000",
    "10001101000",
    "10001100010",
    "11010001000",
    "11000101000",
    "11000100010",
    "10110111000",
    "10110001110",
    "10001101110",
    "10111011000",
    "10111000110",
    "10001110110",
    "11101110110",
    "11010001110",
    "11000101110",
    "11011101000",
    "11011100010",
    "11011101110",
    "11101011000",
    "11101000110",
    "11100010110",
    "11101101000",
    "11101100010",
    "11100011010",
    "11101111010",
    "11001000010",
    "11110001010",
    "10100110000",
    "10100001100",
    "10010110000",
    "10010000110",
    "10000101100",
    "10000100110",
    "10110010000",
    "10110000100",
    "10011010000",
    "10011000010",
    "10000110100",
    "10000110010",
    "11000010010",
    "11001010000",
    "11110111010",
    "11000010100",
    "10001111010",
    "10100111100",
    "10010111100",
    "10010011110",
    "10111100100",
    "10011110100",
    "10011110010",
    "11110100100",
    "11110010100",
    "11110010010",
    "11011011110",
    "11011110110",
    "11110110110",
    "10101111000",
    "10100011110",
    "10001011110",
    "10111101000",
    "10111100010",
    "11110101000",
    "11110100010",
    "10111011110",
    "10111101110",
    "11101011110",
    "11110101110",
    "11010000100",
    "11010010000",
    "11010011100",
];

/// A character of a segment: one symbol, or two digits in code set C.
type Character = Vec<u16>;

/// `VALUES[type][character]`.
fn value(kind: char, c: &[u16]) -> Option<usize> {
    let shared = |s: u16| match s {
        FNC3 => Some(96),
        FNC2 => Some(97),
        SHIFT => Some(98),
        FNC1 => Some(102),
        STARTA => Some(103),
        STARTB => Some(104),
        STARTC => Some(105),
        _ => None,
    };
    match (kind, c) {
        ('A', [s]) => match *s {
            32..=95 => Some(*s as usize - 32),
            0..=31 => Some(*s as usize + 64),
            CODEC => Some(99),
            CODEB => Some(100),
            FNC4 => Some(101),
            s => shared(s),
        },
        ('B', [s]) => match *s {
            32..=127 => Some(*s as usize - 32),
            CODEC => Some(99),
            FNC4 => Some(100),
            CODEA => Some(101),
            s => shared(s),
        },
        ('C', [a, b])
            if (*a as u8).is_ascii_digit()
                && *a < 0x80
                && (*b as u8).is_ascii_digit()
                && *b < 0x80 =>
        {
            Some(((*a - 48) * 10 + (*b - 48)) as usize)
        }
        ('C', [s]) => match *s {
            CODEB => Some(100),
            CODEA => Some(101),
            FNC1 => Some(102),
            STARTA => Some(103),
            STARTB => Some(104),
            STARTC => Some(105),
            _ => None,
        },
        _ => None,
    }
}

fn is_digit(s: u16) -> bool {
    s < 0x80 && (s as u8).is_ascii_digit()
}
fn is_control(s: u16) -> bool {
    s < 32
}
fn is_lower(s: u16) -> bool {
    s < 0x80 && (s as u8).is_ascii_lowercase()
}

/// A segment: its code set, data and the segment after a code change.
struct Segment {
    kind: char,
    data: Vec<u16>,
    extra: Option<Box<Segment>>,
}

impl Segment {
    /// `Code128#initialize` with a type: `data=` splits at the first code
    /// change (the rest becomes `extra`, created recursively), then the
    /// data must be valid.
    fn new(kind: char, data: &[u16]) -> Result<Self, Failure> {
        let split = data
            .iter()
            .position(|s| matches!(*s, CODEA | CODEB | CODEC));
        let (own, extra) = match split {
            Some(at) => (&data[..at], Some(&data[at..])),
            None => (data, None),
        };
        // `split` drops a trailing empty part, so a final code change
        // starts an empty segment.
        let extra = match extra {
            Some(extra) => {
                let kind = match extra[0] {
                    CODEA => 'A',
                    CODEB => 'B',
                    _ => 'C',
                };
                Some(Box::new(Segment::new(kind, &extra[1..])?))
            }
            None => None,
        };
        let segment = Segment {
            kind,
            data: own.to_vec(),
            extra,
        };
        let characters = segment.characters();
        if characters.is_empty() || characters.iter().any(|c| value(kind, c).is_none()) {
            return Err(Failure::Invalid);
        }
        Ok(segment)
    }

    /// `characters`: symbols, digit pairs in code set C.
    fn characters(&self) -> Vec<Character> {
        if self.kind != 'C' {
            return self.data.iter().map(|s| vec![*s]).collect();
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.data.len() {
            if is_digit(self.data[i]) {
                out.push(self.data[i..(i + 2).min(self.data.len())].to_vec());
                i += 2;
            } else {
                out.push(vec![self.data[i]]);
                i += 1;
            }
        }
        out
    }

    fn numbers(&self) -> Vec<Option<usize>> {
        self.characters()
            .iter()
            .map(|c| value(self.kind, c))
            .collect()
    }

    fn change_code(&self) -> u16 {
        match self.kind {
            'A' => CODEA,
            'B' => CODEB,
            _ => CODEC,
        }
    }

    fn data_encoding(&self) -> String {
        self.numbers()
            .iter()
            .filter_map(|n| n.map(|n| ENCODINGS[n]))
            .collect()
    }

    /// `extra_encoding`: the change code (in this set) and the segment.
    fn extra_encoding(&self) -> Result<String, String> {
        let Some(extra) = &self.extra else {
            return Ok(String::new());
        };
        let code =
            value(self.kind, &[extra.change_code()]).ok_or("undefined method `+' for nil")?;
        Ok(format!(
            "{}{}{}",
            ENCODINGS[code],
            extra.data_encoding(),
            extra.extra_encoding()?
        ))
    }

    fn extra_numbers(&self) -> Vec<Option<usize>> {
        let Some(extra) = &self.extra else {
            return Vec::new();
        };
        let mut numbers = vec![value(self.kind, &[extra.change_code()])];
        numbers.extend(extra.numbers());
        numbers.extend(extra.extra_numbers());
        numbers
    }

    fn encoding(&self) -> Result<String, String> {
        let start = match self.kind {
            'A' => 103,
            'B' => 104,
            _ => 105,
        };
        let mut out = format!(
            "{}{}{}",
            ENCODINGS[start],
            self.data_encoding(),
            self.extra_encoding()?
        );
        let mut sum = start;
        for (pos, number) in self
            .numbers()
            .into_iter()
            .chain(self.extra_numbers())
            .enumerate()
        {
            sum += number.ok_or("undefined method `*' for nil")? * (pos + 1);
        }
        out.push_str(ENCODINGS[sum % 103]);
        out.push_str("1100011101011");
        Ok(out)
    }
}

/// The data as symbols; any non-ASCII character fails like Ruby's binary
/// regular expression match.
fn symbols(data: &str) -> Result<Vec<u16>, Failure> {
    if !data.is_ascii() {
        return Err(Failure::Message(
            "incompatible encoding regexp match (ASCII-8BIT regexp with UTF-8 string)".to_owned(),
        ));
    }
    Ok(data.bytes().map(u16::from).collect())
}

/// `Code128.new(data)` (best type) or `Code128A/B/C.new(data)`.
pub(crate) fn encode(data: &str, kind: Option<char>) -> Result<String, Failure> {
    let symbols = symbols(data)?;
    segment(symbols, kind)?.encoding().map_err(Failure::Message)
}

fn segment(symbols: Vec<u16>, kind: Option<char>) -> Result<Segment, Failure> {
    match kind {
        Some(kind) => Segment::new(kind, &symbols),
        None => {
            let mut best = shortest_encoding(&symbols);
            if best.is_empty() {
                return Err(Failure::Message(
                    "undefined method `upcase' for nil".to_owned(),
                ));
            }
            let kind = match best.remove(0) {
                CODEA => 'A',
                CODEB => 'B',
                _ => 'C',
            };
            Segment::new(kind, &best)
        }
    }
}

/// `CODEC_CHARS_RE` (`(?:\d{2}|FNC1){2,}`) at `at`: the end of each
/// count of units from the longest down to two.
fn codec_units(data: &[u16], at: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut i = at;
    loop {
        if i + 1 < data.len() && is_digit(data[i]) && is_digit(data[i + 1]) {
            i += 2;
        } else if i < data.len() && data[i] == FNC1 {
            i += 1;
        } else {
            break;
        }
        ends.push(i);
    }
    if ends.len() < 2 {
        return Vec::new();
    }
    ends.drain(..1);
    ends.reverse();
    ends
}

/// `extract_codec`: split at `(^CODEC)|(CODEC(?!\d))`, keeping the matches,
/// without empty parts.
fn extract_codec(data: &[u16]) -> Vec<Vec<u16>> {
    let mut parts = Vec::new();
    let mut last = 0;
    let mut at = 0;
    while at < data.len() {
        let line_start = at == 0 || data[at - 1] == u16::from(b'\n');
        let ends = codec_units(data, at);
        let found = if line_start {
            ends.first().copied()
        } else {
            ends.into_iter()
                .find(|end| !data.get(*end).is_some_and(|s| is_digit(*s)))
        };
        match found {
            Some(end) => {
                parts.push(data[last..at].to_vec());
                parts.push(data[at..end].to_vec());
                last = end;
                at = end;
            }
            None => at += 1,
        }
    }
    parts.push(data[last..].to_vec());
    parts.retain(|part| !part.is_empty());
    parts
}

/// `codec_segment?`: `\A(?:\d{2}|FNC1){2,}\Z` (a final line feed allowed).
fn codec_segment(block: &[u16]) -> bool {
    let block = match block.split_last() {
        Some((last, rest)) if *last == u16::from(b'\n') => rest,
        _ => block,
    };
    codec_units(block, 0).first() == Some(&block.len())
}

fn control_before_lowercase(data: &[u16]) -> bool {
    match data.iter().position(|s| is_control(*s)) {
        Some(ctrl) => data
            .iter()
            .position(|s| is_lower(*s))
            .is_none_or(|lower| ctrl < lower),
        None => false,
    }
}

fn handle_code_a(data: &[u16]) -> Vec<u16> {
    let mut out = vec![CODEA];
    for (i, c) in data.iter().enumerate() {
        if is_lower(*c) {
            if control_before_lowercase(&data[i + 1..]) {
                out.extend([SHIFT, *c]);
            } else {
                out.extend(handle_code_b(&data[i..]));
                break;
            }
        } else {
            out.push(*c);
        }
    }
    out
}

fn handle_code_b(data: &[u16]) -> Vec<u16> {
    let mut out = vec![CODEB];
    for (i, c) in data.iter().enumerate() {
        if is_control(*c) {
            if control_before_lowercase(&data[i + 1..]) {
                out.extend(handle_code_a(&data[i..]));
                break;
            } else {
                out.extend([SHIFT, *c]);
            }
        } else {
            out.push(*c);
        }
    }
    out
}

/// `apply_shortest_encoding_for_data`.
fn shortest_encoding(data: &[u16]) -> Vec<u16> {
    let mut out = Vec::new();
    for block in extract_codec(data) {
        if codec_segment(&block) {
            out.push(CODEC);
            out.extend(block);
        } else if control_before_lowercase(&block) {
            out.extend(handle_code_a(&block));
        } else {
            out.extend(handle_code_b(&block));
        }
    }
    out
}

/// asciidoctor-diagram's `gs1_128`: `(FNC1)`… placeholders become control
/// characters, whitespace is removed, an initial FNC1 is ensured, then
/// `Code128.new`.
pub(crate) fn gs1_128(code: &str) -> Result<String, Failure> {
    // `code.gsub(/\([^)]+\)/) { … }`.
    let mut symbols: Vec<u16> = Vec::new();
    let mut replaced = false;
    let mut rest = code;
    while let Some(open) = rest.find('(') {
        let after = &rest[open + 1..];
        match after.find(')').filter(|close| *close > 0) {
            Some(close) => {
                push_text(&mut symbols, &rest[..open]);
                let control = &rest[open..open + close + 2];
                let replacement = match control.to_uppercase().as_str() {
                    "(FNC1)" => Some(FNC1),
                    "(FNC2)" => Some(FNC2),
                    "(FNC3)" => Some(FNC3),
                    "(FNC4)" => Some(FNC4),
                    "(CODEA)" => Some(CODEA),
                    "(CODEB)" => Some(CODEB),
                    "(CODEC)" => Some(CODEC),
                    "(SHIFT)" => Some(SHIFT),
                    "(SP)" => Some(u16::from(b' ')),
                    _ => None,
                };
                match replacement {
                    Some(symbol) => {
                        replaced |= symbol >= 0x100;
                        symbols.push(symbol);
                    }
                    None => push_text(&mut symbols, control),
                }
                rest = &rest[open + close + 2..];
            }
            None => {
                push_text(&mut symbols, &rest[..open + 1]);
                rest = after;
            }
        }
    }
    push_text(&mut symbols, rest);
    if !code.is_ascii() {
        // Binary control characters cannot join UTF-8 text.
        return Err(Failure::Message(
            if replaced {
                "incompatible character encodings: ASCII-8BIT and UTF-8"
            } else {
                "incompatible character encodings: UTF-8 and ASCII-8BIT"
            }
            .to_owned(),
        ));
    }
    // `gsub(/\s+/, '')`.
    symbols.retain(|s| !matches!(*s, 0x20 | 0x09..=0x0d));
    if symbols.first() != Some(&FNC1) {
        symbols.insert(0, FNC1);
    }
    segment(symbols, None)?.encoding().map_err(Failure::Message)
}

fn push_text(symbols: &mut Vec<u16>, text: &str) {
    symbols.extend(text.bytes().map(u16::from));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings_match_barby() {
        // From Ruby (barby 0.6.9).
        assert_eq!(
            encode("abc", None),
            Ok("11010010000100101100001001000011010000101100110111101101100011101011".to_owned())
        );
        assert_eq!(encode("a\tb", None), Err(Failure::Invalid));
        assert_eq!(encode("abc", Some('A')), Err(Failure::Invalid));
        assert_eq!(encode("123", Some('C')), Err(Failure::Invalid));
        assert_eq!(
            encode("", None),
            Err(Failure::Message(
                "undefined method `upcase' for nil".to_owned()
            ))
        );
        assert_eq!(
            encode("é", None),
            Err(Failure::Message(
                "incompatible encoding regexp match (ASCII-8BIT regexp with UTF-8 string)"
                    .to_owned()
            ))
        );
    }

    #[test]
    fn codec_runs_split_like_rubys_regexp() {
        let s = |t: &str| t.bytes().map(u16::from).collect::<Vec<_>>();
        assert_eq!(extract_codec(&s("12345")), [s("1234"), s("5")]);
        assert_eq!(extract_codec(&s("ab12345")), [s("ab1"), s("2345")]);
        assert_eq!(extract_codec(&s("ab12")), [s("ab12")]);
        assert!(codec_segment(&s("1234\n")));
        assert!(!codec_segment(&s("12")));
    }
}
