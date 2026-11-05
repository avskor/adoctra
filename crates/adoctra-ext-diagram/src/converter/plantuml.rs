//! `PlantUmlConverter` (`UmlConverter` for `plantuml`, `SaltConverter` for
//! `salt`) and `PlantUMLPreprocessedSource`.
//!
//! Ruby runs a native PlantUML (`plantuml-native` attribute,
//! `plantuml-full` or `plantuml-headless` on `PATH`) with `-pipe`, or else
//! the PlantUML JARs (`DIAGRAM_PLANTUML_CLASSPATH`) behind its Java server.
//! adoctra runs the same command line natively, or through
//! `java -cp <classpath> net.sourceforge.plantuml.Run` for the JARs, and as a
//! last resort a `plantuml` command on `PATH`.

use super::{find_command, generate_stdin_stdout, Converter, Find, OptValue, Options};
use crate::host::{Command, DiagramHost};
use crate::source::{expand_path, Source};

pub(crate) struct PlantUml {
    /// `uml` or `salt` (`@start<tag>`).
    pub tag: &'static str,
}

/// How PlantUML is run.
enum Tool {
    /// Ruby's native PlantUML (`plantuml-native`, `plantuml-full`,
    /// `plantuml-headless`).
    Native(String),
    /// A `plantuml` command on `PATH` standing in for Ruby's JARs.
    Command(String),
    /// `java` with these leading arguments.
    Java(String, Vec<String>),
}

impl Tool {
    fn command(&self, args: Vec<String>) -> Command {
        match self {
            Tool::Native(program) | Tool::Command(program) => Command {
                program: program.clone(),
                args,
                ..Command::default()
            },
            Tool::Java(java, leading) => Command {
                program: java.clone(),
                args: leading.iter().cloned().chain(args).collect(),
                ..Command::default()
            },
        }
    }
}

const NOT_FOUND: &str = "Could not load PlantUML. Either require 'asciidoctor-diagram-plantuml' \
    or specify the location of the PlantUML JAR(s) using the 'DIAGRAM_PLANTUML_CLASSPATH' \
    environment variable. Alternatively a PlantUML binary can be provided (plantuml-native in \
    $PATH).";

fn tool(source: &Source<'_, '_>, host: &dyn DiagramHost) -> Result<Tool, String> {
    let native = find_command(
        source,
        host,
        Find {
            attrs: Some(&["plantuml-native"]),
            alt_cmds: &["plantuml-headless"],
            raise: false,
            ..Find::new("plantuml-full")
        },
    )?;
    if let Some(native) = native {
        return Ok(Tool::Native(native));
    }
    if let Some(classpath) = host.env("DIAGRAM_PLANTUML_CLASSPATH") {
        let java = super::java(host)?;
        return Ok(Tool::Java(
            java,
            vec![
                "-Djava.awt.headless=true".to_owned(),
                "-cp".to_owned(),
                classpath,
                "net.sourceforge.plantuml.Run".to_owned(),
            ],
        ));
    }
    host.which("plantuml")
        .map(Tool::Command)
        .ok_or_else(|| NOT_FOUND.to_owned())
}

impl PlantUml {
    fn wrapped(&self, source: &Source<'_, '_>) -> String {
        let code = source.loaded_code();
        if code.contains("@start") && code.contains("@end") {
            code.to_owned()
        } else {
            format!("@start{tag}\n{code}\n@end{tag}", tag = self.tag)
        }
    }

    fn theme(source: &Source<'_, '_>) -> Option<String> {
        source.attr("theme")
    }

    /// `add_common_args`.
    fn common_args(source: &Source<'_, '_>, args: &mut Vec<String>) {
        let base_dir = expand_path(&source.base_dir(), &crate::source::current_dir());
        args.push("-filedir".to_owned());
        args.push(base_dir.clone());
        if let Some(config) = source
            .attr_document("plantumlconfig")
            .or_else(|| source.attr("config"))
        {
            args.push("-config".to_owned());
            args.push(expand_path(&config, &base_dir));
        }
        if let Some(include) = source.attr("includedir") {
            args.push(format!(
                "-Dplantuml.include.path={}",
                expand_path(&include, &base_dir)
            ));
        }
    }

