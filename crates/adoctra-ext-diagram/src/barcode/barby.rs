//! The barcode symbologies of barby 0.6.9 that asciidoctor-diagram
//! registers: each turns the data into barby's `encoding` (`1` bar, `0`
//! space; one string per row for QR), failing where and how barby fails.

use super::code128;

/// A failure: barby's `ArgumentError` while the barcode is created (which
/// asciidoctor-diagram reports as `Invalid <type> data`), or any other
/// exception (reported with its message).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Invalid,
    Message(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Message(message)
    }
}

/// A barcode's modules.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// A one-dimensional barcode.
    Linear(String),
    /// A two-dimensional one (QR), row by row.
    Matrix(Vec<String>),
}

/// Ruby's `/^…$/` against a string: some line (a part between line feeds;
/// not the empty rest after a final line feed) satisfies `line`.
pub(crate) fn some_line(data: &str, line: impl Fn(&str) -> bool) -> bool {
    let mut parts: Vec<&str> = data.split('\n').collect();
    if parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop();
    }
    parts.into_iter().any(line)
}

fn all_digits(text: &str, count: Option<usize>) -> bool {
    text.bytes().all(|b| b.is_ascii_digit()) && count.is_none_or(|n| text.len() == n)
}

/// `String#to_i` of one character.
fn digit(c: char) -> u32 {
    c.to_digit(10).filter(|_| c.is_ascii_digit()).unwrap_or(0)
}

/// Create the barcode of `kind` (asciidoctor-diagram's type name) for
/// `data` and return its encoding.
pub(crate) fn encode(kind: &str, data: &str) -> Result<Encoding, Failure> {
    let linear = |encoding: Result<String, Failure>| encoding.map(Encoding::Linear);
    match kind {
        "bookland" => linear(bookland(data)),
        "codabar" => linear(codabar(data)),
        "code25" => linear(code25(data, Code25Kind::Standard)),
        "code25iata" => linear(code25(data, Code25Kind::Iata)),
        "code25interleaved" => linear(code25(data, Code25Kind::Interleaved)),
        "code39" => linear(code39(data)),
        "code93" => linear(code93(data)),
        "code128" => linear(code128::encode(data, None)),
        "code128a" => linear(code128::encode(data, Some('A'))),
        "code128b" => linear(code128::encode(data, Some('B'))),
        "code128c" => linear(code128::encode(data, Some('C'))),
        "ean8" => linear(ean(data, Ean::Ean8)),
        "ean13" => linear(ean(data, Ean::Ean13)),
        "gs1_128" => linear(code128::gs1_128(data)),
        "qrcode" => super::qr::barby_qr(data).map(Encoding::Matrix),
        "upca" => linear(ean(data, Ean::UpcA)),
        other => Err(Failure::Message(format!(
            "Unsupported barcode type: {other}"
        ))),
    }
}

