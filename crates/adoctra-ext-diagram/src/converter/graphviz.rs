//! `GraphvizConverter`: `dot -o<file> -T<format> [-K<layout>]` with the
//! code on stdin, run in the source's base directory.

use super::{find_command, generate_stdin, Converter, Find, OptValue, Options};
use crate::host::{Command, DiagramHost};
use crate::source::Source;

pub(crate) struct Graphviz;

impl Converter for Graphviz {
    fn supported_formats(&self) -> &'static [&'static str] {
        &["png", "pdf", "svg"]
    }

    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![("layout", OptValue::from(source.attr("layout")))])
    }

    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let dot = find_command(
            source,
            host,
            Find {
                alt_attrs: &["graphvizdot"],
                ..Find::new("dot")
            },
        )?
        .unwrap_or_default();
        let code = self.code(source, host)?;
        let layout = options.iter().find_map(|(key, value)| match (key, value) {
            (&"layout", OptValue::Str(layout)) => Some(layout.clone()),
            _ => None,
        });
        generate_stdin(host, &dot, format, code.as_bytes(), |output| {
            let mut args = vec![format!("-o{output}"), format!("-T{format}")];
            if let Some(layout) = layout {
                args.push(format!("-K{layout}"));
            }
            Command {
                program: dot.clone(),
                args,
                chdir: Some(source.base_dir()),
                ..Command::default()
            }
        })
    }
}
