//! `adoctra` CLI (native). Parses an AsciiDoc file, prints any diagnostics to
//! stderr, and converts via the chosen backend.
//!
//! Usage:
//!   adoctra <input.adoc> [--to html|docx|json] [-s] [-a name[=value]]... [-D dir] [-o out|-] [--stdout]
//!   adoctra -            (read from stdin)
//!
//! The safe mode (`-S unsafe|safe|server|secure`, default safe) confines
//! paths to the input's directory below unsafe. Like Ruby's CLI, it supplies the clock (`SOURCE_DATE_EPOCH` or the local
//! time and the input's modification time) for the date attributes, reads a
//! custom stylesheet and docinfo files next to the input, and copies the
//! stylesheet next to the output with `linkcss` and `copycss`.
//!
//! Output follows Ruby's `Asciidoctor.convert`: standard input (and `-o -`)
//! streams to stdout; otherwise the file is `-o`, else
//! `<docname><outfilesuffix>` in `-D` or next to the input. `-D`/`-o`
//! resolve against `-B` or the current directory and, from safe mode up,
//! must stay inside it (an `-o` within `-D` inside the document's base
//! directory); the document sees `outfile`/`outdir` below server mode.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use adoctra::Backend;

mod args;
mod glob;
mod resources;

use args::{override_name, Args, Command, Level, Verbosity};

fn main() -> ExitCode {
    match args::parse(std::env::args().skip(1)) {
        Command::Help => {
            print_usage(&mut std::io::stdout());
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("adoctra {}", env!("CARGO_PKG_VERSION"));
            println!(
                "Compatibility target: Asciidoctor {}",
                adoctra::COMPATIBILITY_VERSION
            );
            ExitCode::SUCCESS
        }
        Command::Usage => {
            print_usage(&mut std::io::stderr());
            ExitCode::FAILURE
        }
        // Like Ruby's OptionParser errors: the message on stderr, the usage
        // on stdout.
        Command::Invalid(message) => {
            eprintln!("adoctra: {message}");
            print_usage(&mut std::io::stdout());
            ExitCode::FAILURE
        }
        Command::Failed(message) => {
            eprintln!("adoctra: {message}");
            ExitCode::FAILURE
        }
        Command::Convert(args) => invoke(args),
    }
}

/// Ruby's `Invoker#invoke!`: every input converted with the same options
/// (standard input or a single pipe streams to stdout unless `-o`; `-R`
/// keeps the input's directory below the source root in `-D`). A failure
/// stops the run; a logged message at `--failure-level` or above fails it
/// too.
fn invoke(args: Args) -> ExitCode {
    LOG.with(|log| log.borrow_mut().verbosity = args.verbosity);
    let result = (|| -> Result<(), Failure> {
        if let Some(backend) = &args.unavailable_backend {
            return Err(Failure::Message(match backend.as_str() {
                "docbook" | "docbook5" | "manpage" => {
                    format!("FAILED: backend '{backend}' is not supported by adoctra")
                }
                _ => format!(
                    "FAILED: missing converter for backend '{backend}'. Processing aborted."
                ),
            }));
        }
        let single_stream = match args.inputs.as_slice() {
            [input] => {
                input == "-" || std::fs::metadata(input).is_ok_and(|meta| args::is_pipe(&meta))
            }
            _ => false,
        };
        for input in &args.inputs {
            let mut job = args.clone();
            job.input = input.clone();
            if single_stream && job.output.is_none() {
                job.output = Some("-".into());
            }
            if let (Some(source), Some(to_dir), false) =
                (&args.source_dir, &args.to_dir, input == "-")
            {
                let dir = expand_path(Path::new(input))?;
                let dir = posix(dir.parent().unwrap_or(Path::new("/")));
                let source = posix(&expand_path(Path::new(source))?);
                if let Some(rest) = dir.strip_prefix(&format!("{source}/")) {
                    job.to_dir = Some(format!("{to_dir}/{rest}"));
                }
            }
            convert(&job)?;
        }
        Ok(())
    })();
    let mut failed = false;
    if let Err(error) = result {
        eprintln!("adoctra: {error}");
        failed = true;
    }
    if LOG
        .with(|log| log.borrow().max)
        .is_some_and(|max| max >= args.failure_level)
    {
        failed = true;
    }
    match failed {
        true => ExitCode::FAILURE,
        false => ExitCode::SUCCESS,
    }
}

