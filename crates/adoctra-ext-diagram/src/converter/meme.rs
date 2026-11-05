//! `MemeConverter`: labels drawn over a background image with ImageMagick
//! (`magick convert`/`magick identify`, else `convert` and `identify`).

use super::{find_command, run, temp_path, Converter, Find, OptValue, Options};
use crate::host::{Command, DiagramHost};
use crate::source::Source;

pub(crate) struct Meme;

fn get<'o>(options: &'o Options, name: &str) -> Option<&'o str> {
    super::option(options, name)
}

/// ImageMagick's two commands: `magick <command>` or the separate tools.
struct Magick {
    convert: Vec<String>,
    identify: Vec<String>,
}

impl Magick {
    /// `Cli.run`, no working directory.
    fn run(
        host: &dyn DiagramHost,
        tool: &[String],
        args: Vec<String>,
    ) -> Result<crate::host::Output, String> {
        let command = Command {
            program: tool[0].clone(),
            args: tool[1..].iter().cloned().chain(args).collect(),
            ..Command::default()
        };
        run(host, &command)
    }
}

/// `calculate_height`: `<n>%` of the image height (integer division), or
/// `<n>`/`<n>px`.
fn calculate_height(total: u64, value: &str) -> Result<u64, String> {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if let Some(percent) = value.strip_suffix('%').filter(|p| digits(p)) {
        return Ok(total * percent.parse::<u64>().unwrap_or(0) / 100);
    }
    let pixels = value.strip_suffix("px").unwrap_or(value);
    if digits(pixels) {
        return Ok(pixels.parse().unwrap_or(0));
    }
    Err(format!("Invalid height value '{value}'"))
}

/// `prepare_label`: upcased unless `noupcase`, ` // ` as ImageMagick's
/// `\n`.
fn prepare_label(label: &str, noupcase: bool) -> String {
    let label = if noupcase {
        label.to_owned()
    } else {
        label.to_uppercase()
    };
    label.replace(" // ", "\\n")
}

