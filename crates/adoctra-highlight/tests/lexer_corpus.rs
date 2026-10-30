//! Every ported lexer must produce the reference Rouge's token stream for
//! its demo (`lib/rouge/demos`, MIT). `tests/data/lexer_corpus.txt` holds Ruby's FNV-1a
//! fingerprints and the inputs (`ruby scripts/generate_rouge.rb
//! lexer-corpus`).

use adoctra_highlight::lexer::{self, Lexer};

const CORPUS: &str = include_str!("data/lexer_corpus.txt");

/// Decodes a JSON string literal (Ruby's `JSON.generate` of a String).
fn json_string(text: &str) -> String {
    let inner = &text[1..text.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    let mut high: Option<u32> = None;
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let decoded = match chars.next().unwrap() {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'b' => '\u{8}',
            'f' => '\u{c}',
            '/' => '/',
            '"' => '"',
            '\\' => '\\',
            'u' => {
                let hex: String = (&mut chars).take(4).collect();
                let unit = u32::from_str_radix(&hex, 16).unwrap();
                if (0xd800..0xdc00).contains(&unit) {
                    high = Some(unit);
                    continue;
                }
                match high.take() {
                    Some(high) => {
                        char::from_u32(0x10000 + ((high - 0xd800) << 10) + (unit - 0xdc00)).unwrap()
                    }
                    None => char::from_u32(unit).unwrap(),
                }
            }
            other => panic!("unexpected escape \\{other}"),
        };
        out.push(decoded);
    }
    out
}

fn fingerprint(tokens: &[(adoctra_highlight::token::Token, String)]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut feed = |value: u64| {
        for i in 0..8 {
            hash = (hash ^ ((value >> (8 * i)) & 0xff)).wrapping_mul(0x100000001b3);
        }
    };
    for (token, value) in tokens {
        feed(token.id() as u64);
        feed(value.len() as u64);
        for byte in value.bytes() {
            feed(byte as u64);
        }
    }
    format!("{hash:016x}")
}

#[test]
fn ported_lexers_reproduce_rouge_token_streams() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for line in CORPUS.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields[0], "L", "Rouge failed on a demo: {line}");
        let (tag, expected, input) = (fields[1], fields[2], json_string(fields[3]));
        let Some(mut lexer) = Lexer::find(tag) else {
            continue;
        };
        checked += 1;
        match lexer.lex(&input) {
            Ok(tokens) => {
                let actual = fingerprint(&tokens);
                if actual != expected {
                    failures.push(format!("{tag}: token stream differs"));
                }
            }
            Err(error) => failures.push(format!("{tag}: {error}")),
        }
    }
    let ported = lexer::all().count();
    assert_eq!(checked, ported, "every ported lexer has a corpus entry");
    assert!(
        failures.is_empty(),
        "{} of {checked} lexers differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn reports_the_port_status() {
    let ported: Vec<_> = lexer::all().map(|def| def.tag).collect();
    let total = lexer::every().count();
    eprintln!(
        "{} of {total} lexers ported: {}",
        ported.len(),
        ported.join(" ")
    );
    assert_eq!(total, CORPUS.lines().count());
}
