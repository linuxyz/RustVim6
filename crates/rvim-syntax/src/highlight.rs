//! Highlight groups and colour attributes (reference: highlight.c).

use std::collections::HashMap;

/// A colour as it appears in `:highlight` (`#RRGGBB` or a cterm index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HlColor {
    /// xterm palette index (0-255).
    Indexed(u8),
    /// 24-bit RGB (`guifg=#RRGGBB`).
    Rgb(u8, u8, u8),
}

/// The visual attributes of a highlight group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HlAttr {
    pub fg: Option<HlColor>,
    pub bg: Option<HlColor>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
}

impl HlAttr {
    /// Merge `other` on top of this attribute (later `:hi` arguments win).
    pub fn merge(&mut self, other: &HlAttr) {
        if other.fg.is_some() {
            self.fg = other.fg;
        }
        if other.bg.is_some() {
            self.bg = other.bg;
        }
        self.bold |= other.bold;
        self.italic |= other.italic;
        self.underline |= other.underline;
        self.reverse |= other.reverse;
    }
}

/// Named highlight groups plus `:hi link` aliases.
#[derive(Clone, Debug, Default)]
pub struct HighlightTable {
    attrs: HashMap<String, HlAttr>,
    cterm_attrs: HashMap<String, HlAttr>,
    links: HashMap<String, String>,
}

fn rgb(r: u8, g: u8, b: u8) -> Option<HlColor> {
    Some(HlColor::Rgb(r, g, b))
}

impl HighlightTable {
    /// A dark-background palette close to vim's default groups.
    pub fn with_defaults() -> Self {
        let mut table = Self::default();
        table.set(
            "Normal",
            HlAttr {
                fg: rgb(212, 212, 212),
                bg: rgb(30, 30, 30),
                ..HlAttr::default()
            },
        );
        table.set(
            "Comment",
            HlAttr {
                fg: rgb(106, 153, 85),
                italic: true,
                ..HlAttr::default()
            },
        );
        table.set(
            "Constant",
            HlAttr {
                fg: rgb(79, 193, 255),
                ..HlAttr::default()
            },
        );
        table.set(
            "String",
            HlAttr {
                fg: rgb(206, 145, 120),
                ..HlAttr::default()
            },
        );
        table.set(
            "Number",
            HlAttr {
                fg: rgb(181, 206, 168),
                ..HlAttr::default()
            },
        );
        table.set(
            "Boolean",
            HlAttr {
                fg: rgb(86, 156, 214),
                ..HlAttr::default()
            },
        );
        table.set(
            "Keyword",
            HlAttr {
                fg: rgb(86, 156, 214),
                ..HlAttr::default()
            },
        );
        table.set(
            "Statement",
            HlAttr {
                fg: rgb(197, 134, 192),
                ..HlAttr::default()
            },
        );
        table.set(
            "PreProc",
            HlAttr {
                fg: rgb(197, 134, 192),
                ..HlAttr::default()
            },
        );
        table.set(
            "Type",
            HlAttr {
                fg: rgb(78, 201, 176),
                ..HlAttr::default()
            },
        );
        table.set(
            "Identifier",
            HlAttr {
                fg: rgb(156, 220, 254),
                ..HlAttr::default()
            },
        );
        table.set(
            "Function",
            HlAttr {
                fg: rgb(220, 220, 170),
                ..HlAttr::default()
            },
        );
        table.set(
            "Special",
            HlAttr {
                fg: rgb(215, 186, 125),
                ..HlAttr::default()
            },
        );
        table.set(
            "Delimiter",
            HlAttr {
                fg: rgb(212, 212, 212),
                ..HlAttr::default()
            },
        );
        table.set(
            "Title",
            HlAttr {
                fg: rgb(220, 220, 170),
                bold: true,
                ..HlAttr::default()
            },
        );
        table.set(
            "Todo",
            HlAttr {
                fg: rgb(255, 255, 0),
                bold: true,
                ..HlAttr::default()
            },
        );
        table.set(
            "Error",
            HlAttr {
                fg: rgb(255, 255, 255),
                bg: rgb(163, 21, 21),
                ..HlAttr::default()
            },
        );
        table.set(
            "Underlined",
            HlAttr {
                underline: true,
                ..HlAttr::default()
            },
        );
        // Spell groups (highlight.c init_highlight): bad words are underlined,
        // capitalisation errors and rare words get their own colour.
        table.set(
            "SpellBad",
            HlAttr {
                fg: rgb(255, 106, 106),
                underline: true,
                ..HlAttr::default()
            },
        );
        table.set(
            "SpellCap",
            HlAttr {
                fg: rgb(255, 196, 0),
                underline: true,
                ..HlAttr::default()
            },
        );
        table.set(
            "SpellRare",
            HlAttr {
                fg: rgb(160, 160, 255),
                underline: true,
                ..HlAttr::default()
            },
        );
        table.set(
            "SpellLocal",
            HlAttr {
                fg: rgb(120, 220, 120),
                underline: true,
                ..HlAttr::default()
            },
        );
        // vim's default links (highlight.c hl_init).
        table.link("String", "Constant");
        table.link("Character", "Constant");
        table.link("Number", "Constant");
        table.link("Boolean", "Constant");
        table.link("Float", "Number");
        table.link("Identifier", "Normal");
        table.link("Function", "Identifier");
        table.link("Statement", "Keyword");
        table.link("Conditional", "Statement");
        table.link("Repeat", "Statement");
        table.link("Label", "Statement");
        table.link("Operator", "Statement");
        table.link("Exception", "Statement");
        table.link("Keyword", "Statement");
        table.link("Type", "Normal");
        table.link("StorageClass", "Type");
        table.link("Structure", "Type");
        table.link("Typedef", "Type");
        table.link("Special", "Normal");
        table.link("SpecialChar", "Special");
        table.link("PreProc", "Special");
        table.link("Include", "PreProc");
        table.link("Define", "PreProc");
        table.link("Macro", "PreProc");
        table.link("Comment", "Normal");
        table.link("Title", "Normal");
        table.link("Error", "Normal");
        table
    }