const LEFT_ODD: [&str; 10] = [
    "0001101", "0011001", "0010011", "0111101", "0100011", "0110001", "0101111", "0111011",
    "0110111", "0001011",
];
const LEFT_EVEN: [&str; 10] = [
    "0100111", "0110011", "0011011", "0100001", "0011101", "0111001", "0000101", "0010001",
    "0001001", "0010111",
];
const RIGHT: [&str; 10] = [
    "1110010", "1100110", "1101100", "1000010", "1011100", "1001110", "1010000", "1000100",
    "1001000", "1110100",
];
/// `LEFT_PARITY_MAPS`: `true` is odd.
const PARITY: [[bool; 6]; 10] = [
    [true, true, true, true, true, true],
    [true, true, false, true, false, false],
    [true, true, false, false, true, false],
    [true, true, false, false, false, true],
    [true, false, true, true, false, false],
    [true, false, false, true, true, false],
    [true, false, false, false, true, true],
    [true, false, true, false, true, false],
    [true, false, true, false, false, true],
    [true, false, false, true, false, true],
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ean {
    Ean13,
    Ean8,
    UpcA,
}

/// `EAN13#checksum`: alternate digits from the right weigh 3.
fn ean_checksum(numbers: &[u32]) -> u32 {
    let sum: u32 = numbers
        .iter()
        .rev()
        .enumerate()
        .map(|(i, n)| if i % 2 == 0 { n * 3 } else { *n })
        .sum();
    let rest = sum % 10;
    if rest == 0 {
        0
    } else {
        10 - rest
    }
}

/// `EAN13`, `EAN8` and `UPCA` (`'0' + data`).
fn ean(data: &str, kind: Ean) -> Result<String, Failure> {
    let data = match kind {
        Ean::UpcA => format!("0{data}"),
        _ => data.to_owned(),
    };
    let length = if kind == Ean::Ean8 { 7 } else { 12 };
    if !some_line(&data, |line| all_digits(line, Some(length))) {
        return Err(Failure::Invalid);
    }
    Ok(ean_encoding(&data, kind == Ean::Ean8))
}

fn ean_encoding(data: &str, eight: bool) -> String {
    let numbers: Vec<u32> = data.chars().map(digit).collect();
    let mut with_checksum = numbers.clone();
    with_checksum.push(ean_checksum(&numbers));
    let slice = |list: &[u32], start: usize, len: usize| -> Vec<u32> {
        list.iter().skip(start).take(len).copied().collect()
    };
    let (left, right, parity): (Vec<u32>, Vec<u32>, Vec<bool>) = if eight {
        (
            slice(&numbers, 0, 4),
            slice(&with_checksum, 4, 4),
            vec![true; 4],
        )
    } else {
        (
            slice(&numbers, 1, 6),
            slice(&with_checksum, 7, 6),
            PARITY[numbers.first().copied().unwrap_or(0) as usize].to_vec(),
        )
    };
    let mut out = String::from("101");
    for (odd, n) in parity.iter().zip(&left) {
        out.push_str(if *odd { LEFT_ODD } else { LEFT_EVEN }[*n as usize]);
    }
    out.push_str("01010");
    for n in &right {
        out.push_str(RIGHT[*n as usize]);
    }
    out.push_str("101");
    out
}

/// `Bookland`: the ISBN's prefix (978 by default) and nine digits as an
/// EAN-13.
fn bookland(isbn: &str) -> Result<String, Failure> {
    // `isbn.gsub(/\D/, '')` against `\A(\d\d\d)?(\d{9})(\d)?\Z`.
    let digits: String = isbn.chars().filter(char::is_ascii_digit).collect();
    let (prefix, number) = match digits.len() {
        12 | 13 => (&digits[..3], &digits[3..12]),
        9 | 10 => ("978", &digits[..9]),
        _ => return Err(Failure::Invalid),
    };
    ean(&format!("{prefix}{number}"), Ean::Ean13)
}

const CODABAR_CHARACTERS: &str = "0123456789-$:/.+ABCD";
const CODABAR_BINARY: [&str; 20] = [
    "0000011", "0000110", "0001001", "1100000", "0010010", "1000010", "0100001", "0100100",
    "0110000", "1001000", "0001100", "0011000", "1000101", "1010001", "1010100", "0010101",
    "0011010", "0101001", "0001011", "0001110",
];

/// `Codabar`: `\A[ABCD][0-9\-$:/.+]+[ABCD]\z`.
fn codabar(data: &str) -> Result<String, Failure> {
    let chars: Vec<char> = data.chars().collect();
    let edge = |c: char| "ABCD".contains(c);
    let valid = chars.len() >= 3
        && edge(chars[0])
        && edge(chars[chars.len() - 1])
        && chars[1..chars.len() - 1]
            .iter()
            .all(|c| "0123456789-$:/.+".contains(*c));
    if !valid {
        return Err(Failure::Invalid);
    }
    let encoded: Vec<String> = chars
        .iter()
        .map(|c| {
            let index = CODABAR_CHARACTERS.find(*c).unwrap_or(0);
            CODABAR_BINARY[index]
                .chars()
                .enumerate()
                .map(|(i, bit)| match (i % 2 == 0, bit == '0') {
                    (true, true) => "1".to_owned(),
                    (true, false) => "111".to_owned(),
                    (false, true) => "00".to_owned(),
                    (false, false) => "000000".to_owned(),
                })
                .collect()
        })
        .collect();
    Ok(encoded.join("00000"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Code25Kind {
    Standard,
    Iata,
    Interleaved,
}

/// `Code25::ENCODINGS` (wide is `true`).
const CODE25: [[bool; 5]; 10] = [
    [false, false, true, true, false],
    [true, false, false, false, true],
    [false, true, false, false, true],
    [true, true, false, false, false],
    [false, false, true, false, true],
    [true, false, true, false, false],
    [false, true, true, false, false],
    [false, false, false, true, true],
    [true, false, false, true, false],
    [false, true, false, true, false],
];

/// `encoding_for_bars`: each bar (wide `111`, narrow `1`) and a space.
fn code25_bars(bars: &[bool]) -> String {
    bars.iter()
        .map(|wide| if *wide { "1110" } else { "10" })
        .collect()
}

/// `encoding_for_interleaved`: alternating bars and spaces, wide 3.
fn interleaved(widths: &[bool]) -> String {
    widths
        .iter()
        .enumerate()
        .map(|(i, wide)| {
            let c = if i % 2 == 0 { '1' } else { '0' };
            c.to_string().repeat(if *wide { 3 } else { 1 })
        })
        .collect()
}

/// `Code25`, `Code25IATA`, `Code25Interleaved`: the data is checked only
/// when encoding (an `ArgumentError` the converter does not catch).
fn code25(data: &str, kind: Code25Kind) -> Result<String, Failure> {
    let digits: Vec<usize> = data.chars().map(|c| digit(c) as usize).collect();
    let valid = some_line(data, |line| all_digits(line, None))
        && (kind != Code25Kind::Interleaved || digits.len().is_multiple_of(2));
    if !valid {
        return Err(Failure::Message("data not valid".to_owned()));
    }
    let strip_end_space = |text: String| text.trim_end_matches('0').to_owned();
    Ok(match kind {
        Code25Kind::Standard | Code25Kind::Iata => {
            let (start, stop): (&[bool], &[bool]) = if kind == Code25Kind::Standard {
                (&[true, true, false], &[true, false, true])
            } else {
                (&[false, false], &[true, false])
            };
            let mut out = code25_bars(start);
            for d in &digits {
                out.push_str(&code25_bars(&CODE25[*d]));
            }
            out + &strip_end_space(code25_bars(stop))
        }
        Code25Kind::Interleaved => {
            let mut out = interleaved(&[false, false, false, false]);
            for pair in digits.chunks(2) {
                let zipped: Vec<bool> = CODE25[pair[0]]
                    .iter()
                    .zip(&CODE25[pair[pair.len() - 1]])
                    .flat_map(|(bar, space)| [*bar, *space])
                    .collect();
                out.push_str(&interleaved(&zipped));
            }
            out + &interleaved(&[true, false, false])
        }
    })
}

const CODE39_CHARACTERS: &str = " $%+-./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
/// `Code39::ENCODINGS` in `CODE39_CHARACTERS` order, `W` wide.
const CODE39: [&str; 43] = [
    "NWWNNNWNN",
    "NWNWNWNNN",
    "NNNWNWNWN",
    "NWNNNWNWN",
    "NWNNNNWNW",
    "WWNNNNWNN",
    "NWNWNNNWN",
    "NNNWWNWNN",
    "WNNWNNNNW",
    "NNWWNNNNW",
    "WNWWNNNNN",
    "NNNWWNNNW",
    "WNNWWNNNN",
    "NNWWWNNNN",
    "NNNWNNWNW",
    "WNNWNNWNN",
    "NNWWNNWNN",
    "WNNNNWNNW",
    "NNWNNWNNW",
    "WNWNNWNNN",
    "NNNNWWNNW",
    "WNNNWWNNN",
    "NNWNWWNNN",
    "NNNNNWWNW",
    "WNNNNWWNN",
    "NNWNNWWNN",
    "NNNNWWWNN",
    "WNNNNNNWW",
    "NNWNNNNWW",
    "WNWNNNNWN",
    "NNNNWNNWW",
    "WNNNWNNWN",
    "NNWNWNNWN",
    "NNNNNNWWW",
    "WNNNNNWWN",
    "NNWNNNWWN",
    "NNNNWNWWN",
    "WWNNNNNNW",
    "NWWNNNNNW",
    "WWWNNNNNN",
    "NWNNWNNNW",
    "WWNNWNNNN",
    "NWWNWNNNN",
];

fn code39_bars(pattern: &str) -> String {
    pattern
        .chars()
        .enumerate()
        .map(|(i, w)| {
            let c = if i % 2 == 0 { '1' } else { '0' };
            c.to_string().repeat(if w == 'W' { 2 } else { 1 })
        })
        .collect()
}

/// `Code39` (not extended, no checksum).
fn code39(data: &str) -> Result<String, Failure> {
    let indexes: Option<Vec<usize>> = data.chars().map(|c| CODE39_CHARACTERS.find(c)).collect();
    let indexes = indexes.ok_or(Failure::Invalid)?;
    let star = code39_bars("NWNNWNWNN");
    let characters: Vec<String> = indexes.iter().map(|i| code39_bars(CODE39[*i])).collect();
    Ok(format!("{star}0{}0{star}", characters.join("0")))
}

/// Code 93's shift characters (`\301`–`\304`) as values 43–46.
const CODE93_CHARACTERS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-. $/+%";
const CODE93: [&str; 47] = [
    "100010100",
    "101001000",
    "101000100",
    "101000010",
    "100101000",
    "100100100",
    "100100010",
    "101010000",
    "100010010",
    "100001010",
    "110101000",
    "110100100",
    "110100010",
    "110010100",
    "110010010",
    "110001010",
    "101101000",
    "101100100",
    "101100010",
    "100110100",
    "100011010",
    "101011000",
    "101001100",
    "101000110",
    "100101100",
    "100010110",
    "110110100",
    "110110010",
    "110101100",
    "110100110",
    "110010110",
    "110011010",
    "101101100",
    "101100110",
    "100110110",
    "100111010",
    "100101110",
    "111010100",
    "111010010",
    "111001010",
    "101101110",
    "101110110",
    "110101110",
    "100100110",
    "111011010",
    "111010110",
    "100110010",
];

/// `Code93::EXTENDED_MAPPING` for an ASCII character: the values it
/// becomes (shifts are 43 `$`, 44 `%`, 45 `/`, 46 `+`).
fn code93_values(c: char) -> Option<Vec<usize>> {
    let value = |c: char| CODE93_CHARACTERS.find(c);
    let (dollar, percent, slash, plus) = (43, 44, 45, 46);
    let letter = |c: u8| value(c as char).unwrap_or(0);
    let code = c as u32;
    if code > 0x7f {
        return None;
    }
    let b = code as u8;
    Some(match b {
        0 => vec![percent, letter(b'U')],
        1..=26 => vec![dollar, letter(b'A' + b - 1)],
        27..=31 => vec![percent, letter(b'A' + b - 27)],
        b' ' | b'-' | b'.' | b'0'..=b'9' | b'A'..=b'Z' => vec![letter(b)],
        // barby's table: `/` is `/K`, `+` is `/O`.
        b'/' => vec![slash, letter(b'K')],
        b'+' => vec![slash, letter(b'O')],
        b'!'..=b',' => vec![slash, letter(b'A' + b - b'!')],
        b':' => vec![slash, letter(b'Z')],
        b';'..=b'?' => vec![percent, letter(b'F' + b - b';')],
        b'@' => vec![percent, letter(b'V')],
        b'['..=b'_' => vec![percent, letter(b'K' + b - b'[')],
        b'`' => vec![percent, letter(b'W')],
        b'a'..=b'z' => vec![plus, letter(b - 32)],
        b'{'..=b'~' => vec![percent, letter(b'P' + b - b'{')],
        0x7f => vec![percent, letter(b'T')],
        _ => return None,
    })
}

/// `Code93`: extended mapping, the C and K checksums.
fn code93(data: &str) -> Result<String, Failure> {
    let mut values = Vec::new();
    for c in data.chars() {
        let mapped =
            code93_values(c).ok_or_else(|| "undefined method `split' for nil".to_owned())?;
        values.extend(mapped);
    }
    let reversed: Vec<usize> = values.iter().rev().copied().collect();
    let c: usize = reversed
        .iter()
        .enumerate()
        .map(|(i, v)| (i % 20 + 1) * v)
        .sum::<usize>()
        % 47;
    let k: usize = std::iter::once(c)
        .chain(reversed.iter().copied())
        .enumerate()
        .map(|(i, v)| (i % 15 + 1) * v)
        .sum::<usize>()
        % 47;
    let mut out = String::from("101011110");
    for v in values.iter().chain([&c, &k]) {
        out.push_str(CODE93[*v]);
    }
    out.push_str("1010111101");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rubys_line_anchors() {
        assert!(!some_line("abc\n", |l| all_digits(l, None)));
        assert!(some_line("", |l| all_digits(l, None)));
        assert!(some_line("x\n\n", |l| all_digits(l, None)));
        assert!(some_line("12\n", |l| all_digits(l, Some(2))));
    }

    #[test]
    fn encodings_match_barby() {
        // From Ruby (barby 0.6.9).
        assert_eq!(
            code93("ab"),
            Ok(
                "1010111101001100101101010001001100101101001001001110101101011101010111101"
                    .to_owned()
            )
        );
        assert_eq!(
            codabar("A1B"),
            Ok("100111000000100000010000010010011100000010000010000001000000100111".to_owned())
        );
        assert_eq!(
            ean("abc\n123456789012", Ean::Ean13),
            Ok("10100011010001101000110100110010010011011110101010101110010011101010000100010010010001110100101".to_owned())
        );
        assert_eq!(code39("abc"), Err(Failure::Invalid));
        assert_eq!(
            code93("é"),
            Err(Failure::Message(
                "undefined method `split' for nil".to_owned()
            ))
        );
        assert_eq!(
            code25("12x", Code25Kind::Standard),
            Err(Failure::Message("data not valid".to_owned()))
        );
    }
}
