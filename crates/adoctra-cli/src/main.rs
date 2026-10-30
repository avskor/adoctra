//! `adoctra` CLI (native). Parses an AsciiDoc file, prints any diagnostics to
//! stderr, and converts via the chosen backend.
//!
//! Usage:
//!   adoctra <input.adoc> [--to html|docx|json] [-s] [-a name[=value]]... [-o out] [--stdout]
//!   adoctra -            (read from stdin)
//!
//! The safe mode (`-S unsafe|safe|server|secure`, default safe) confines
//! paths to the input's directory below unsafe. Like Ruby's CLI, it supplies the clock (`SOURCE_DATE_EPOCH` or the local
//! time and the input's modification time) for the date attributes, reads a
//! custom stylesheet and docinfo files next to the input, and copies the
//! stylesheet next to the output with `linkcss` and `copycss`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use adoctra::Backend;

mod resources;

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
    embedded: bool,
    attributes: adoctra::Attributes,
    safe: adoctra::SafeMode,
    json_attributes: bool,
    editor_diagnostics: bool,
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
    let (source_id, provider) = load_source(&args.input, &mut sources, args.safe)?;
    // Like Ruby below server mode, the document sees the user's home.
    let mut attributes = args.attributes.clone();
    if args.safe < adoctra::SafeMode::Server
        && !attributes
            .iter()
            .any(|(name, _)| override_name(name) == "user-home")
    {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            attributes.set("user-home", home.to_string_lossy().replace('\\', "/"));
        }
    }

    // Parsing never fails on syntax; surface diagnostics, then keep going.
    let options = adoctra::ParseOptions {
        source_id,
        attributes,
        embedded: args.embedded,
        safe: args.safe,
        dates: Some(dates(&args.input)?),
        ..Default::default()
    };
    // A standalone HTML document reads its stylesheet where Ruby's converter
    // builds the head: core calls the backend's hook before the body converts.
    let referrer = sources
        .get(source_id)
        .map(|source| source.name().to_owned());
    let mut html_start = (args.backend == "html" && !args.embedded)
        .then(|| html_backend(provider.clone(), referrer.clone()).start());
    let result = match (&provider, html_start.as_mut()) {
        (Some(provider), Some(start)) => {
            adoctra::parse_resource_with_start(&mut sources, &options, provider.as_ref(), start)?
        }
        (Some(provider), None) => {
            adoctra::parse_resource(&mut sources, &options, provider.as_ref())?
        }
        (None, start) => {
            let text = sources.get(source_id).expect("registered stdin").text();
            match start {
                Some(start) => adoctra::parse_with_start(text, &options, start),
                None => adoctra::parse_with_options(text, &options),
            }
        }
    };
    for d in &result.diagnostics {
        if !args.editor_diagnostics {
            let severity = d.report.severity.unwrap_or(d.severity);
            if d.report.suppressed || severity == adoctra::Severity::Info {
                continue;
            }
            let message = d.report.message.as_deref().unwrap_or(&d.message);
            let cursor = match d.report.location {
                adoctra::DiagnosticLocation::Unlocated => None,
                adoctra::DiagnosticLocation::Cursor(cursor) => Some(cursor),
                adoctra::DiagnosticLocation::Source => sources
                    .get(d.span.source_id)
                    .and_then(|source| source.position(d.span.start))
                    .map(|position| adoctra::DiagnosticCursor {
                        source_id: d.span.source_id,
                        line: position.line,
                    }),
            };
            if let Some(cursor) = cursor {
                let source = sources
                    .get(cursor.source_id)
                    .expect("diagnostic source is registered");
                eprintln!(
                    "adoctra: {}:{}: {}: {message}",
                    source.name(),
                    cursor.line,
                    severity.as_str()
                );
            } else {
                eprintln!("adoctra: {}: {message}", severity.as_str());
            }
            continue;
        }
        let file = sources
            .get(d.span.source_id)
            .expect("parser source is registered");
        if let Some(raw) = d.omitted_span {
            eprintln!(
                "adoctra: {}:{}:{}: {} at omitted bytes {}..{}: {}",
                file.name(),
                raw.line,
                raw.start + 1,
                d.severity.as_str(),
                raw.start,
                raw.end,
                d.message
            );
            continue;
        }
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

    let backend = select_backend(
        &args.backend,
        args.embedded,
        provider.clone(),
        referrer,
        html_start,
    )?;
    let bytes = if args.json_attributes {
        if args.backend != "json" {
            return Err(adoctra::Error::Unsupported(
                "--json-attributes requires --to json".into(),
            ));
        }
        adoctra::json::JsonBackend::new().convert_with_attributes(&result.document)?
    } else {
        backend.convert(&result.document)?
    };

    if args.to_stdout || (args.input == "-" && args.output.is_none()) {
        std::io::stdout().write_all(&bytes)?;
        return Ok(());
    }

    let out_path = args
        .output
        .unwrap_or_else(|| default_output(&args.input, backend.extension()));
    // Like Ruby's CLI (`mkdirs: true`), create the output directory.
    if let Some(dir) = out_path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out_path, &bytes)?;
    eprintln!("wrote {}", out_path.display());
    if args.backend == "html" && args.safe < adoctra::SafeMode::Secure {
        let base = match &args.input[..] {
            "-" => std::env::current_dir()?,
            input => std::fs::canonicalize(input)?
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        };
        copy_stylesheet(&result.document.final_attributes, &base, &out_path)?;
    }
    Ok(())
}

