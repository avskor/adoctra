//! The converters Ruby runs inside its Java command server (ditaa, JSyntrax,
//! Structurizr), here through each tool's own command line with the same
//! library calls: ditaa's `CommandLineConverter`, JSyntrax's `Main` and the
//! Structurizr CLI's `export`. Rendering matches; error texts are the
//! tools' rather than the server's.

use super::{generate_file, java, option, temp_path, Converter, Options, Syntrax};
use crate::host::{Command, DiagramHost};
use crate::source::Source;

/// The JVM options of Ruby's `CommandServer`.
const JVM_OPTIONS: [&str; 3] = [
    "-Djava.awt.headless=true",
    "-Djava.net.useSystemProxies=true",
    "-Dfile.encoding=UTF-8",
];

fn split_path(value: &str) -> Vec<String> {
    value.split(':').map(str::to_owned).collect()
}

/// The JSyntrax JARs (`DIAGRAM_JSYNTRAX_CLASSPATH`, else the `lib` JARs of
/// `DIAGRAM_JSYNTRAX_HOME`), as Ruby's `JSYNTRAX_JARS`.
fn jsyntrax_jars(host: &dyn DiagramHost) -> Option<Vec<String>> {
    if let Some(classpath) = host.env("DIAGRAM_JSYNTRAX_CLASSPATH") {
        return Some(split_path(&classpath));
    }
    let home = host.env("DIAGRAM_JSYNTRAX_HOME")?;
    Some(lib_jars(host, &home))
}

/// The Structurizr JARs (`DIAGRAM_STRUCTURIZR_CLASSPATH`, else the `lib`
/// JARs of `DIAGRAM_STRUCTURIZRCLI_HOME`).
fn structurizr_jars(host: &dyn DiagramHost) -> Option<Vec<String>> {
    if let Some(classpath) = host.env("DIAGRAM_STRUCTURIZR_CLASSPATH") {
        return Some(split_path(&classpath));
    }
    let home = host.env("DIAGRAM_STRUCTURIZRCLI_HOME")?;
    Some(lib_jars(host, &home))
}

fn lib_jars(host: &dyn DiagramHost, home: &str) -> Vec<String> {
    let lib = crate::source::expand_path("lib", home);
    let mut jars: Vec<String> = host
        .list_dir(&lib)
        .into_iter()
        .filter(|name| name.ends_with(".jar"))
        .map(|name| format!("{lib}/{name}"))
        .collect();
    jars.sort();
    jars
}

/// Ruby's `Java.classpath`: one JVM holds the JARs of every Java-based
/// type in `require` order (ditaa, PlantUML, Structurizr, JSyntrax), so a
/// tool sees the others' JARs (JSyntrax's PNG output needs PlantUML's
/// Batik). Each item must exist, as the server checks.
fn classpath(host: &dyn DiagramHost) -> Result<String, String> {
    let mut items = Vec::new();
    for name in ["DIAGRAM_DITAA_CLASSPATH", "DIAGRAM_PLANTUML_CLASSPATH"] {
        if let Some(value) = host.env(name) {
            items.extend(split_path(&value));
        }
    }
    items.extend(structurizr_jars(host).unwrap_or_default());
    items.extend(jsyntrax_jars(host).unwrap_or_default());
    if let Some(missing) = items.iter().find(|item| !host.exists(item)) {
        return Err(format!("Classpath item {missing} does not exist"));
    }
    Ok(items.join(":"))
}

/// `java <server options> -cp <classpath> <main> <args>`.
fn java_command(host: &dyn DiagramHost, main: &str, args: Vec<String>) -> Result<Command, String> {
    let classpath = classpath(host)?;
    let mut all: Vec<String> = JVM_OPTIONS.iter().map(|o| (*o).to_owned()).collect();
    all.extend(["-cp".to_owned(), classpath, main.to_owned()]);
    all.extend(args);
    Ok(Command {
        program: java(host)?,
        args: all,
        ..Command::default()
    })
}