impl Converter for Meme {
    fn supported_formats(&self) -> &'static [&'static str] {
        &["png", "gif"]
    }

    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        let background = source
            .attr("background")
            .ok_or("background attribute is required")?;
        let height = source
            .attr("height-fraction")
            .unwrap_or_else(|| "20%".to_owned());
        Ok(vec![
            ("bg_img", OptValue::Str(background)),
            ("top_label", source.attr("top").into()),
            ("bottom_label", source.attr("bottom").into()),
            (
                "fill_color",
                source.attr_any(&["fillcolor", "fill-color"]).into(),
            ),
            (
                "stroke_color",
                source.attr_any(&["strokecolor", "stroke-color"]).into(),
            ),
            (
                "stroke_width",
                source.attr_any(&["strokewidth", "stroke-width"]).into(),
            ),
            (
                "font",
                OptValue::Str(source.attr("font").unwrap_or_else(|| "Impact".to_owned())),
            ),
            ("height", OptValue::Str(height.clone())),
            (
                "top_height",
                OptValue::Str(source.attr("top-height").unwrap_or_else(|| height.clone())),
            ),
            (
                "bottom_height",
                OptValue::Str(source.attr("bottom-height").unwrap_or(height)),
            ),
            ("noupcase", source.attr("noupcase-option").into()),
            ("imagesdir", source.attr("imagesdir").into()),
        ])
    }

    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let magick = find_command(
            source,
            host,
            Find {
                raise: false,
                ..Find::new("magick")
            },
        )?;
        let tools = match magick {
            Some(magick) => Magick {
                convert: vec![magick.clone(), "convert".to_owned()],
                identify: vec![magick, "identify".to_owned()],
            },
            None => {
                let convert = find_command(source, host, Find::new("convert"))?.unwrap_or_default();
                let identify =
                    find_command(source, host, Find::new("identify"))?.unwrap_or_default();
                Magick {
                    convert: vec![convert],
                    identify: vec![identify],
                }
            }
        };
        let background = get(options, "bg_img").ok_or("background attribute is required")?;
        // `resolve_path(bg_img, imagesdir)`: from `imagesdir`, else the
        // document's base directory.
        let background = source.resolve_path(background, get(options, "imagesdir"));
        let fill = get(options, "fill_color").unwrap_or("white");
        let stroke = get(options, "stroke_color").unwrap_or("black");
        let stroke_width = get(options, "stroke_width").unwrap_or("2");
        let font = get(options, "font").unwrap_or("Impact");
        let noupcase = get(options, "noupcase").is_some();

        let output = Magick::run(
            host,
            &tools.identify,
            vec!["-format".to_owned(), "%w %h".to_owned(), background.clone()],
        )?;
        // `match(/(?<w>\d+) (?<h>\d+)/)`.
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let dimensions = text.match_indices(' ').find_map(|(at, _)| {
            let width: String = text[..at]
                .chars()
                .rev()
                .take_while(char::is_ascii_digit)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let height: String = text[at + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            (!width.is_empty() && !height.is_empty()).then(|| {
                (
                    width.parse::<u64>().unwrap_or(0),
                    height.parse::<u64>().unwrap_or(0),
                )
            })
        });
        let (width, height) = dimensions.ok_or("undefined method `[]' for nil")?;
        let top_height = calculate_height(height, get(options, "top_height").unwrap_or("20%"))?;
        let bottom_height =
            calculate_height(height, get(options, "bottom_height").unwrap_or("20%"))?;

        let mut temporaries = Vec::new();
        let mut label = |text: Option<&str>, label_height: u64, gravity: &str| {
            let Some(text) = text else {
                return Ok(None);
            };
            let path = temp_path(host, "meme", ".png");
            temporaries.push(path.clone());
            Magick::run(
                host,
                &tools.convert,
                vec![
                    "-background".to_owned(),
                    "none".to_owned(),
                    "-fill".to_owned(),
                    fill.to_owned(),
                    "-stroke".to_owned(),
                    stroke.to_owned(),
                    "-strokewidth".to_owned(),
                    stroke_width.to_owned(),
                    "-font".to_owned(),
                    font.to_owned(),
                    "-size".to_owned(),
                    format!("{width}x{label_height}"),
                    "-gravity".to_owned(),
                    gravity.to_owned(),
                    format!("label:{}", prepare_label(text, noupcase)),
                    path.clone(),
                ],
            )
            .map(|_| Some(path))
        };
        let result = (|| {
            let top = label(get(options, "top_label"), top_height, "north")?;
            let bottom = label(get(options, "bottom_label"), bottom_height, "south")?;
            let final_image = temp_path(host, "meme", &format!(".{format}"));
            let mut args = vec![background.clone()];
            if let Some(top) = top {
                args.extend([top, "-geometry".to_owned(), "+0+0".to_owned()]);
                args.push("-composite".to_owned());
            }
            if let Some(bottom) = bottom {
                args.extend([
                    bottom,
                    "-geometry".to_owned(),
                    // Ruby's integer difference, negative when the label is
                    // taller than the image.
                    format!("+0+{}", height as i64 - bottom_height as i64),
                    "-composite".to_owned(),
                ]);
            }
            args.push(final_image.clone());
            let result = Magick::run(host, &tools.convert, args)
                .and_then(|_| host.read(&final_image).map_err(|e| e.to_string()));
            host.remove(&final_image);
            result
        })();
        for path in temporaries {
            host.remove(&path);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_and_labels_follow_ruby() {
        assert_eq!(calculate_height(155, "20%"), Ok(31));
        assert_eq!(calculate_height(155, "40px"), Ok(40));
        assert_eq!(calculate_height(155, "12"), Ok(12));
        assert_eq!(
            calculate_height(155, "1.5em"),
            Err("Invalid height value '1.5em'".to_owned())
        );
        assert_eq!(prepare_label("one // two", false), "ONE\\nTWO");
        assert_eq!(prepare_label("straße", true), "straße");
        assert_eq!(prepare_label("straße", false), "STRASSE");
    }
}