/// Ruby's logger for the run: `-q` prints nothing (a NullLogger), `-v`
/// also prints INFO; every logged message raises the maximum severity
/// `--failure-level` reads. A message Ruby logs only at INFO level
/// (`if logger.info?`) is logged with `-v` only (Ruby's NullLogger for `-q`
/// is at WARN level).
struct Log {
    verbosity: Verbosity,
    max: Option<Level>,
}

thread_local! {
    static LOG: std::cell::RefCell<Log> = const {
        std::cell::RefCell::new(Log { verbosity: Verbosity::Normal, max: None })
    };
}

fn log(level: Level, verbose_only: bool, line: impl FnOnce() -> String) {
    LOG.with(|log| {
        let mut log = log.borrow_mut();
        if verbose_only && log.verbosity != Verbosity::Verbose {
            return;
        }
        log.max = log.max.max(Some(level));
        let shown = match log.verbosity {
            Verbosity::Quiet => false,
            Verbosity::Normal => level > Level::Info,
            Verbosity::Verbose => true,
        };
        if shown {
            eprintln!("adoctra: {}", line());
        }
    });
}

fn level(severity: adoctra::Severity) -> Level {
    match severity {
        adoctra::Severity::Info => Level::Info,
        adoctra::Severity::Warning => Level::Warn,
        adoctra::Severity::Error => Level::Error,
    }
}

/// A warning the CLI itself logs (paths, stylesheets, backend messages).
fn warn(message: &str) {
    log(Level::Warn, false, || format!("warning: {message}"));
}