    fn theme_args(theme: Option<String>, args: &mut Vec<String>) {
        if let Some(theme) = theme {
            args.push("-theme".to_owned());
            args.push(theme);
        }
    }

    /// `preprocess_native` (its common arguments appear twice, as in Ruby).
    fn preprocess(
        source: &Source<'_, '_>,
        tool: &Tool,
        code: &str,
        host: &dyn DiagramHost,
    ) -> Result<String, String> {
        let mut args = vec![
            "-pipe".to_owned(),
            "-preproc".to_owned(),
            "-failfast2".to_owned(),
            "-stdrpt:1".to_owned(),
        ];
        // Ruby's native path passes the theme twice; its default path (the
        // Java server's preprocessor) applies it only when rendering, so the
        // JARs preprocess without it.
        let native = matches!(tool, Tool::Native(_));
        Self::common_args(source, &mut args);
        if native {
            Self::theme_args(Self::theme(source), &mut args);
        }
        Self::common_args(source, &mut args);
        if native {
            Self::theme_args(Self::theme(source), &mut args);
        }
        let output = generate_stdin_stdout(host, code.as_bytes(), tool.command(args))?;
        // Ruby's default (its Java server) returns the preprocessed text
        // without the final line break the command line prints.
        let text = String::from_utf8_lossy(&output);
        Ok(text.trim_end_matches(['\n', '\r']).to_owned())
    }
}

impl Converter for PlantUml {
    fn supported_formats(&self) -> &'static [&'static str] {
        &["png", "svg", "txt", "atxt", "utxt"]
    }

    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        let mut options = vec![(
            "size_limit",
            OptValue::Str(
                source
                    .attr("size-limit")
                    .unwrap_or_else(|| "4096".to_owned()),
            ),
        )];
        if source.opt("smetana") {
            options.push(("smetana", OptValue::Bool(true)));
        }
        if let Some(theme) = source.attr("theme") {
            options.push(("theme", OptValue::Str(theme)));
        }
        if source.opt("debug") {
            options.push(("debug", OptValue::Bool(true)));
        }
        Ok(options)
    }

    fn code(&self, source: &Source<'_, '_>, host: &dyn DiagramHost) -> Result<String, String> {
        let code = self.wrapped(source);
        if source.attr("preprocess").as_deref().unwrap_or("true") != "true" {
            return Ok(code);
        }
        let tool = tool(source, host)?;
        Self::preprocess(source, &tool, &code, host)
    }

    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = tool(source, host)?;
        let code = self.code(source, host)?;
        let flag = match format {
            "png" => "-tpng",
            "svg" => "-tsvg",
            "txt" | "utxt" => "-tutxt",
            "atxt" => "-ttxt",
            other => return Err(format!("Unsupported format: {other}")),
        };
        let mut args = vec![flag.to_owned()];
        Self::common_args(source, &mut args);
        let option = |name: &str| options.iter().find(|(key, _)| *key == name).map(|(_, v)| v);
        if let Some(OptValue::Str(theme)) = option("theme") {
            Self::theme_args(Some(theme.clone()), &mut args);
        }
        let dot = find_command(
            source,
            host,
            Find {
                alt_attrs: &["graphvizdot"],
                raise: false,
                ..Find::new("dot")
            },
        )?;
        match dot {
            Some(dot) if option("smetana").is_none() => {
                args.push("-graphvizdot".to_owned());
                args.push(dot);
            }
            _ => args.push("-Playout=smetana".to_owned()),
        }
        let mut full = vec![
            "-pipe".to_owned(),
            "-failfast2".to_owned(),
            "-stdrpt:1".to_owned(),
        ];
        full.extend(args);
        let mut command = tool.command(full);
        if let Some(OptValue::Str(limit)) = option("size_limit") {
            command
                .env
                .insert("PLANTUML_LIMIT_SIZE".to_owned(), limit.clone());
        }
        generate_stdin_stdout(host, code.as_bytes(), command)
    }

    fn server_code(
        &self,
        source: &Source<'_, '_>,
        host: &dyn DiagramHost,
    ) -> Result<String, String> {
        self.code(source, host)
    }
}
