//! Styled terminal output a plugin can emit without linking a colour crate.

use super::*;

pub(super) const ANSI_RESET: &str = "\u{1b}[0m";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TextStyle {
    Heading,
    Key,
    Success,
    Warning,
    Error,
    Muted,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TextFormatter {
    pub(super) color: bool,
}

impl TextFormatter {
    pub fn stdout() -> Self {
        Self::automatic(io::stdout().is_terminal())
    }

    pub fn stderr() -> Self {
        Self::automatic(io::stderr().is_terminal())
    }

    pub const fn with_color(color: bool) -> Self {
        Self { color }
    }

    pub fn paint(self, style: TextStyle, value: impl Display) -> String {
        if !self.color {
            return value.to_string();
        }

        format!("{}{value}{ANSI_RESET}", ansi_prefix(style))
    }

    pub(super) fn automatic(is_terminal: bool) -> Self {
        Self::with_color(color_enabled(
            is_terminal,
            std::env::var_os("NO_COLOR").is_some(),
        ))
    }
}
