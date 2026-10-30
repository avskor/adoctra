//! Rouge's token types (`Rouge::Token`), generated in definition order.

pub use crate::generated::tokens::*;
use crate::generated::tokens::{PARENTS, TOKENS};

/// A token type; the index into Rouge's token list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token(pub(crate) u16);

impl Token {
    /// The dotted name (`Name.Builtin.Pseudo`).
    pub fn qualname(self) -> &'static str {
        TOKENS[self.0 as usize].0
    }

    /// The CSS class Rouge's HTML formatter uses (empty for `Text`).
    pub fn shortname(self) -> &'static str {
        TOKENS[self.0 as usize].1
    }

    /// The token type named `qualname` (`Token[...]`).
    pub fn named(qualname: &str) -> Option<Token> {
        TOKENS
            .iter()
            .position(|(name, _)| *name == qualname)
            .map(|index| Token(index as u16))
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }

    /// The position in Rouge's token definition order.
    pub fn id(self) -> u16 {
        self.0
    }

    /// The parent token type, `None` for a root.
    pub fn parent(self) -> Option<Token> {
        let parent = PARENTS[self.0 as usize];
        (parent != self.0).then_some(Token(parent))
    }
}