/// Why a run failed: a processor error, or a failure Ruby's CLI reports
/// with its own message (an output path outside the jail, writing over the
/// input file).
enum Failure {
    Error(adoctra::Error),
    Message(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error(error) => error.fmt(f),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl From<adoctra::Error> for Failure {
    fn from(error: adoctra::Error) -> Self {
        Self::Error(error)
    }
}

impl From<adoctra::ResourceError> for Failure {
    fn from(error: adoctra::ResourceError) -> Self {
        Self::Error(error.into())
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Error(error.into())
    }
}

/// Ruby's `Timings#print_report` on stderr.
fn print_timings(input: &str, parsed: std::time::Duration, converting: std::time::Duration) {
    eprintln!("Input file: {input}");
    eprintln!(
        "  Time to read and parse source: {:.5}",
        parsed.as_secs_f64()
    );
    eprintln!(
        "  Time to convert document: {:.5}",
        converting.as_secs_f64()
    );
    eprintln!(
        "  Total time (read, parse and convert): {:.5}",
        (parsed + converting).as_secs_f64()
    );
}

/// One input: Ruby's `Asciidoctor.convert` (or `convert_file`).
fn convert(args: &Args) -> Result<(), Failure> {
    let started = std::time::Instant::now();
    let mut sources = adoctra::SourceMap::new();
    let mut attributes = args.attributes.clone();
    // Ruby's CLI streams standard input (and `-o -`) to standard output;
    // otherwise Asciidoctor.convert writes a file it resolves first.
    let stream = args.to_stdout
        || args.output.as_deref() == Some("-")
        || (args.input == "-" && args.output.is_none());
    // Ruby's `Asciidoctor.convert` with `to_file: '/dev/null'` only loads
    // the document: no conversion, no output path (so no jail check), and
    // only the messages logged while loading.
    let load_only = !stream && args.output.as_deref() == Some("/dev/null");
    let plan = match stream || load_only {
        true => None,
        false => Some(output_plan(args)?),
    };
    // Ruby's Document#initialize: a `:to_file` path sets `outfilesuffix`
    // to its extension, replacing a host value.
    if let (Some(file), false) = (&args.output, stream || load_only) {
        let previous: Vec<_> = attributes
            .iter()
            .filter(|(key, _)| override_name(key) == "outfilesuffix")
            .map(|(key, _)| key.to_owned())
            .collect();
        for key in previous {
            attributes.remove(&key);
        }
        attributes.set("outfilesuffix", extname(file));
    }
    let (source_id, provider) = load_source(args, &mut sources, &mut attributes)?;
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
        output: plan.as_ref().map(|plan| plan.target.clone()),
        // Ruby's `:base_dir` from `-B`; otherwise core uses `docdir`.
        base_dir: match &args.base_dir {
            Some(dir) => Some(posix(&expand_path(dir)?)),
            None => None,
        },
        ..Default::default()
    };
    // A standalone HTML document reads its stylesheet where Ruby's converter
    // builds the head, and rouge highlights listings with custom `subs`
    // inside their substitutions: core calls the backend's hook.
    let referrer = sources
        .get(source_id)
        .map(|source| source.name().to_owned());
    let mut html_start = (args.format == "html" && !load_only).then(|| {
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
        if load_only && d.report.conversion {
            continue;
        }
        if !args.editor_diagnostics {
            let severity = d.report.severity.unwrap_or(d.severity);
            if d.report.suppressed {
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
                log(level(severity), d.report.verbose_only, || {
                    format!(
                        "{}:{}: {}: {message}",
                        source.name(),
                        cursor.line,
                        severity.as_str()
                    )
                });
            } else {
                log(level(severity), d.report.verbose_only, || {
                    format!("{}: {message}", severity.as_str())
                });
            }
            continue;
        }
        // Editor diagnostics print every physical diagnostic; the run's
        // failure level still sees what Ruby logs.
        if !d.report.suppressed && (!d.report.verbose_only || args.verbosity == Verbosity::Verbose)
        {
            let severity = d.report.severity.unwrap_or(d.severity);
            LOG.with(|log| {
                let mut log = log.borrow_mut();
                log.max = log.max.max(Some(level(severity)));
            });
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

    let parsed = started.elapsed();
    if load_only {
        if args.timings {
            print_timings(&args.input, parsed, std::time::Duration::ZERO);
        }
        return Ok(());
    }
    let backend = select_backend(
        &args.format,
        args.embedded,
        provider.clone(),
        referrer,
        html_start,
    )?;
    let bytes = if args.json_attributes {
        if args.format != "json" {
            return Err(
                adoctra::Error::Unsupported("--json-attributes requires --to json".into()).into(),
            );
        }
        adoctra::json::JsonBackend::new().convert_with_attributes(&result.document)?
    } else {
        backend.convert(&result.document)?
    };

    let converted = started.elapsed();
    let timings = || {
        if args.timings {
            print_timings(&args.input, parsed, converted - parsed);
        }
    };
    let Some(plan) = plan else {
        std::io::stdout().write_all(&bytes)?;
        timings();
        return Ok(());
    };
    let paths = result
        .document
        .output
        .clone()
        .expect("core resolves the requested output");
    // Other formats keep their own extension next to the input or in the
    // destination directory.
    let outfile = match (&plan.target, args.format.as_str()) {
        (adoctra::OutputTarget::Directory(dir), "json" | "docx") => format!(
            "{}/{}.{}",
            dir.trim_end_matches('/'),
            result.document.attributes.get("docname").unwrap_or(""),
            backend.extension()
        ),
        _ => paths.file.clone(),
    };
    if plan.input.as_deref() == Some(outfile.as_str()) {
        return Err(Failure::Message(format!(
            "input file and output file cannot be the same: {outfile}"
        )));
    }
    // Like Ruby's CLI (`mkdirs: true`), create the output directory.
    let outdir = Path::new(&outfile)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    mkdir_p(&outdir)?;
    // Ruby's Document#write writes the converted string as it is (only a
    // stream gets `output.chomp` and a LF).
    let mut bytes = bytes;
    if args.format == "html" && bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    write_file(Path::new(&outfile), &bytes)?;
    timings();
    if args.format == "html" && args.safe < adoctra::SafeMode::Secure {
        let highlighter = HIGHLIGHTER_STYLESHEET.with(|cell| cell.borrow_mut().take());
        let docfile = result
            .document
            .attributes
            .get("docfile")
            .unwrap_or("<stdin>");
        copy_stylesheet(
            &result.document.final_attributes,
            &CopyContext {
                base_dir: &document_base_dir(args)?,
                outdir: &posix(&outdir),
                safe: args.safe,
                docfile,
            },
            highlighter,
        )?;
    }
    Ok(())
}

/// The output location of a run writing a file, resolved like Ruby's
/// `Asciidoctor.convert` (paths before loading the document; the file name
/// in a directory after it, by core).
struct OutputPlan {
    target: adoctra::OutputTarget,
    /// The input file's absolute path (Ruby's `File.absolute_path`), which
    /// the output must not replace.
    input: Option<String>,
}

/// Ruby's resolution of `:to_dir`/`:to_file` (`normalize_system_path` with
/// `recover: false`): against `-B` or the current directory, jailed there
/// from safe mode up; a file within the destination directory without a
/// jail; with neither, the input file's directory.
fn output_plan(args: &Args) -> Result<OutputPlan, Failure> {
    let working_dir = posix(&match &args.base_dir {
        Some(dir) => expand_path(dir)?,
        None => std::env::current_dir()?,
    });
    let jail = (args.safe >= adoctra::SafeMode::Safe).then_some(working_dir.as_str());
    let input = (args.input != "-")
        .then(|| expand_path(Path::new(&args.input)).map(|path| posix(&path)))
        .transpose()?;
    let base_dir = document_base_dir(args)?;
    // Document#normalize_system_path: from safe mode up a file without an
    // explicit jail is confined to the document's base directory.
    let resolve = |target: &str, start: &str, jail: Option<&str>| {
        normalize_system_path(
            &base_dir,
            args.safe,
            target,
            Some(start),
            jail,
            "to_dir",
            false,
        )
        .map(|resolved| resolved.path)
        .map_err(Failure::Message)
    };
    let target = match (&args.to_dir, &args.output) {
        (Some(dir), Some(file)) => {
            let outdir = resolve(dir, &working_dir, jail)?;
            adoctra::OutputTarget::File(resolve(file, &outdir, None)?)
        }
        (Some(dir), None) => adoctra::OutputTarget::Directory(resolve(dir, &working_dir, jail)?),
        (None, Some(file)) => adoctra::OutputTarget::File(resolve(file, &working_dir, jail)?),
        (None, None) => {
            let input = input
                .as_deref()
                .expect("a file input writes next to itself");
            let dir = match input.rfind('/') {
                Some(0) => "/",
                Some(slash) => &input[..slash],
                None => ".",
            };
            adoctra::OutputTarget::Directory(dir.to_owned())
        }
    };
    Ok(OutputPlan { target, input })
}

/// Ruby's `Document#base_dir` for the CLI: `-B`, else the input file's
/// directory, else (standard input) a `docdir` attribute or the current
/// directory, absolute (`File.expand_path`).
fn document_base_dir(args: &Args) -> std::io::Result<String> {
    let dir = match (&args.base_dir, args.input.as_str()) {
        (Some(dir), _) => expand_path(dir)?,
        (None, "-") => {
            let docdir = args
                .attributes
                .iter()
                .filter(|(name, _)| override_name(name) == "docdir")
                .last()
                .filter(|(name, _)| !name.ends_with('!') && !name.starts_with('!'))
                .map(|(_, value)| value.to_owned());
            match docdir {
                Some(docdir) => expand_path(Path::new(&docdir))?,
                None => std::env::current_dir()?,
            }
        }
        (None, input) => {
            let input = expand_path(Path::new(input))?;
            input.parent().map(Path::to_path_buf).unwrap_or(input)
        }
    };
    Ok(posix(&dir))
}

/// Ruby's `AbstractNode#normalize_system_path(target, start, jail)` for a
/// document with this base directory and safe mode: below safe mode a
/// relative start joins the base directory and there is no default jail;
/// from safe mode up the base directory is the default start and jail.
fn normalize_system_path(
    base_dir: &str,
    safe: adoctra::SafeMode,
    target: &str,
    start: Option<&str>,
    jail: Option<&str>,
    target_name: &str,
    recover: bool,
) -> Result<adoctra::SystemPath, String> {
    let (start, jail) = if safe < adoctra::SafeMode::Safe {
        let start = match start {
            Some(start) if start.starts_with('/') => start.to_owned(),
            Some(start) => format!("{base_dir}/{start}"),
            None => base_dir.to_owned(),
        };
        (start, jail)
    } else {
        (
            start.unwrap_or(base_dir).to_owned(),
            Some(jail.unwrap_or(base_dir)),
        )
    };
    adoctra::resolve_system_path(target, &start, jail, target_name, recover)
}

/// Ruby's `Helpers.extname`: from the last `.` unless a `/` follows it.
fn extname(path: &str) -> &str {
    match path.rfind('.') {
        Some(dot) if !path[dot..].contains('/') => &path[dot..],
        _ => "",
    }
}

/// A path with `/` separators, as Ruby's posixified paths.
fn posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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

/// Where Ruby's `copycss` step resolves its paths.
struct CopyContext<'a> {
    /// The document's base directory (`-B`, else the input's directory).
    base_dir: &'a str,
    /// The output file's directory.
    outdir: &'a str,
    safe: adoctra::SafeMode,
    /// `docfile`, or `<stdin>`, naming the document in warnings.
    docfile: &'a str,
}

impl CopyContext<'_> {
    /// Ruby's `normalize_system_path(target, start, jail)` for this
    /// document, recovering with warnings.
    fn normalize(&self, target: &str, start: Option<&str>, jail: Option<&str>) -> String {
        let resolved =
            normalize_system_path(self.base_dir, self.safe, target, start, jail, "path", true)
                .expect("recovering resolution never fails");
        for warning in resolved.warnings {
            warn(&warning);
        }
        resolved.path
    }
}

/// Ruby's `Helpers.uriish?` (`UriSniffRx`): a scheme of two or more
/// characters before a colon.
fn uriish(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.len() > 1
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

/// Ruby's `copycss` step after writing a file (`Asciidoctor.convert`, below
/// secure mode): with `linkcss` and `copycss` and a non-URI `stylesdir`,
/// the default stylesheet (adoctra's, under its own name) or a custom one
/// (read from `copycss` when it names a file, else from `stylesheet`,
/// against the base directory) goes to `stylesdir` resolved from the output
/// directory, and so does the syntax highlighter's (rouge after a
/// class-mode listing). Directories are created like the CLI's `mkdirs`.
fn copy_stylesheet(
    attributes: &adoctra::Attributes,
    cx: &CopyContext,
    highlighter: Option<(String, &'static str)>,
) -> Result<(), Failure> {
    let stylesdir = attributes.get("stylesdir");
    if attributes.get("linkcss").is_none()
        || attributes.get("copycss").is_none()
        || stylesdir.is_some_and(uriish)
    {
        return Ok(());
    }
    let stylesheet = attributes.get("stylesheet");
    let copy_default = matches!(stylesheet, Some("" | "DEFAULT"));
    let copy_user = !copy_default && stylesheet.is_some_and(|s| !uriish(s));
    if !copy_default && !copy_user && highlighter.is_none() {
        return Ok(());
    }
    let jail = (cx.safe >= adoctra::SafeMode::Safe).then_some(cx.outdir);
    let stylesoutdir = cx.normalize(stylesdir.unwrap_or(""), Some(cx.outdir), jail);
    mkdir_p(Path::new(&stylesoutdir))?;
    if copy_default {
        write_file(
            &Path::new(&stylesoutdir).join(adoctra::html::DEFAULT_STYLESHEET_NAME),
            adoctra::html::STYLESHEET.as_bytes(),
        )?;
    } else if let Some(stylesheet) = stylesheet.filter(|_| copy_user) {
        let source = match attributes.get("copycss") {
            Some("") | None => cx.normalize(stylesheet, None, None),
            Some(path) => cx.normalize(path, None, None),
        };
        let dest = cx.normalize(stylesheet, Some(&stylesoutdir), jail);
        if source != dest {
            match std::fs::read(&source) {
                Ok(data) => {
                    if let Some(dir) = Path::new(&dest).parent() {
                        mkdir_p(dir)?;
                    }
                    write_file(Path::new(&dest), &data)?;
                }
                // Like Ruby, an existing copy silences the warning.
                Err(_) if Path::new(&dest).is_file() => {}
                Err(_) => warn(&format!(
                    "{}: stylesheet does not exist or cannot be read: {source}",
                    cx.docfile
                )),
            }
        }
    }
    if let Some((name, css)) = highlighter {
        write_file(&Path::new(&stylesoutdir).join(name), css.as_bytes())?;
    }
    Ok(())
}

/// Ruby's `SystemCallError` text for a failed call on `path`:
/// `<strerror> @ <function> - <path>` (e.g. `Is a directory @ rb_sysopen`).
fn system_call_error(error: &std::io::Error, function: &str, path: &Path) -> Failure {
    let text = error.to_string();
    let text = match text.rfind(" (os error ") {
        Some(index) if text.ends_with(')') => text[..index].to_owned(),
        _ => text,
    };
    Failure::Message(format!("{text} @ {function} - {}", path.display()))
}

/// Ruby's `Helpers.mkdir_p`: create missing ancestors first; `Dir.mkdir`
/// fails (`dir_s_mkdir`) on the first path that exists but is not a
/// directory.
fn mkdir_p(dir: &Path) -> Result<(), Failure> {
    if dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty() && *p != Path::new("."))
    {
        mkdir_p(parent)?;
    }
    match std::fs::create_dir(dir) {
        Err(_) if dir.is_dir() => Ok(()),
        Err(error) => Err(system_call_error(&error, "dir_s_mkdir", dir)),
        Ok(()) => Ok(()),
    }
}

/// Ruby's `File.write` (`rb_sysopen` names a failure to open the file).
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    std::fs::write(path, bytes).map_err(|error| system_call_error(&error, "rb_sysopen", path))
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

/// Ruby's `Asciidoctor.load`: `prepare_source_string` decodes a UTF-16 BOM;
/// other input must be UTF-8, or loading fails with Ruby's message naming
/// the absolute input path (`docfile`) or `<stdin>`.
fn load_source(
    args: &Args,
    sources: &mut adoctra::SourceMap,
    attributes: &mut adoctra::Attributes,
) -> Result<(adoctra::SourceId, Option<Arc<resources::FileResources>>), Failure> {
    let unreadable = |docfile: &str| {
        Failure::Message(format!(
            "FAILED: {docfile}: Failed to load AsciiDoc document - \
             source is either binary or contains invalid Unicode data"
        ))
    };
    let jail = |provider: resources::FileResources| match args.safe {
        adoctra::SafeMode::Unsafe => provider.unjailed(),
        _ => provider,
    };
    if args.input == "-" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map_err(adoctra::Error::from)?;
        let s = adoctra::BinaryResource {
            name: "-".to_owned(),
            bytes,
            content_type: None,
        }
        .decode(None)
        .map_err(|_| unreadable("<stdin>"))?
        .text;
        let base_dir = stdin_base_dir(args.base_dir.as_deref(), attributes)?;
        // Like Ruby, relative paths resolve against the base directory; a
        // missing one simply finds nothing.
        let provider = jail(resources::FileResources::base_dir(&base_dir));
        Ok((sources.add("-", s), Some(Arc::new(provider))))
    } else {
        // Ruby reads the requested file by its path; `-B` (which need not
        // exist) is the root and jail of everything else, else the file's
        // directory.
        let path = std::fs::canonicalize(&args.input).map_err(adoctra::Error::from)?;
        let root = match &args.base_dir {
            Some(dir) => expand_path(dir).map_err(adoctra::Error::from)?,
            None => path.parent().map(Path::to_path_buf).ok_or_else(|| {
                adoctra::Error::from(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "input has no parent directory",
                ))
            })?,
        };
        let name = path
            .to_str()
            .ok_or_else(|| {
                adoctra::Error::from(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "input name is not UTF-8",
                ))
            })?
            .to_owned();
        let provider = jail(resources::FileResources::base_dir(&root).with_input(path.clone()));
        let name = name.as_str();
        let id = match sources.load_document(name, None, &provider) {
            Err(error) if error.kind == adoctra::ResourceErrorKind::InvalidEncoding => {
                let docfile = expand_path(Path::new(&args.input)).map_err(adoctra::Error::from)?;
                return Err(unreadable(&docfile.to_string_lossy()));
            }
            result => result?,
        };
        Ok((id, Some(Arc::new(provider))))
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
            warn(warning);
        }
        HIGHLIGHTER_STYLESHEET
            .with(|cell| *cell.borrow_mut() = output.highlighter_stylesheet.take());
        Ok(output.html.into_bytes())
    }
}

