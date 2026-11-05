//! Diagram converters (asciidoctor-diagram's `*Converter` classes) and the
//! shared helpers of `CliGenerator`, `Cli` and `find_command`.

pub(crate) mod graphviz;
pub(crate) mod http;
pub(crate) mod java_tools;
pub(crate) mod mermaid;
pub(crate) mod plantuml;
pub(crate) mod tools;

pub(crate) use tools::Syntrax;

use crate::host::{Command, DiagramHost};
use crate::source::{expand_path, Inherit, Source};
use std::sync::atomic::{AtomicU64, Ordering};

/// A value of a converter's options hash (stored in the cache metadata and
/// compared to decide regeneration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OptValue {
    Null,
    Str(String),
    Bool(bool),
    Int(i64),
}

impl From<Option<String>> for OptValue {
    fn from(value: Option<String>) -> Self {
        value.map_or(OptValue::Null, OptValue::Str)
    }
}

pub(crate) type Options = Vec<(&'static str, OptValue)>;

/// Ruby's `JSON.dump` of the options hash.
pub(crate) fn options_json(options: &Options) -> String {
    let fields: Vec<String> = options
        .iter()
        .map(|(key, value)| {
            let value = match value {
                OptValue::Null => "null".to_owned(),
                OptValue::Bool(b) => b.to_string(),
                OptValue::Int(n) => n.to_string(),
                OptValue::Str(s) => json_string(s),
            };
            format!("{}:{value}", json_string(key))
        })
        .collect();
    format!("{{{}}}", fields.join(","))
}

pub(crate) fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A diagram converter.
pub(crate) trait Converter: Send + Sync {
    fn supported_formats(&self) -> &'static [&'static str];
    /// Whether the tool scales itself (no `scale` factor on the size).
    fn native_scaling(&self) -> bool {
        false
    }
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String>;
    /// The code the tool receives (`wrap_source`), computed when converting.
    fn code(&self, source: &Source<'_, '_>, _host: &dyn DiagramHost) -> Result<String, String> {
        Ok(source.loaded_code().to_owned())
    }
    /// The code an HTTP server receives (the wrapped code without local
    /// preprocessing unless the converter needs it).
    fn server_code(
        &self,
        source: &Source<'_, '_>,
        host: &dyn DiagramHost,
    ) -> Result<String, String> {
        self.code(source, host)
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String>;
}

/// `find_command(cmd, attrs:/alt_attrs:, alt_cmds:, raise_on_error:)`.
pub(crate) struct Find<'n> {
    pub cmd: &'n str,
    pub attrs: Option<&'n [&'n str]>,
    pub alt_attrs: &'n [&'n str],
    pub alt_cmds: &'n [&'n str],
    pub raise: bool,
}

impl<'n> Find<'n> {
    pub fn new(cmd: &'n str) -> Self {
        Self {
            cmd,
            attrs: None,
            alt_attrs: &[],
            alt_cmds: &[],
            raise: true,
        }
    }
}

pub(crate) fn find_command(
    source: &Source<'_, '_>,
    host: &dyn DiagramHost,
    find: Find<'_>,
) -> Result<Option<String>, String> {
    let attr_names: Vec<&str> = match find.attrs {
        Some(attrs) => attrs.to_vec(),
        None => find
            .alt_attrs
            .iter()
            .copied()
            .chain(std::iter::once(find.cmd))
            .collect(),
    };
    let cmd_names: Vec<&str> = std::iter::once(find.cmd)
        .chain(find.alt_cmds.iter().copied())
        .collect();
    // Ruby caches the result (even `nil`) in the processor's `config`
    // under `cmd-<first attribute name>`, shared by lookups with that name.
    let cmd_var = format!("cmd-{}", attr_names[0]);
    let path = match source.cached_command(&cmd_var) {
        Some(path) => path,
        None => {
            let cwd = crate::source::current_dir();
            let from_attrs = attr_names.iter().find_map(|name| {
                let value = source.attr_inherit(&[name], Inherit::Document)?;
                let path = expand_path(&value, &cwd);
                host.is_executable(&path).then_some(path)
            });
            let path = from_attrs.or_else(|| cmd_names.iter().find_map(|name| host.which(name)));
            source.cache_command(cmd_var, path.clone());
            path
        }
    };
    if path.is_none() && find.raise {
        let names: Vec<String> = cmd_names.iter().map(|c| format!("'{c}'")).collect();
        return Err(format!(
            "Could not find the {} executable in PATH; add it to the PATH or specify its \
             location using the '{}' document attribute",
            names.join(", "),
            attr_names[0]
        ));
    }
    Ok(path)
}

