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
    base_dir: Option<PathBuf>,
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
    let mut attributes = args.attributes.clone();
    let (source_id, provider, base) = load_source(&args, &mut sources, &mut attributes)?;
    // Like Ruby below server mode, the document sees the user's home.
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
    // builds the head, and rouge highlights listings with custom `subs`
    // inside their substitutions: core calls the backend's hook.
    let referrer = sources
        .get(source_id)
        .map(|source| source.name().to_owned());
    let mut html_start = (args.backend == "html").then(|| {
        if args.embedded {
            adoctra::html::HtmlBackend::embedded().start()
        } else {
            html_backend(provider.clone(), referrer.clone()).start()
        }
    });
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
        let highlighter = HIGHLIGHTER_STYLESHEET.with(|cell| cell.borrow_mut().take());
        copy_stylesheet(
            &result.document.final_attributes,
            &base,
            &out_path,
            highlighter,
        )?;
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
/// directory) is copied there. The syntax highlighter's stylesheet (rouge
/// after a class-mode listing) is written there too.
fn copy_stylesheet(
    attributes: &adoctra::Attributes,
    base: &Path,
    out_path: &Path,
    highlighter: Option<(String, &'static str)>,
) -> adoctra::Result<()> {
    let uri = |value: &str| value.contains("://");
    let stylesdir = attributes.get("stylesdir").unwrap_or("");
    if attributes.get("linkcss").is_none() || attributes.get("copycss").is_none() || uri(stylesdir)
    {
        return Ok(());
    }
    let outdir = out_path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stylesoutdir = outdir.join(stylesdir);
    copy_document_stylesheet(attributes, base, &stylesoutdir)?;
    if let Some((name, css)) = highlighter {
        std::fs::create_dir_all(&stylesoutdir)?;
        std::fs::write(stylesoutdir.join(name), css)?;
    }
    Ok(())
}

fn copy_document_stylesheet(
    attributes: &adoctra::Attributes,
    base: &Path,
    stylesoutdir: &Path,
) -> adoctra::Result<()> {
    let uri = |value: &str| value.contains("://");
    let Some(stylesheet) = attributes.get("stylesheet") else {
        return Ok(());
    };
    if matches!(stylesheet, "" | "DEFAULT") {
        std::fs::create_dir_all(stylesoutdir)?;
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
    let mut base_dir = None;

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
            "-B" | "--base-dir" => base_dir = Some(PathBuf::from(it.next()?)),
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
        base_dir,
    })
}

fn override_name(name: &str) -> String {
    let name = name.strip_suffix('@').unwrap_or(name);
    name.strip_prefix('!')
        .or_else(|| name.strip_suffix('!'))
        .unwrap_or(name)
        .to_lowercase()
}

/// Ruby's `base_dir` for standard input (Document#initialize): `-B`
/// (which also replaces `docdir`), else a `docdir` attribute, else the
/// current directory (which becomes `docdir`). Ruby's File.expand_path
/// keeps symlinks, so `docdir` is the absolute, lexically normalized path.
fn stdin_base_dir(
    base_dir: Option<&Path>,
    attributes: &mut adoctra::Attributes,
) -> adoctra::Result<PathBuf> {
    let docdir = attributes
        .iter()
        .filter(|(name, _)| override_name(name) == "docdir")
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .last();
    let dir = match (base_dir, &docdir) {
        (Some(dir), _) => expand_path(dir)?,
        (None, Some((name, value))) if !name.ends_with('!') && !name.starts_with('!') => {
            return Ok(expand_path(Path::new(value))?);
        }
        (None, _) => std::env::current_dir()?,
    };
    if let Some((name, _)) = docdir {
        attributes.remove(&name);
    }
    attributes.set("docdir", dir.to_string_lossy().replace('\\', "/"));
    Ok(dir)
}

/// Ruby's `File.expand_path`: absolute, `.`/`..` resolved lexically.
fn expand_path(path: &Path) -> std::io::Result<PathBuf> {
    use std::path::Component;
    let mut expanded = PathBuf::new();
    for component in std::env::current_dir()?.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                expanded.pop();
            }
            other => expanded.push(other),
        }
    }
    Ok(expanded)
}

fn load_source(
    args: &Args,
    sources: &mut adoctra::SourceMap,
    attributes: &mut adoctra::Attributes,
) -> adoctra::Result<(
    adoctra::SourceId,
    Option<Arc<resources::FileResources>>,
    PathBuf,
)> {
    let jail = |provider: resources::FileResources| match args.safe {
        adoctra::SafeMode::Unsafe => provider.unjailed(),
        _ => provider,
    };
    if args.input == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        let base_dir = stdin_base_dir(args.base_dir.as_deref(), attributes)?;
        // Like Ruby, relative paths resolve against the base directory; a
        // missing one simply finds nothing.
        let provider = resources::FileResources::new(&base_dir).ok().map(jail);
        Ok((sources.add("-", s), provider.map(Arc::new), base_dir))
    } else {
        // The explicitly requested primary file establishes the resource
        // root, or `-B` does when it contains the file.
        let path = std::fs::canonicalize(&args.input)?;
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "input has no parent directory",
            )
        })?;
        let root = match &args.base_dir {
            Some(dir) => std::fs::canonicalize(dir)?,
            None => parent.to_path_buf(),
        };
        let name = path
            .strip_prefix(&root)
            .ok()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "input is outside the base directory or its name is not UTF-8",
                )
            })?;
        let provider = jail(resources::FileResources::new(&root)?);
        Ok((
            sources.load(name, None, &provider)?,
            Some(Arc::new(provider)),
            parent.to_path_buf(),
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
        // Embedded output prints its warnings too.
        "html" if embedded => Ok(Box::new(Html(
            adoctra::html::HtmlBackend::embedded(),
            start,
        ))),
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

thread_local! {
    /// The highlighter stylesheet of the last HTML conversion, for the
    /// `copycss` step (the backend trait only returns bytes).
    static HIGHLIGHTER_STYLESHEET: std::cell::RefCell<Option<(String, &'static str)>> =
        const { std::cell::RefCell::new(None) };
}

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
        HIGHLIGHTER_STYLESHEET
            .with(|cell| *cell.borrow_mut() = output.highlighter_stylesheet.take());
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
         -B, --base-dir <DIR>  root for relative paths (default: the input's directory,\n                       \
         for stdin the docdir attribute or the current directory)\n  \
         -o, --out <FILE>     output path\n      --stdout         write to stdout\n  \
         --json-attributes   include header/final attributes with --to json\n  \
         --diagnostics <MODE> compat (default) or editor with physical byte spans\n  \
         -h, --help           show this help"
    );
}
