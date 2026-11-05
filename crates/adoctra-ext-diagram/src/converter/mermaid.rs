//! `MermaidConverter` with mermaid-cli (`mmdc`). Ruby falls back to the
//! legacy `mermaid` + PhantomJS CLI; adoctra does not run it and reports
//! the `mmdc` error instead.

use super::{find_command, generate_file, Converter, Find, OptValue, Options};
use crate::host::{Command, DiagramHost};
use crate::source::Source;

pub(crate) struct Mermaid;

fn get<'o>(options: &'o Options, name: &str) -> Option<&'o str> {
    options.iter().find_map(|(key, value)| match value {
        OptValue::Str(value) if *key == name => Some(value.as_str()),
        _ => None,
    })
}

impl Converter for Mermaid {
    fn supported_formats(&self) -> &'static [&'static str] {
        &["png", "svg"]
    }

    fn native_scaling(&self) -> bool {
        true
    }

    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        let scale = source.attr("scale");
        if let Some(scale) = &scale {
            if scale.is_empty() || !scale.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!(
                    "Mermaid only supports integer scale factors: {scale}"
                ));
            }
        }
        Ok(vec![
            ("css", source.attr("css").into()),
            (
                "gantt_config",
                source.attr_any(&["ganttconfig", "gantt-config"]).into(),
            ),
            (
                "seq_config",
                source
                    .attr_any(&["sequenceconfig", "sequence-config"])
                    .into(),
            ),
            ("width", source.attr("width").into()),
            ("height", source.attr("height").into()),
            ("scale", scale.into()),
            ("theme", source.attr("theme").into()),
            ("background", source.attr("background").into()),
            ("config", source.attr("config").into()),
            (
                "puppeteer_config",
                source
                    .attr_any(&["puppeteerconfig", "puppeteer-config"])
                    .into(),
            ),
        ])
    }

    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let base_dir = source.base_dir();
        let resolve = |path: &str| crate::source::expand_path(path, &base_dir);
        let mmdc = find_command(source, host, Find::new("mmdc"))?.unwrap_or_default();
        let code = self.code(source, host)?;
        generate_file(
            host,
            &mmdc,
            "mmd",
            format,
            code.as_bytes(),
            |input, output| {
                let mut args = vec![
                    "-i".to_owned(),
                    input.to_owned(),
                    "-o".to_owned(),
                    output.to_owned(),
                ];
                if let Some(css) = get(options, "css") {
                    args.extend(["--cssFile".to_owned(), resolve(css)]);
                }
                if let Some(theme) = get(options, "theme") {
                    args.extend(["--theme".to_owned(), theme.to_owned()]);
                }
                if let Some(width) = get(options, "width") {
                    args.extend(["--width".to_owned(), width.to_owned()]);
                }
                if let Some(height) = get(options, "height") {
                    args.extend(["--height".to_owned(), height.to_owned()]);
                }
                if let Some(scale) = get(options, "scale") {
                    args.extend(["--scale".to_owned(), scale.to_owned()]);
                }
                if let Some(background) = get(options, "background") {
                    let background = if background.starts_with('#') {
                        background.to_owned()
                    } else {
                        format!("#{background}")
                    };
                    args.extend(["--backgroundColor".to_owned(), background]);
                }
                if let Some(config) = get(options, "config") {
                    args.extend(["--configFile".to_owned(), resolve(config)]);
                } else if get(options, "gantt_config").is_some()
                    || get(options, "seq_config").is_some()
                {
                    let mut parts = Vec::new();
                    if let Some(gantt) = get(options, "gantt_config") {
                        let text = host
                            .read(&resolve(gantt))
                            .map_err(|e| format!("{gantt}: {e}"))?;
                        parts.push(format!("\"gantt\": {}", String::from_utf8_lossy(&text)));
                    }
                    if let Some(sequence) = get(options, "seq_config") {
                        let text = host
                            .read(&resolve(sequence))
                            .map_err(|e| format!("{sequence}: {e}"))?;
                        parts.push(format!("\"sequence\": {}", String::from_utf8_lossy(&text)));
                    }
                    let config = format!("{input}.json");
                    host.write(&config, format!("{{{}}}", parts.join(",")).as_bytes())
                        .map_err(|e| format!("{config}: {e}"))?;
                    args.extend(["--configFile".to_owned(), config]);
                }
                if let Some(puppeteer) = get(options, "puppeteer_config") {
                    args.extend(["--puppeteerConfigFile".to_owned(), resolve(puppeteer)]);
                }
                let mut command = Command {
                    program: mmdc.clone(),
                    args,
                    chdir: Some(base_dir.clone()),
                    ..Command::default()
                };
                command.env.insert(
                    "NODE_OPTIONS".to_owned(),
                    "--unhandled-rejections=strict".to_owned(),
                );
                Ok(command)
            },
        )
    }
}
