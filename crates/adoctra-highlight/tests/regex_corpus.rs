//! Every regexp of every Rouge state rule, matched like `StringScanner#skip`
//! at each character of its lexer's demo plus a stress text, must give the
//! reference Ruby's match lengths and group ranges. `tests/data/regex_corpus.txt`
//! holds Ruby's FNV-1a fingerprints (`ruby scripts/generate_rouge.rb
//! regex-corpus`); the inputs are rebuilt from it.

use adoctra_highlight::regex::Regex;
use std::collections::HashMap;

const CORPUS: &str = include_str!("data/regex_corpus.txt");

/// Decodes a JSON string literal (Ruby's `JSON.generate` of a String).
fn json_string(text: &str) -> String {
    let inner = &text[1..text.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    let mut pending_high: Option<u32> = None;
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let escaped = chars.next().unwrap();
        let decoded = match escaped {
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
                    pending_high = Some(unit);
                    continue;
                }
                if let Some(high) = pending_high.take() {
                    char::from_u32(0x10000 + ((high - 0xd800) << 10) + (unit - 0xdc00)).unwrap()
                } else {
                    char::from_u32(unit).unwrap()
                }
            }
            other => panic!("unexpected escape \\{other}"),
        };
        out.push(decoded);
    }
    out
}

struct Fingerprint(u64);

impl Fingerprint {
    fn feed(&mut self, value: u64) {
        for i in 0..8 {
            self.0 = (self.0 ^ ((value >> (8 * i)) & 0xff)).wrapping_mul(0x100000001b3);
        }
    }
}

fn fingerprint(regex: &Regex, input: &str) -> u64 {
    let mut hash = Fingerprint(0xcbf29ce484222325);
    for (offset, _) in input.char_indices() {
        let rest = &input[offset..];
        if let Some(captures) = regex.match_at(rest, 0) {
            hash.feed(offset as u64);
            hash.feed(captures.end() as u64);
            for group in 1..=regex.groups() {
                match captures.get(group) {
                    Some((start, end)) => {
                        hash.feed(start as u64);
                        hash.feed(end as u64);
                    }
                    None => {
                        hash.feed(u64::MAX);
                        hash.feed(u64::MAX);
                    }
                }
            }
        }
    }
    hash.0
}

#[test]
fn every_rouge_rule_regexp_matches_like_ruby() {
    let mut stress = String::new();
    let mut demos = HashMap::new();
    let mut failures = Vec::new();
    let mut total = 0;
    for line in CORPUS.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields[0] {
            "S" => stress = json_string(fields[1]),
            "D" => {
                demos.insert(fields[1].to_string(), json_string(fields[2]));
            }
            "R" => {
                total += 1;
                let (tag, options, expected, source) =
                    (fields[1], fields[2], fields[3], json_string(fields[4]));
                let regex = match Regex::new(&source, options.parse().unwrap()) {
                    Ok(regex) => regex,
                    Err(error) => {
                        failures.push(format!("{tag}: compile {source:?}: {error}"));
                        continue;
                    }
                };
                let input = format!("{}\n{}", demos[tag], stress);
                let actual = format!("{:016x}", fingerprint(&regex, &input));
                if actual != expected {
                    failures.push(format!("{tag}: mismatch /{source}/{options}"));
                }
            }
            other => panic!("unknown corpus record {other}"),
        }
    }
    assert!(total > 5000, "corpus has {total} regexps");
    assert!(
        failures.is_empty(),
        "{} of {total} regexps differ from Ruby:\n{}",
        failures.len(),
        failures
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