/// `Java.java`: `$JAVA_HOME/bin/java`, else `java` on `PATH`.
pub(crate) fn java(host: &dyn DiagramHost) -> Result<String, String> {
    host.env("JAVA_HOME")
        .map(|home| expand_path("bin/java", &home))
        .filter(|java| host.is_executable(java))
        .or_else(|| host.which("java"))
        .ok_or_else(|| "Could not find Java executable".to_owned())
}

/// A string option's value (`nil` and booleans are `None`).
pub(crate) fn option<'o>(options: &'o Options, name: &str) -> Option<&'o str> {
    options.iter().find_map(|(key, value)| match value {
        OptValue::Str(value) if *key == name => Some(value.as_str()),
        _ => None,
    })
}

/// Whether an option is truthy in Ruby (any string or `true`).
pub(crate) fn truthy(options: &Options, name: &str) -> bool {
    options.iter().any(|(key, value)| {
        *key == name && matches!(value, OptValue::Str(_) | OptValue::Bool(true))
    })
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `Cli.run`: a non-zero exit is `"<tool> failed: <stderr or stdout>"`.
pub(crate) fn run(
    host: &dyn DiagramHost,
    command: &Command,
) -> Result<crate::host::Output, String> {
    let output = host
        .run(command)
        .map_err(|e| format!("{}: {e}", command.program))?;
    if output.status != 0 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "{} failed: {}",
            basename(&command.program),
            if stderr.is_empty() { stdout } else { stderr }
        ));
    }
    Ok(output)
}

/// What a `CliGenerator` block returns: the command and, for tools that
/// pick their own output name, the file to read instead (`:out_file`).
pub(crate) struct Gen {
    pub command: Command,
    pub out_file: Option<String>,
}

impl From<Command> for Gen {
    fn from(command: Command) -> Self {
        Self {
            command,
            out_file: None,
        }
    }
}

/// `generate`: run the tool and read its result (stdout or the target
/// file, which `out_file` replaces); the result must not be empty.
fn generate(host: &dyn DiagramHost, gen: Gen, target: Option<&str>) -> Result<Vec<u8>, String> {
    let output = run(host, &gen.command)?;
    let data = match target {
        None => output.stdout.clone(),
        Some(target) => match gen.out_file.as_deref() {
            // `read_result` renames the tool's file to the target.
            Some(out_file) if host.exists(out_file) => {
                let data = host.read(out_file).unwrap_or_default();
                host.remove(out_file);
                data
            }
            Some(_) => Vec::new(),
            None if host.exists(target) => host.read(target).unwrap_or_default(),
            None => Vec::new(),
        },
    };
    if data.is_empty() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{} failed: {}",
            gen.command.program,
            if stdout.is_empty() { stderr } else { stdout }
        ));
    }
    Ok(data)
}

static TEMP: AtomicU64 = AtomicU64::new(0);

/// A fresh temporary path named like Ruby's `Tempfile.new([prefix,
/// suffix])` (`Dir::Tmpname`: `<prefix><YYYYMMDD>-<pid>-<base36><suffix>`).
pub(crate) fn temp_path(host: &dyn DiagramHost, prefix: &str, suffix: &str) -> String {
    let n = TEMP.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (year, month, day) = civil_date(now.as_secs() / 86_400);
    let mut random = u64::from(now.subsec_nanos()) ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    random &= 0xffff_ffff;
    let mut base36 = String::new();
    loop {
        base36.insert(0, char::from_digit((random % 36) as u32, 36).unwrap_or('0'));
        random /= 36;
        if random == 0 {
            break;
        }
    }
    host.temp_dir()
        .join(format!(
            "{prefix}{year:04}{month:02}{day:02}-{}-{base36}{suffix}",
            std::process::id()
        ))
        .to_string_lossy()
        .into_owned()
}