/// The clock for Ruby's date attributes: `SOURCE_DATE_EPOCH` (UTC, also as
/// the document time), else the local time and the input file's
/// modification time (the current time for stdin).
fn dates(input: &str) -> adoctra::Result<adoctra::DocumentDates> {
    use chrono::{Datelike, Offset, TimeZone, Timelike};
    fn convert<Tz: TimeZone>(time: chrono::DateTime<Tz>) -> adoctra::DateTime {
        adoctra::DateTime {
            year: time.year(),
            month: time.month() as u8,
            day: time.day() as u8,
            hour: time.hour() as u8,
            minute: time.minute() as u8,
            second: time.second() as u8,
            utc_offset: time.offset().fix().local_minus_utc(),
        }
    }
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH") {
        let time = epoch
            .trim()
            .parse::<i64>()
            .ok()
            .and_then(|secs| chrono::Utc.timestamp_opt(secs, 0).single())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("invalid SOURCE_DATE_EPOCH: {epoch}"),
                )
            })?;
        let time = convert(time);
        return Ok(adoctra::DocumentDates {
            local: time,
            document: time,
        });
    }
    let now = chrono::Local::now();
    let document = match input {
        "-" => now,
        path => chrono::DateTime::<chrono::Local>::from(std::fs::metadata(path)?.modified()?),
    };
    Ok(adoctra::DocumentDates {
        local: convert(now),
        document: convert(document),
    })
}

/// Ruby's `copycss` step after writing a file: with `linkcss` and `copycss`
/// (and a non-URI `stylesdir`), adoctra's stylesheet is written into
/// `stylesdir` next to the output, or a custom stylesheet (from `copycss`
/// when it names a file, else the `stylesheet` path, relative to the input's
/// directory) is copied there.
fn copy_stylesheet(
    attributes: &adoctra::Attributes,
    base: &Path,
    out_path: &Path,
) -> adoctra::Result<()> {
    let uri = |value: &str| value.contains("://");
    let stylesdir = attributes.get("stylesdir").unwrap_or("");
    if attributes.get("linkcss").is_none() || attributes.get("copycss").is_none() || uri(stylesdir)
    {
        return Ok(());
    }
    let Some(stylesheet) = attributes.get("stylesheet") else {
        return Ok(());
    };
    let outdir = out_path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stylesoutdir = outdir.join(stylesdir);
    if matches!(stylesheet, "" | "DEFAULT") {
        std::fs::create_dir_all(&stylesoutdir)?;
        std::fs::write(
            stylesoutdir.join(adoctra::html::DEFAULT_STYLESHEET_NAME),
            adoctra::html::STYLESHEET,
        )?;
    } else if !uri(stylesheet) {
        let source = match attributes.get("copycss") {
            Some("") | None => base.join(stylesheet),
            Some(path) => base.join(path),
        };
        let dest = stylesoutdir.join(stylesheet);
        let same = match (source.canonicalize(), dest.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same {
            match std::fs::read(&source) {
                Ok(data) => {
                    if let Some(dir) = dest.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    std::fs::write(&dest, data)?;
                }
                // Like Ruby, an existing copy silences the warning.
                Err(_) if dest.is_file() => {}
                Err(_) => eprintln!(
                    "adoctra: warning: stylesheet does not exist or cannot be read: {}",
                    source.display()
                ),
            }
        }
    }
    Ok(())
}

fn parse_args() -> Option<Args> {
    let mut input: Option<String> = None;
    let mut backend = "html".to_string();
    let mut output: Option<PathBuf> = None;
    let mut to_stdout = false;
    let mut embedded = false;
    let mut json_attributes = false;
    let mut editor_diagnostics = false;
    let mut attributes = adoctra::Attributes::new();
    // adoctra's CLI defaults to safe mode (Ruby's to unsafe): paths stay in
    // the input's directory.
    let mut safe = adoctra::SafeMode::Safe;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return None,
            "--to" | "-b" => backend = it.next()?,
            "-o" | "--out" => output = Some(PathBuf::from(it.next()?)),
            "--stdout" => to_stdout = true,
            "--json-attributes" => json_attributes = true,
            "--diagnostics" => {
                editor_diagnostics = match it.next()?.as_str() {
                    "compat" => false,
                    "editor" => true,
                    _ => return None,
                }
            }
            "-s" | "--embedded" | "--no-header-footer" => embedded = true,
            "-S" | "--safe-mode" => safe = adoctra::SafeMode::from_name(&it.next()?)?,
            "--safe" => safe = adoctra::SafeMode::Safe,
            // Keep Ruby's soft/default and unset markers for ParseOptions.
            "-a" | "--attribute" => {
                let entry = it.next()?;
                let (name, value) = entry.split_once('=').unwrap_or((&entry, ""));
                if name.trim_matches(['!', '@']).is_empty() {
                    return None;
                }
                // Preserve command-line order when spellings of the same
                // override change, e.g. `-a x! -a x=value@`.
                let previous: Vec<_> = attributes
                    .iter()
                    .filter(|(key, _)| override_name(key) == override_name(name))
                    .map(|(key, _)| key.to_owned())
                    .collect();
                for key in previous {
                    attributes.remove(&key);
                }
                attributes.set(name, value);
            }
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
        embedded,
        attributes,
        safe,
        json_attributes,
        editor_diagnostics,
    })
}