fn print_usage(out: &mut dyn Write) {
    let _ = writeln!(
        out,
        "Usage: adoctra [OPTION]... FILE...\n\
         Convert the AsciiDoc input FILE(s) (`-` for standard input) to HTML; the output\n\
         is written to a file named after the input unless -o says otherwise.\n\
         Messages are printed to stderr.\n\n\
         \x20 -b, --backend BACKEND      html5 (default), xhtml5; adoctra's json, docx\n\
         \x20 -d, --doctype DOCTYPE      article, book, manpage or inline\n\
         \x20 -e, --embedded             output an embedded document (no header/footer)\n\
         \x20 -s, --no-header-footer     the same as --embedded\n\
         \x20 -o, --out-file FILE        output file; - for stdout\n\
         \x20 -D, --destination-dir DIR  output directory (default: the input's directory)\n\
         \x20 -R, --source-dir DIR       source root whose structure -D keeps\n\
         \x20 -B, --base-dir DIR         base directory of the document and its resources\n\
         \x20 -S, --safe-mode MODE       unsafe, safe (default), server or secure; from safe\n\
         \x20                            mode up -D/-o stay inside -B or the current directory\n\
         \x20     --safe                 the same as -S safe\n\
         \x20 -n, --section-numbers      number section titles (sectnums)\n\
         \x20 -a, --attribute NAME[=VALUE]  set a document attribute (name@ or value@: soft)\n\
         \x20     --failure-level LEVEL  INFO, WARN, ERROR or FATAL (default) fails the run\n\
         \x20 -q, --quiet                print no messages\n\
         \x20 -v, --verbose              also print INFO messages\n\
         \x20 -t, --timings              print a timings report\n\
         \x20 -w, --warnings, --trace    accepted for compatibility (no effect)\n\
         \x20     --to FORMAT            html, json or docx\n\
         \x20     --stdout               write to stdout (like -o -)\n\
         \x20     --json-attributes      include header/final attributes with --to json\n\
         \x20     --diagnostics MODE     compat (default) or editor with physical byte spans\n\
         \x20 -h, --help                 print this help\n\
         \x20 -V, --version              print the version"
    );
}