/// The proleptic Gregorian date of a day count since 1970-01-01.
fn civil_date(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `generate_stdin`: the code on stdin, the result in a temporary file the
/// arguments name.
pub(crate) fn generate_stdin<G: Into<Gen>>(
    host: &dyn DiagramHost,
    tool: &str,
    format: &str,
    code: &[u8],
    build: impl FnOnce(&str) -> G,
) -> Result<Vec<u8>, String> {
    let target = temp_path(host, basename(tool), &format!(".{format}"));
    let result = generate_stdin_file(host, code, &target, || build(&target));
    host.remove(&target);
    result
}

/// `generate_stdin_file`: the code on stdin, the result in `target`.
pub(crate) fn generate_stdin_file<G: Into<Gen>>(
    host: &dyn DiagramHost,
    code: &[u8],
    target: &str,
    build: impl FnOnce() -> G,
) -> Result<Vec<u8>, String> {
    let mut gen = build().into();
    gen.command.stdin = Some(code.to_vec());
    generate(host, gen, Some(target))
}

/// `generate_stdin_stdout`: the code on stdin, the result on stdout.
pub(crate) fn generate_stdin_stdout(
    host: &dyn DiagramHost,
    code: &[u8],
    mut command: Command,
) -> Result<Vec<u8>, String> {
    command.stdin = Some(code.to_vec());
    generate(host, command.into(), None)
}

/// `generate_file`: the code in a temporary input file, the result in a
/// temporary output file, both named by the arguments.
pub(crate) fn generate_file<G: Into<Gen>>(
    host: &dyn DiagramHost,
    tool: &str,
    input_ext: &str,
    output_ext: &str,
    code: &[u8],
    build: impl FnOnce(&str, &str) -> Result<G, String>,
) -> Result<Vec<u8>, String> {
    let input = temp_path(host, basename(tool), &format!(".{input_ext}"));
    host.write(&input, code)
        .map_err(|e| format!("{input}: {e}"))?;
    let target = temp_path(host, basename(tool), &format!(".{output_ext}"));
    let result = build(&input, &target).and_then(|gen| generate(host, gen.into(), Some(&target)));
    host.remove(&target);
    host.remove(&input);
    host.remove(&format!("{input}.json"));
    result
}

/// `generate_file_stdout`: the code in a temporary input file, the result
/// on stdout.
pub(crate) fn generate_file_stdout(
    host: &dyn DiagramHost,
    tool: &str,
    input_ext: &str,
    code: &[u8],
    build: impl FnOnce(&str) -> Command,
) -> Result<Vec<u8>, String> {
    let input = temp_path(host, basename(tool), &format!(".{input_ext}"));
    host.write(&input, code)
        .map_err(|e| format!("{input}: {e}"))?;
    let result = generate(host, build(&input).into(), None);
    host.remove(&input);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_names_follow_rubys_tmpname() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(20_762), (2026, 11, 5));
        assert_eq!(civil_date(11_016), (2000, 2, 29));
        struct Tmp;
        impl DiagramHost for Tmp {
            fn run(&self, _: &Command) -> Result<crate::host::Output, String> {
                unreachable!()
            }
            fn which(&self, _: &str) -> Option<String> {
                None
            }
            fn is_executable(&self, _: &str) -> bool {
                false
            }
            fn temp_dir(&self) -> std::path::PathBuf {
                "/t".into()
            }
            fn read(&self, _: &str) -> std::io::Result<Vec<u8>> {
                unreachable!()
            }
            fn write(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
                unreachable!()
            }
            fn exists(&self, _: &str) -> bool {
                false
            }
            fn modified(&self, _: &str) -> Option<std::time::SystemTime> {
                None
            }
            fn remove(&self, _: &str) {}
            fn link(&self, _: &str, _: &str) -> std::io::Result<()> {
                unreachable!()
            }
            fn env(&self, _: &str) -> Option<String> {
                None
            }
        }
        let path = temp_path(&Tmp, "d2", ".svg");
        let name = path
            .strip_prefix("/t/d2")
            .unwrap()
            .strip_suffix(".svg")
            .unwrap();
        let parts: Vec<&str> = name.split('-').collect();
        assert_eq!(parts.len(), 3, "{path}");
        assert_eq!(parts[0].len(), 8);
        assert!(parts[0].bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(parts[1], std::process::id().to_string());
        assert!(parts[2]
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()));
    }

    #[test]
    fn options_dump_like_ruby_json() {
        let options: Options = vec![
            ("layout", OptValue::Null),
            ("size_limit", OptValue::Str("4096".into())),
            ("smetana", OptValue::Bool(true)),
            ("css", OptValue::Str("a\"b".into())),
        ];
        assert_eq!(
            options_json(&options),
            r#"{"layout":null,"size_limit":"4096","smetana":true,"css":"a\"b"}"#
        );
    }
}