    pub fn set(&mut self, group: &str, attr: HlAttr) {
        self.attrs.insert(group.to_string(), attr);
    }

    pub fn set_cterm(&mut self, group: &str, attr: HlAttr) {
        self.cterm_attrs.insert(group.to_string(), attr);
    }

    /// Direct attributes, without inheriting a link or the other color mode.
    pub fn attributes(&self, group: &str, gui: bool) -> Option<HlAttr> {
        if gui { &self.attrs } else { &self.cterm_attrs }
            .get(group)
            .copied()
    }

    pub fn force_link(&mut self, from: &str, to: &str) {
        self.clear(Some(from));
        self.link(from, to);
    }

    pub fn link(&mut self, from: &str, to: &str) {
        self.links.insert(from.to_string(), to.to_string());
    }

    /// Drop one group (or everything with `None`).
    pub fn clear(&mut self, group: Option<&str>) {
        match group {
            Some(g) => {
                self.attrs.remove(g);
                self.cterm_attrs.remove(g);
                self.links.remove(g);
            }
            None => {
                self.attrs.clear();
                self.cterm_attrs.clear();
                self.links.clear();
            }
        }
    }

    /// True when the group exists (defined or linked).
    pub fn exists(&self, group: &str) -> bool {
        self.attrs.contains_key(group)
            || self.cterm_attrs.contains_key(group)
            || self.links.contains_key(group)
    }

    /// Resolve a group through up to 10 `:hi link` hops.
    pub fn resolve(&self, group: &str) -> Option<HlAttr> {
        self.resolve_for(group, true)
    }

    pub fn resolve_for(&self, group: &str, gui: bool) -> Option<HlAttr> {
        let mut name = group;
        for _ in 0..10 {
            if let Some(attr) = self
                .attributes(name, gui)
                .or_else(|| self.attributes(name, !gui))
            {
                return Some(attr);
            }
            name = self.links.get(name)?;
        }
        None
    }