fn override_name(name: &str) -> String {
    let name = name.strip_suffix('@').unwrap_or(name);
    name.strip_prefix('!')
        .or_else(|| name.strip_suffix('!'))
        .unwrap_or(name)
        .to_lowercase()
}

fn load_source(
    input: &str,
    sources: &mut adoctra::SourceMap,
    safe: adoctra::SafeMode,
) -> adoctra::Result<(adoctra::SourceId, Option<Arc<resources::FileResources>>)> {
    if input == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok((sources.add(input, s), None))
    } else {
        // The explicitly requested primary file establishes the resource root.
        let path = std::fs::canonicalize(input)?;
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "input has no parent directory",
            )
        })?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "input file name is not UTF-8",
                )
            })?;
        let provider = resources::FileResources::new(parent)?;
        let provider = match safe {
            adoctra::SafeMode::Unsafe => provider.unjailed(),
            _ => provider,
        };
        Ok((
            sources.load(name, None, &provider)?,
            Some(Arc::new(provider)),
        ))
    }
}

fn select_backend(
    name: &str,
    embedded: bool,
    provider: Option<Arc<resources::FileResources>>,
    referrer: Option<String>,
    start: Option<adoctra::html::HtmlStart>,
) -> adoctra::Result<Box<dyn Backend>> {
    match name {
        // Embedded output prints its warnings too (Html, without a start).
        "html" if embedded => Ok(Box::new(Html(adoctra::html::HtmlBackend::embedded(), None))),
        "html" => Ok(Box::new(Html(html_backend(provider, referrer), start))),
        "docx" => Ok(Box::new(adoctra::docx::DocxBackend::new())),
        "json" => Ok(Box::new(adoctra::json::JsonBackend::new())),
        other => Err(adoctra::Error::Unsupported(format!("backend `{other}`"))),
    }
}

/// The standalone HTML backend, reading through the CLI's resources.
fn html_backend(
    provider: Option<Arc<resources::FileResources>>,
    referrer: Option<String>,
) -> adoctra::html::HtmlBackend {
    let backend = adoctra::html::HtmlBackend::new();
    match provider {
        Some(provider) => backend.with_resources(provider, referrer),
        None => backend,
    }
}

/// The HTML backend, printing its warnings like the parser's diagnostics,
/// with the stylesheet its start hook read during parsing.
struct Html(adoctra::html::HtmlBackend, Option<adoctra::html::HtmlStart>);

impl Backend for Html {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn extension(&self) -> &'static str {
        self.0.extension()
    }

    fn convert(&self, doc: &adoctra::Document) -> adoctra::Result<Vec<u8>> {
        let mut output = match &self.1 {
            Some(start) => self.0.render_with_start(doc, start),
            None => self.0.render(doc),
        };
        // Like Ruby's CLI (`output.chomp` and a LF), end non-empty output
        // with a newline: the inline doctype's content has none.
        if !output.html.is_empty() && !output.html.ends_with('\n') {
            output.html.push('\n');
        }
        for warning in &output.warnings {
            eprintln!("adoctra: warning: {warning}");
        }
        Ok(output.html.into_bytes())
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
         USAGE:\n  adoctra <input.adoc> [--to html|docx|json] [-s] [-a <name=value>] [-o <out>] [--stdout]\n  \
         adoctra -   (read from stdin)\n\n\
         OPTIONS:\n  -b, --to <BACKEND>   output format [default: html]\n  \
         -s, --embedded       HTML body content only (no document shell, header or footer)\n  \
         -a, --attribute <NAME[=VALUE]>  set a document attribute\n  \
         -S, --safe-mode <MODE>  unsafe, safe (default), server or secure\n  \
         -o, --out <FILE>     output path\n      --stdout         write to stdout\n  \
         --json-attributes   include header/final attributes with --to json\n  \
         --diagnostics <MODE> compat (default) or editor with physical byte spans\n  \
         -h, --help           show this help"
    );
}
