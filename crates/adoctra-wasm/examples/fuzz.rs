//! Robustness driver for `scripts/fuzz.py`: the editor path (`parse_json`,
//! `tokens_json`) over many inputs in one process, with no dependencies.
//!
//! stdin carries frames `<id> <byte length>\n<bytes>`; stdout gets one JSON
//! line per input: timings, a caught panic message, and span invariant
//! violations (main-source spans outside the source or off a char boundary).
//! Each input runs on a worker thread with `--stack` bytes (default 1 MiB,
//! about a wasm32 module's stack), so deep nesting that would overflow the
//! editor's stack aborts here too; the harness then knows the input that was
//! running from the missing line.
use adoctra_core::visit::{self, VisitorMut};
use adoctra_core::Span;
use std::io::{self, BufRead, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Spans<'a> {
    len: usize,
    source: &'a str,
    violations: Vec<String>,
}

impl Spans<'_> {
    fn check(&mut self, what: &str, span: &Span) {
        // Only main-source coordinates are checkable without includes.
        if span.source_id.0 != 0 {
            return;
        }
        let ok = span.start <= span.end
            && span.end <= self.len
            && self.source.is_char_boundary(span.start)
            && self.source.is_char_boundary(span.end);
        if !ok && self.violations.len() < 8 {
            self.violations.push(format!(
                "{what} {}..{} of {}",
                span.start, span.end, self.len
            ));
        }
    }
}

impl VisitorMut for Spans<'_> {
    fn visit_span(&mut self, span: &mut Span) {
        self.check("span", span);
    }
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    adoctra_core::json::escape_into(text, &mut out);
    out
}

/// One input on the editor path: (parse ms, tokens ms, json bytes, violations).
fn run(source: &str) -> (f64, f64, usize, Vec<String>) {
    let start = Instant::now();
    let json = adoctra_wasm::parse_json(source);
    let parse_ms = start.elapsed().as_secs_f64() * 1e3;
    let start = Instant::now();
    let tokens = adoctra_wasm::tokens_json(source);
    let tokens_ms = start.elapsed().as_secs_f64() * 1e3;

    let mut result = adoctra_core::parse(source);
    let mut spans = Spans {
        len: source.len(),
        source,
        violations: Vec::new(),
    };
    visit::walk_document_mut(&mut spans, &mut result.document);
    for diagnostic in &result.diagnostics {
        spans.check("diagnostic", &diagnostic.span);
    }
    for token in &result.tokens {
        spans.check("token", &token.span);
    }
    for comment in &result.comments {
        spans.check("comment", comment);
    }
    for token in adoctra_core::tokens(source) {
        spans.check("tokens()", &token.span);
    }
    (
        parse_ms,
        tokens_ms,
        json.len() + tokens.len(),
        spans.violations,
    )
}

fn main() -> io::Result<()> {
    let mut stack = 1 << 20;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stack" => {
                stack = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--stack BYTES")
            }
            other => panic!("unknown argument {other}"),
        }
    }
    let message = Arc::new(Mutex::new(None::<String>));
    let hook_message = Arc::clone(&message);
    panic::set_hook(Box::new(move |info| {
        let text = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let place = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        *hook_message.lock().unwrap() = Some(format!("{text}{place}"));
    }));

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut header = String::new();
    loop {
        header.clear();
        if input.read_line(&mut header)? == 0 {
            return Ok(());
        }
        let (id, len) = header
            .trim_end()
            .split_once(' ')
            .expect("frame header `<id> <len>`");
        let mut bytes = vec![0; len.parse().expect("frame length")];
        input.read_exact(&mut bytes)?;
        let source = String::from_utf8(bytes).expect("UTF-8 input");
        *message.lock().unwrap() = None;
        let outcome = std::thread::Builder::new()
            .stack_size(stack)
            .spawn(move || panic::catch_unwind(AssertUnwindSafe(|| run(&source))))?
            .join()
            .expect("worker thread");
        let line = match outcome {
            Ok((parse_ms, tokens_ms, bytes, violations)) => format!(
                "{{\"id\":{},\"parse_ms\":{parse_ms:.3},\"tokens_ms\":{tokens_ms:.3},\"bytes\":{bytes},\"violations\":[{}]}}",
                escape(id),
                violations.iter().map(|v| escape(v)).collect::<Vec<_>>().join(",")
            ),
            Err(_) => format!(
                "{{\"id\":{},\"panic\":{}}}",
                escape(id),
                escape(message.lock().unwrap().as_deref().unwrap_or("panic"))
            ),
        };
        writeln!(out, "{line}")?;
        out.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn editor_path_checks_spans_of_unicode_crlf_and_unfinished_input() {
        for source in [
            "= Т\r\n\r\nЖ *🙂\r\n----\r\nx",
            "``\u{43e}}",
            ":doctype: manpage",
            "\u{a0}\n [[a]]\n|===\na|",
        ] {
            let (parse_ms, tokens_ms, bytes, violations) = run(source);
            assert!(
                parse_ms >= 0.0 && tokens_ms >= 0.0 && bytes > 0,
                "{source:?}"
            );
            assert!(violations.is_empty(), "{source:?}: {violations:?}");
        }
    }
}
