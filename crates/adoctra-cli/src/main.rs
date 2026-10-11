//! `adoctra` CLI (native). Parses an AsciiDoc file, prints any diagnostics to
//! stderr, and converts via the chosen backend.
//!
//! Usage:
//!   adoctra <input.adoc> [--to html|docx|json] [-o out] [--stdout]
//!   adoctra -            (read from stdin)

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use adoctra::Backend;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("adoctra: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    input: String,
    backend: String,
    output: Option<PathBuf>,
    to_stdout: bool,
}

fn run() -> adoctra::Result<()> {
    let args = match parse_args() {
        Some(a) => a,
        None => {
            print_usage();
            return Ok(());
        }
    };

    let mut sources = adoctra::SourceMap::new();
    let source_id = sources.add(&args.input, read_source(&args.input)?);
    let source = sources.get(source_id).expect("just registered source");

    // Parsing never fails on syntax; surface diagnostics, then keep going.
    let options = adoctra::ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = adoctra::parse_with_options(source.text(), &options);
    for d in &result.diagnostics {
        let file = sources
            .get(d.span.source_id)
            .expect("parser source is registered");
        let position = file.position(d.span.start);
        let location = match position {
            Some(p) => format!("{}:{}:{}", file.name(), p.line, p.column),
            None => file.name().to_owned(),
        };
        eprintln!(
            "adoctra: {location}: {} at {}..{}: {}",
            d.severity.as_str(),
            d.span.start,
            d.span.end,
            d.message
        );
    }

    let backend = select_backend(&args.backend)?;
    let bytes = backend.convert(&result.document)?;

    if args.to_stdout || (args.input == "-" && args.output.is_none()) {
        std::io::stdout().write_all(&bytes)?;
        return Ok(());
    }

    let out_path = args
        .output
        .unwrap_or_else(|| default_output(&args.input, backend.extension()));
    std::fs::write(&out_path, &bytes)?;
    eprintln!("wrote {}", out_path.display());
    Ok(())
}

fn parse_args() -> Option<Args> {
    let mut input: Option<String> = None;
    let mut backend = "html".to_string();
    let mut output: Option<PathBuf> = None;
    let mut to_stdout = false;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return None,
            "--to" | "-b" => backend = it.next()?,
            "-o" | "--out" => output = Some(PathBuf::from(it.next()?)),
            "--stdout" => to_stdout = true,
            other => {
                if input.is_some() {
                    return None;
                }
                input = Some(other.to_string());
            }
        }
    }

    Some(Args {
        input: input?,
        backend,
        output,
        to_stdout,
    })
}

fn read_source(input: &str) -> adoctra::Result<String> {
    if input == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        Ok(std::fs::read_to_string(input)?)
    }
}

fn select_backend(name: &str) -> adoctra::Result<Box<dyn Backend>> {
    match name {
        "html" => Ok(Box::new(adoctra::html::HtmlBackend::new())),
        "docx" => Ok(Box::new(adoctra::docx::DocxBackend::new())),
        "json" => Ok(Box::new(adoctra::json::JsonBackend::new())),
        other => Err(adoctra::Error::Unsupported(format!("backend `{other}`"))),
    }
}

fn default_output(input: &str, ext: &str) -> PathBuf {
    let mut p = PathBuf::from(input);
    p.set_extension(ext);
    p
}

fn print_usage() {
    eprintln!(
        "adoctra — AsciiDoc processor\n\n\
         USAGE:\n  adoctra <input.adoc> [--to html|docx|json] [-o <out>] [--stdout]\n  \
         adoctra -   (read from stdin)\n\n\
         OPTIONS:\n  -b, --to <BACKEND>   output format [default: html]\n  \
         -o, --out <FILE>     output path\n      --stdout         write to stdout\n  \
         -h, --help           show this help"
    );
}
