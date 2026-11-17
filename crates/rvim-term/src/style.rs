//! Cell styles and SGR encoding (reference: highlight.c / term.c).

/// Terminal colour. 256-colour and truecolour are both representable; whether
/// they render depends on the terminal (M4 emits them, `termguicolors` flips the
/// default palette selection in M6 when syntax highlighting lands).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    /// xterm palette index (0-15 system, 16-255 cube/greyscale).
    Ansi(u8),
    Rgb(u8, u8, u8),
}

/// Attributes of one screen cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub underline: bool,
    pub reverse: bool,
}

impl Style {
    pub const NORMAL: Style = Style {
        fg: Color::Default,
        bg: Color::Default,
        bold: false,
        underline: false,
        reverse: false,
    };

    pub const REVERSE: Style = Style {
        fg: Color::Default,
        bg: Color::Default,
        bold: false,
        underline: false,
        reverse: true,
    };

    pub const BOLD: Style = Style {
        fg: Color::Default,
        bg: Color::Default,
        bold: true,
        underline: false,
        reverse: false,
    };

    /// SGR escape sequence for this style (always starts from a reset).
    pub fn sgr(&self) -> String {
        let mut out = String::from("\u{1b}[0");
        if self.bold {
            out.push_str(";1");
        }
        if self.underline {
            out.push_str(";4");
        }
        if self.reverse {
            out.push_str(";7");
        }
        out.push_str(&color_sgr(self.fg, true));
        out.push_str(&color_sgr(self.bg, false));
        out.push('m');
        out
    }
}

fn color_sgr(color: Color, foreground: bool) -> String {
    match color {
        Color::Default => String::new(),
        Color::Ansi(n) if n < 8 => {
            let base = if foreground { 30 } else { 40 };
            format!(";{}", base + n)
        }
        Color::Ansi(n) if n < 16 => {
            let base = if foreground { 90 } else { 100 };
            format!(";{}", base + (n - 8))
        }
        Color::Ansi(n) => format!(";{};5;{}", if foreground { 38 } else { 48 }, n),
        Color::Rgb(r, g, b) => format!(";{};2;{};{};{}", if foreground { 38 } else { 48 }, r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_style_is_a_plain_reset() {
        assert_eq!(Style::NORMAL.sgr(), "\u{1b}[0m");
    }

    #[test]
    fn attributes_and_colors() {
        let style = Style {
            fg: Color::Ansi(1),
            bg: Color::Ansi(16),
            bold: true,
            ..Style::NORMAL
        };
        assert_eq!(style.sgr(), "\u{1b}[0;1;31;48;5;16m");
        let truecolor = Style {
            fg: Color::Rgb(1, 2, 3),
            ..Style::NORMAL
        };
        assert_eq!(truecolor.sgr(), "\u{1b}[0;38;2;1;2;3m");
        assert_eq!(Style::REVERSE.sgr(), "\u{1b}[0;7m");
    }
}
