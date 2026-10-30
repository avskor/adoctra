//! Rouge's themes as the Asciidoctor rouge adapter uses them: the class
//! mode stylesheet (`Theme#render scope: 'pre.rouge'`), the inline mode's
//! `<pre>` style and each token's inline rules.

use crate::generated::themes::THEMES;
use crate::token::Token;

#[derive(Debug)]
pub struct ThemeData {
    pub name: &'static str,
    pub stylesheet: &'static str,
    pub base_style: Option<&'static str>,
    pub(crate) inline: &'static [&'static str],
}

/// The adapter's default style.
pub const DEFAULT_THEME: &str = "github";

impl ThemeData {
    /// `Rouge::Theme.find`.
    pub fn find(name: &str) -> Option<&'static ThemeData> {
        THEMES.iter().find(|theme| theme.name == name)
    }

    /// The inline CSS rules for `token` (`style_for(token).rendered_rules`).
    pub fn inline_style(&self, token: Token) -> &'static str {
        self.inline[token.index()]
    }
}