    /// The group a name links to, if any (`:hi link` target).
    pub fn link_target(&self, group: &str) -> Option<&str> {
        self.links.get(group).map(String::as_str)
    }

    /// All defined group names, sorted (tests and `:hi` listing).
    pub fn groups(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.attrs.keys().map(String::as_str).collect();
        out.extend(self.cterm_attrs.keys().map(String::as_str));
        out.extend(self.links.keys().map(String::as_str));
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Parse `#RRGGBB` / `#RGB` / a decimal cterm index.
pub fn parse_color(text: &str) -> Option<HlColor> {
    let text = text.trim();
    let named = match text.to_ascii_lowercase().as_str() {
        "black" => Some(0),
        "darkred" => Some(1),
        "darkgreen" => Some(2),
        "brown" | "darkyellow" => Some(3),
        "darkblue" => Some(4),
        "darkmagenta" => Some(5),
        "darkcyan" => Some(6),
        "gray" | "grey" | "lightgray" | "lightgrey" => Some(7),
        "darkgray" | "darkgrey" => Some(8),
        "red" | "lightred" => Some(9),
        "green" | "lightgreen" => Some(10),
        "yellow" | "lightyellow" => Some(11),
        "blue" | "lightblue" => Some(12),
        "magenta" | "lightmagenta" => Some(13),
        "cyan" | "lightcyan" => Some(14),
        "white" => Some(15),
        _ => None,
    };
    if let Some(index) = named {
        return Some(HlColor::Indexed(index));
    }
    if let Some(hex) = text.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<Vec<u8>>>()?;
        return match digits.len() {
            3 => Some(HlColor::Rgb(digits[0] * 17, digits[1] * 17, digits[2] * 17)),
            6 => Some(HlColor::Rgb(
                digits[0] * 16 + digits[1],
                digits[2] * 16 + digits[3],
                digits[4] * 16 + digits[5],
            )),
            _ => None,
        };
    }
    text.parse::<u8>().ok().map(HlColor::Indexed)
}

/// Parse the `gui=`/`cterm=` attribute list.
pub fn parse_attrs(text: &str) -> HlAttr {
    let mut attr = HlAttr::default();
    for part in text.split(',') {
        match part.trim() {
            "bold" => attr.bold = true,
            "italic" => attr.italic = true,
            "underline" => attr.underline = true,
            "reverse" | "inverse" => attr.reverse = true,
            _ => {}
        }
    }
    attr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_link_through_to_a_colour() {
        let table = HighlightTable::with_defaults();
        let attr = table.resolve("Statement").expect("Statement resolves");
        assert_eq!(attr.fg, Some(HlColor::Rgb(197, 134, 192)));
        assert!(table.exists("Conditional"));
        assert_eq!(
            table.resolve("Conditional").map(|a| a.fg),
            table.resolve("Statement").map(|a| a.fg)
        );
    }

    #[test]
    fn explicit_attributes_win_over_links() {
        let mut table = HighlightTable::with_defaults();
        table.set(
            "Statement",
            HlAttr {
                fg: Some(HlColor::Rgb(1, 2, 3)),
                ..HlAttr::default()
            },
        );
        assert_eq!(
            table.resolve("Conditional").and_then(|a| a.fg),
            Some(HlColor::Rgb(1, 2, 3))
        );
    }

    #[test]
    fn colours_and_attributes_parse() {
        assert_eq!(parse_color("#ff8000"), Some(HlColor::Rgb(255, 128, 0)));
        assert_eq!(parse_color("#f80"), Some(HlColor::Rgb(255, 136, 0)));
        assert_eq!(parse_color("42"), Some(HlColor::Indexed(42)));
        assert_eq!(parse_color("bogus"), None);
        let attr = parse_attrs("bold,italic,underline");
        assert!(attr.bold && attr.italic && attr.underline);
    }

    #[test]
    fn links_stop_at_cycles() {
        let mut table = HighlightTable::default();
        table.link("A", "B");
        table.link("B", "A");
        assert_eq!(table.resolve("A"), None);
    }
}