/// The Java exception a tool's command line printed (`Exception in thread
/// "main" <class>: <message>` or a stack trace's `<class>: <message>`
/// line, message lines up to the first frame): its class and message.
fn java_exception(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    let (class, first) = lines.by_ref().find_map(|line| {
        let line = line
            .strip_prefix("Exception in thread \"main\" ")
            .unwrap_or(line);
        let (class, message) = line.split_once(": ")?;
        let simple = class.rsplit('.').next().unwrap_or(class);
        (class.contains('.')
            && !class.contains(' ')
            && (simple.ends_with("Exception") || simple.ends_with("Error")))
        .then(|| (class.to_owned(), message.to_owned()))
    })?;
    let mut message = first;
    for line in lines.take_while(|line| !line.starts_with("\tat ")) {
        message.push('\n');
        message.push_str(line);
    }
    Some((class, message))
}

/// Run a Java tool; a failure becomes the server's error text
/// (`Java.create_error`: `<prefix>: <msg>`), where `msg` is the
/// exception's message, or its `toString` when the adapter wraps it
/// (`wrapped`).
fn run_java(
    host: &dyn DiagramHost,
    command: &Command,
    prefix: &str,
    wrapped: bool,
) -> Result<crate::host::Output, String> {
    let output = host
        .run(command)
        .map_err(|e| format!("{}: {e}", command.program))?;
    if output.status == 0 {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    match java_exception(&stderr) {
        Some((class, message)) if wrapped => Err(format!("{prefix}: {class}: {message}")),
        Some((_, message)) => Err(format!("{prefix}: {message}")),
        None => super::run(host, command),
    }
}

/// The server request's `X-Options` as the Java side reads them: the
/// flags joined with spaces, then `split(" ")` (a value with a space
/// becomes several arguments, as in Ruby).
fn x_options(flags: &[String]) -> Vec<String> {
    flags
        .join(" ")
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `DitaaConverter`.
pub(crate) struct Ditaa;

const DITAA_OPTIONS: [&str; 11] = [
    "scale",
    "tabs",
    "background",
    "antialias",
    "separation",
    "round-corners",
    "shadows",
    "debug",
    "fixed-slope",
    "transparent",
    "bullet-characters",
];

impl Converter for Ditaa {
    fn supported_formats(&self) -> &'static [&'static str] {
        &["png", "svg", "txt"]
    }
    fn native_scaling(&self) -> bool {
        true
    }
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(DITAA_OPTIONS
            .iter()
            .zip([
                "scale",
                "tabs",
                "background",
                "antialias",
                "separation",
                "round_corners",
                "shadows",
                "debug",
                "fixed_slope",
                "transparent",
                "bullet_characters",
            ])
            .map(|(attr, key)| {
                let value = source.attr(attr).or_else(|| {
                    source.attr_inherit(&[attr], crate::source::Inherit::Prefix("ditaa-option"))
                });
                (key, value.into())
            })
            .collect())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let code = self.code(source, host)?;
        if format == "txt" {
            return Ok(code.into_bytes());
        }
        if host.env("DIAGRAM_DITAA_CLASSPATH").is_none() {
            return Err(
                "Could not load Ditaa. Either require 'asciidoctor-diagram-ditaamini' or \
                        specify the location of the Ditaa JAR(s) using the \
                        'DIAGRAM_DITAA_CLASSPATH' environment variable."
                    .to_owned(),
            );
        }
        let mut flags = Vec::new();
        let value = |key| option(options, key);
        for (key, flag) in [
            ("scale", "--scale"),
            ("tabs", "--tabs"),
            ("background", "--background"),
        ] {
            if let Some(v) = value(key) {
                flags.extend([flag.to_owned(), v.to_owned()]);
            }
        }
        for (key, flag, when) in [
            ("antialias", "--no-antialias", "false"),
            ("separation", "--no-separation", "false"),
            ("round_corners", "--round-corners", "true"),
            ("shadows", "--no-shadows", "false"),
            ("debug", "--debug", "true"),
            ("fixed_slope", "--fixed-slope", "true"),
            ("transparent", "--transparent", "true"),
        ] {
            if value(key) == Some(when) {
                flags.push(flag.to_owned());
            }
        }
        if let Some(v) = value("bullet_characters") {
            flags.extend(["--bullet-characters".to_owned(), v.to_owned()]);
        }
        // The server adapter's arguments: the request charset, `--svg`
        // for SVG, then `X-Options`; stdin to stdout.
        let mut args = vec!["--encoding".to_owned(), "UTF-8".to_owned()];
        if format == "svg" {
            args.push("--svg".to_owned());
        }
        args.extend(x_options(&flags));
        args.extend(["-".to_owned(), "-".to_owned()]);
        let mut command = java_command(
            host,
            "org.stathissideris.ditaa.core.CommandLineConverter",
            args,
        )?;
        command.stdin = Some(code.into_bytes());
        let output = run_java(host, &command, "Ditaa image generation failed", false)?;
        if output.stdout.is_empty() {
            return Err(format!(
                "{} failed: {}",
                command.program,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(output.stdout)
    }
}

impl Syntrax {
    /// JSyntrax's `Main` with the JARs Ruby's server would load, when
    /// configured; `None` runs the `syntrax` command.
    pub(crate) fn jsyntrax(
        &self,
        format: &str,
        shared: &[String],
        code: &str,
        host: &dyn DiagramHost,
    ) -> Option<Result<Vec<u8>, String>> {
        jsyntrax_jars(host)?;
        let java = match java(host) {
            Ok(java) => java,
            Err(e) => return Some(Err(e)),
        };
        Some(
            generate_file(
                host,
                &java,
                "spec",
                format,
                code.as_bytes(),
                |input, output| {
                    let mut args = vec![
                        "-i".to_owned(),
                        input.to_owned(),
                        "-o".to_owned(),
                        output.to_owned(),
                    ];
                    args.extend(x_options(shared));
                    java_command(host, "org.atpfivt.jsyntrax.Main", args)
                },
            )
            .map_err(|error| {
                // A script JSyntrax cannot run: `generateSVG` returns null, which
                // the server's adapter dereferences.
                if error.contains("Something is wrong with input script") {
                    "JSyntrax code generation failed: Cannot invoke \
                 \"String.getBytes(java.nio.charset.Charset)\" because \"result\" is null"
                        .to_owned()
                } else {
                    error
                }
            }),
        )
    }
}

/// `StructurizrConverter`: the DSL (or JSON) workspace as the text of one
/// view for the renderer, through `structurizr-cli export`.
pub(crate) struct Structurizr;

const PREFIX: &str = "Structurizr code generation failed";

/// The default renderer (`Renderers::DEFAULT_RENDERER`).
pub(crate) const DEFAULT_RENDERER: &str = "plantuml-c4";

impl Converter for Structurizr {
    fn supported_formats(&self) -> &'static [&'static str] {
        &["txt"]
    }
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("view", source.attr("view").into()),
            (
                "renderer",
                super::OptValue::Str(
                    source
                        .attr("renderer")
                        .unwrap_or_else(|| DEFAULT_RENDERER.to_owned()),
                ),
            ),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        _: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        if structurizr_jars(host).is_none() {
            return Err(
                "Could not load Structurizr. Specify the location of the Structurizr \
                        JAR(s) using the 'DIAGRAM_STRUCTURIZRCLI_HOME' or \
                        DIAGRAM_STRUCTURIZR_CLASSPATH' environment variable."
                    .to_owned(),
            );
        }
        let renderer = option(options, "renderer").unwrap_or(DEFAULT_RENDERER);
        // `Renderers.mime_type`, then the exporter the server picks.
        let (format, extension) = match renderer {
            "plantuml-c4" => ("plantuml/c4plantuml", "puml"),
            "plantuml" => ("plantuml/structurizr", "puml"),
            "mermaid" => ("mermaid", "mmd"),
            "graphviz" => ("dot", "dot"),
            "d2" => return Err(format!("{PREFIX}: D2 exporter not available")),
            other => return Err(format!("Unsupported renderer: '{other}'")),
        };
        let code = self.code(source, host)?;
        // The server parses DSL when the code starts with `workspace`
        // (case-insensitive), else JSON.
        let dsl = code
            .trim_start()
            .get(..9)
            .is_some_and(|start| start.eq_ignore_ascii_case("workspace"));
        // Ruby's server parses the code as if it were a file in the base
        // directory (`X-Structurizr-IncludeDir`), so relative `!include`
        // and `!docs` resolve there: the workspace file is written there
        // (hidden, removed after the export), the views to a temporary
        // directory.
        let base_dir = source.base_dir();
        let name = temp_path(host, ".structurizr", if dsl { ".dsl" } else { ".json" });
        let workspace = format!("{base_dir}/{}", name.rsplit('/').next().unwrap_or(&name));
        let dir = temp_path(host, "structurizr", "");
        let out = format!("{dir}/out");
        host.write(&workspace, code.as_bytes())
            .map_err(|e| format!("{workspace}: {e}"))?;
        let result = (|| {
            let args = vec![
                "export".to_owned(),
                "-w".to_owned(),
                workspace.clone(),
                "-f".to_owned(),
                format.to_owned(),
                "-o".to_owned(),
                out.clone(),
            ];
            let mut command =
                java_command(host, "com.structurizr.cli.StructurizrCliApplication", args)?;
            command.chdir = Some(source.base_dir());
            // The adapter wraps exceptions (`IOException(e)`); its parser
            // reads the code as lines, so a message names no file.
            run_java(host, &command, PREFIX, true)
                .map_err(|e| e.replace(&format!(" of {workspace}:"), ":"))?;
            // One file per view, `structurizr-<key>.<ext>`: the requested
            // view, else the one with the smallest key (`findView`).
            let suffix = format!(".{extension}");
            let mut keys: Vec<String> = host
                .list_dir(&out)
                .into_iter()
                .filter_map(|name| {
                    name.strip_prefix("structurizr-")
                        .and_then(|rest| rest.strip_suffix(&suffix))
                        .map(str::to_owned)
                })
                .collect();
            keys.sort();
            let key = match option(options, "view") {
                Some(view) => keys.iter().find(|key| *key == view),
                None => keys.first(),
            }
            // `getViewWithKey` is null and `exportView` dereferences it.
            .ok_or_else(|| {
                format!(
                    "{PREFIX}: java.lang.NullPointerException: Cannot invoke \
                     \"Object.getClass()\" because \"view\" is null"
                )
            })?;
            host.read(&format!("{out}/structurizr-{key}{suffix}"))
                .map_err(|e| e.to_string())
        })();
        for name in host.list_dir(&out) {
            host.remove(&format!("{out}/{name}"));
        }
        host.remove_dir(&out);
        host.remove(&workspace);
        host.remove_dir(&dir);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_exceptions_are_read_from_the_tool_output() {
        assert_eq!(
            java_exception(
                "Exception in thread \"main\" java.lang.IllegalArgumentException: Bad \"zz\"\n\
                 \tat x.Y.z(Y.java:1)\n"
            ),
            Some((
                "java.lang.IllegalArgumentException".to_owned(),
                "Bad \"zz\"".to_owned()
            ))
        );
        assert_eq!(
            java_exception("log\ncom.a.ParserException: line one\nline two\n\tat a.B.c()"),
            Some((
                "com.a.ParserException".to_owned(),
                "line one\nline two".to_owned()
            ))
        );
        assert_eq!(java_exception("plain failure: no class"), None);
    }

    #[test]
    fn x_options_split_like_the_java_side() {
        assert_eq!(
            x_options(&["--title".to_owned(), "Two words".to_owned()]),
            ["--title", "Two", "words"]
        );
        assert!(x_options(&[]).is_empty());
    }
}
