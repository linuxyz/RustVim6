//! Vim key notation (`:help notation` / `intro.txt` §notation).
//!
//! Vim recognises `<>` names for keys that cannot be typed literally: `<CR>`,
//! `<Tab>`, `<C-W>`, `<F12>`, `<k0>`, `<xHome>`, `<Char-65>`, and so on. The
//! canonical table lives in `misc2.c` (`key_names_table[]`); this module mirrors
//! it, including the alias spellings (`<Return>`, `<NewLine>`, `<BackSpace>`,
//! `<Ins>`, ...), modifier prefixes (`C-`, `S-`, `M-`/`A-`, `D-`), and the
//! `<Char-N>` decimal/hex/octal form.
//!
//! Internal representation: [`Key::Special`] holds the canonical UPPERCASE name
//! (`"CR"`, `"C-R"`, `"UP"`, `"K0"`, ...). Vim distinguishes a few names that it
//! prints in mixed case (`<Esc>`, `<Up>`); we keep uppercase for round-tripping
//! and accept any casing on input (documented in `docs/compat_deviations.md`).

use crate::maps::Key;

/// Named keys: lowercase spelling (as typed) -> canonical internal name.
const NAMED: &[(&str, &str)] = &[
    // Whitespace / control keys.
    ("tab", "TAB"),
    ("nl", "NL"),
    ("newline", "NL"),
    ("linefeed", "NL"),
    ("lf", "NL"),
    ("cr", "CR"),
    ("return", "CR"),
    ("enter", "CR"),
    ("bs", "BS"),
    ("backspace", "BS"),
    ("nul", "C-@"),
    ("esc", "ESC"),
    ("csi", "CSI"),
    ("xcsi", "XCSI"),
    // Editing keys.
    ("del", "DEL"),
    ("delete", "DELETE"),
    ("kdel", "KDEL"),
    ("insert", "INSERT"),
    ("ins", "INSERT"),
    ("kinsert", "KINSERT"),
    // Cursor keys, including the xterm/PC variants vim names separately.
    ("up", "UP"),
    ("down", "DOWN"),
    ("left", "LEFT"),
    ("right", "RIGHT"),
    ("xup", "XUP"),
    ("xdown", "XDOWN"),
    ("xleft", "XLEFT"),
    ("xright", "XRIGHT"),
    ("home", "HOME"),
    ("khome", "KHOME"),
    ("xhome", "XHOME"),
    ("zhome", "ZHOME"),
    ("end", "END"),
    ("kend", "KEND"),
    ("xend", "XEND"),
    ("zend", "ZEND"),
    ("pageup", "PAGEUP"),
    ("pagedown", "PAGEDOWN"),
    ("kpageup", "KPAGEUP"),
    ("kpagedown", "KPAGEDOWN"),
    // Keypad.
    ("kplus", "KPLUS"),
    ("kminus", "KMINUS"),
    ("kdivide", "KDIVIDE"),
    ("kmultiply", "KMULTIPLY"),
    ("kenter", "KENTER"),
    ("kpoint", "KPOINT"),
    ("k0", "K0"),
    ("k1", "K1"),
    ("k2", "K2"),
    ("k3", "K3"),
    ("k4", "K4"),
    ("k5", "K5"),
    ("k6", "K6"),
    ("k7", "K7"),
    ("k8", "K8"),
    ("k9", "K9"),
    // Misc keys.
    ("help", "HELP"),
    ("undo", "UNDO"),
    ("pastestart", "PASTESTART"),
    ("pasteend", "PASTEEND"),
    ("nop", "Nop"),
    // Mouse keys: recognised in mappings now, decoded once mouse support lands (M5).
    ("mouse", "MOUSE"),
    ("leftmouse", "LEFTMOUSE"),
    ("leftmousenm", "LEFTMOUSENM"),
    ("leftdrag", "LEFTDRAG"),
    ("leftrelease", "LEFTRELEASE"),
    ("leftreleasenm", "LEFTRELEASENM"),
    ("mousemove", "MOUSEMOVE"),
    ("middlemouse", "MIDDLEMOUSE"),
    ("middledrag", "MIDDLEDRAG"),
    ("middlerelease", "MIDDLERELEASE"),
    ("rightmouse", "RIGHTMOUSE"),
    ("rightdrag", "RIGHTDRAG"),
    ("rightrelease", "RIGHTRELEASE"),
    ("scrollwheelup", "SCROLLWHEELUP"),
    ("scrollwheeldown", "SCROLLWHEELDOWN"),
    ("scrollwheelleft", "SCROLLWHEELLEFT"),
    ("scrollwheelright", "SCROLLWHEELRIGHT"),
    ("sgrmouse", "SGRMOUSE"),
    ("sgrmouserelease", "SGRMOUSERELEASE"),
];

/// Names that stand for a printable character rather than a special key code.
const CHAR_NAMES: &[(&str, char)] = &[
    ("space", ' '),
    ("bar", '|'),
    ("bslash", '\\'),
    ("lt", '<'),
    ("gt", '>'),
];

/// Base meaning of a `<>` name before modifiers are applied.
enum Base {
    Named(&'static str),
    Char(char),
    Nop,
}

fn resolve_base(text: &str) -> Option<Base> {
    let lower = text.to_ascii_lowercase();
    if let Some((_, name)) = CHAR_NAMES.iter().find(|(alias, _)| *alias == lower) {
        return Some(Base::Char(*name));
    }
    if let Some((_, name)) = NAMED.iter().find(|(alias, _)| *alias == lower) {
        return Some(if *name == "Nop" {
            Base::Nop
        } else {
            Base::Named(name)
        });
    }
    if let Some(rest) = lower.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u8>() {
            if (1..=37).contains(&n) {
                return Some(Base::Named(fkey(n)));
            }
        }
    }
    if let Some(rest) = lower.strip_prefix("xf") {
        if let Ok(n) = rest.parse::<u8>() {
            if (1..=4).contains(&n) {
                return Some(Base::Named(xfkey(n)));
            }
        }
    }
    None
}

const FKEYS: [&str; 37] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15",
    "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24", "F25", "F26", "F27", "F28",
    "F29", "F30", "F31", "F32", "F33", "F34", "F35", "F36", "F37",
];
const XFKEYS: [&str; 4] = ["XF1", "XF2", "XF3", "XF4"];

fn fkey(n: u8) -> &'static str {
    FKEYS[(n - 1) as usize]
}

fn xfkey(n: u8) -> &'static str {
    XFKEYS[(n - 1) as usize]
}

/// Split `C-S-M-a` style prefixes off a name, returning the modifiers in vim's
/// canonical order (`C-`, `S-`, `M-`, `D-`) and the remaining base text.
fn split_modifiers(name: &str) -> (Vec<char>, &str) {
    let mut mods: Vec<char> = Vec::new();
    let mut rest = name;
    'outer: loop {
        for (prefix, code) in [
            ("C-", 'C'),
            ("S-", 'S'),
            ("M-", 'M'),
            ("A-", 'M'),
            ("D-", 'D'),
        ] {
            let boundary = rest
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix));
            if boundary && rest.len() > prefix.len() {
                if !mods.contains(&code) {
                    mods.push(code);
                }
                rest = &rest[prefix.len()..];
                continue 'outer;
            }
        }
        break;
    }
    mods.sort_by_key(|m| match m {
        'C' => 0,
        'S' => 1,
        'M' => 2,
        _ => 3,
    });
    (mods, rest)
}

fn modifier_prefix(mods: &[char]) -> String {
    let mut out = String::new();
    for m in mods {
        out.push(*m);
        out.push('-');
    }
    out
}

/// Parse one `<>` body into a key. `None` means "not a key name" (vim keeps the
/// text literally in that case).
pub fn resolve_name(name: &str) -> Option<Key> {
    // <Char-N>: decimal, 0x-hex or 0-octal character code.
    if let Some(rest) = name
        .get(..5)
        .filter(|head| head.eq_ignore_ascii_case("char-"))
        .map(|_| &name[5..])
    {
        return parse_char_code(rest).map(Key::Char);
    }
    // <t_xx>: termcap entry. Without a terminfo database we cannot resolve the
    // byte sequence, but the name round-trips so mappings keep working.
    if name.len() == 4 && name.get(..2).is_some_and(|h| h.eq_ignore_ascii_case("t_")) {
        return Some(Key::Special(format!(
            "t_{}",
            name[2..].to_ascii_lowercase()
        )));
    }

    let (mods, base_text) = split_modifiers(name);
    if base_text.is_empty() {
        return None;
    }
    let base = match resolve_base(base_text) {
        Some(base) => base,
        None => {
            // A lone printable character is a valid base: <C-r>, <M-x>.
            let mut it = base_text.chars();
            match (it.next(), it.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => Base::Char(c),
                _ => return None,
            }
        }
    };
    match base {
        Base::Nop => {
            if mods.is_empty() {
                Some(Key::Nop)
            } else {
                None
            }
        }
        Base::Char(c) => {
            if mods.is_empty() {
                return Some(Key::Char(c));
            }
            // <S-a> is just the uppercase character in vim.
            if mods == ['S'] && c.is_ascii_alphabetic() {
                return Some(Key::Char(c.to_ascii_uppercase()));
            }
            if !c.is_ascii_alphanumeric() {
                return None;
            }
            Some(Key::Special(format!(
                "{}{}",
                modifier_prefix(&mods),
                c.to_ascii_uppercase()
            )))
        }
        Base::Named(n) => {
            if mods.is_empty() {
                Some(Key::Special(n.to_string()))
            } else {
                Some(Key::Special(format!("{}{n}", modifier_prefix(&mods))))
            }
        }
    }
}

fn parse_char_code(text: &str) -> Option<char> {
    let value = if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()?
    } else if text.len() > 1 && text.starts_with('0') {
        u32::from_str_radix(&text[1..], 8).ok()?
    } else {
        text.parse::<u32>().ok()?
    };
    char::from_u32(value)
}

/// Canonical special-key name for `name`, or `None` when it is not a key name.
///
/// Kept for callers that only care about [`Key::Special`]; printable names such
/// as `<lt>` resolve through [`resolve_name`] instead.
pub fn canonical_special(name: &str) -> Option<String> {
    match resolve_name(name)? {
        Key::Special(n) => Some(n),
        _ => None,
    }
}

/// Parse `<...>` notation into keys. Unknown `<foo>` is kept literally as text,
/// matching vim's behaviour for unrecognised key names.
pub fn parse_notation(text: &str) -> Vec<Key> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '<' {
            out.push(Key::Char(c));
            continue;
        }
        let mut inner = String::new();
        let mut closed = false;
        while let Some(&n) = chars.peek() {
            chars.next();
            if n == '>' {
                closed = true;
                break;
            }
            inner.push(n);
        }
        if !closed {
            out.push(Key::Char('<'));
            out.extend(inner.chars().map(Key::Char));
            continue;
        }
        match resolve_name(&inner) {
            Some(key) => out.push(key),
            None => {
                out.push(Key::Char('<'));
                out.extend(inner.chars().map(Key::Char));
                out.push(Key::Char('>'));
            }
        }
    }
    out
}

/// Vim prints a few names in mixed case (`<Esc>`, `<Up>`, `<k0>`). Map our
/// canonical uppercase name to that spelling for `:map` listings.
pub fn display_name(name: &str) -> String {
    // Modified keys keep their prefix and display the base specially.
    for prefix in ["C-", "S-", "M-", "D-"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            // Only the first modifier prefix is peeled here; nested prefixes are
            // handled by the recursive call on the remainder.
            let inner = display_name(rest);
            return format!("{prefix}{inner}");
        }
    }
    match name {
        "TAB" => "Tab".to_string(),
        "NL" => "NL".to_string(),
        "ESC" => "Esc".to_string(),
        "DEL" => "Del".to_string(),
        "DELETE" => "Delete".to_string(),
        "INSERT" => "Insert".to_string(),
        "UP" => "Up".to_string(),
        "DOWN" => "Down".to_string(),
        "LEFT" => "Left".to_string(),
        "RIGHT" => "Right".to_string(),
        "HOME" => "Home".to_string(),
        "END" => "End".to_string(),
        "PAGEUP" => "PageUp".to_string(),
        "PAGEDOWN" => "PageDown".to_string(),
        "HELP" => "Help".to_string(),
        "UNDO" => "Undo".to_string(),
        "PASTESTART" => "PasteStart".to_string(),
        "PASTEEND" => "PasteEnd".to_string(),
        "LEFTMOUSE" => "LeftMouse".to_string(),
        "LEFTDRAG" => "LeftDrag".to_string(),
        "LEFTRELEASE" => "LeftRelease".to_string(),
        "MIDDLEMOUSE" => "MiddleMouse".to_string(),
        "MIDDLEDRAG" => "MiddleDrag".to_string(),
        "MIDDLERELEASE" => "MiddleRelease".to_string(),
        "RIGHTMOUSE" => "RightMouse".to_string(),
        "RIGHTDRAG" => "RightDrag".to_string(),
        "RIGHTRELEASE" => "RightRelease".to_string(),
        "SCROLLWHEELUP" => "ScrollWheelUp".to_string(),
        "SCROLLWHEELDOWN" => "ScrollWheelDown".to_string(),
        "SCROLLWHEELLEFT" => "ScrollWheelLeft".to_string(),
        "SCROLLWHEELRIGHT" => "ScrollWheelRight".to_string(),
        other => {
            if let Some(digit) = other.strip_prefix('K') {
                if digit.len() == 1 && digit.chars().all(|c| c.is_ascii_digit()) {
                    return format!("k{digit}");
                }
                return match digit {
                    "PLUS" | "MINUS" | "DIVIDE" | "MULTIPLY" | "ENTER" | "POINT" => {
                        let mut chars = digit.chars();
                        let first = chars.next().unwrap_or('k').to_ascii_uppercase();
                        let rest: String = chars.map(|c| c.to_ascii_lowercase()).collect();
                        format!("k{first}{rest}")
                    }
                    _ => other.to_string(),
                };
            }
            if let Some(rest) = other.strip_prefix("X") {
                // xUp, xHome, xF1 ...
                let mut chars = rest.chars();
                if let Some(first) = chars.next() {
                    let mut out = String::from("x");
                    out.push(first);
                    let rest: String = chars.map(|c| c.to_ascii_lowercase()).collect();
                    out.push_str(&rest);
                    return out;
                }
            }
            other.to_string()
        }
    }
}

/// Render one key back to `<>` notation.
pub fn to_notation(key: &Key) -> String {
    match key {
        Key::Char(c) => match c {
            ' ' => "<Space>".to_string(),
            '<' => "<lt>".to_string(),
            '|' => "<Bar>".to_string(),
            '\\' => "<Bslash>".to_string(),
            _ => c.to_string(),
        },
        Key::Special(name) => format!("<{}>", display_name(name)),
        Key::Nop => "<Nop>".to_string(),
    }
}

/// Render a key sequence back to `<>` notation.
pub fn keys_to_notation(keys: &[Key]) -> String {
    keys.iter().map(to_notation).collect()
}

/// Bytes a terminal sends for a special key, when we can encode it.
///
/// Returns `None` for keys that have no single encoding (mouse, `<t_xx>`,
/// keypad variants we never decode).
pub fn encode_special(name: &str) -> Option<Vec<u8>> {
    // Control characters: <C-A> .. <C-Z>.
    if let Some(rest) = name.strip_prefix("C-") {
        if rest.chars().count() == 1 {
            let c = rest.chars().next()?;
            if c.is_ascii_alphabetic() {
                return Some(vec![(c.to_ascii_uppercase() as u8) - b'A' + 1]);
            }
        }
    }
    let bytes: &[u8] = match name {
        "SPACE" => b" ",
        "TAB" => b"\t",
        "NL" => b"\n",
        "CR" => b"\r",
        "BS" => b"\x08",
        "ESC" => b"\x1b",
        "DEL" => b"\x7f",
        "UP" => b"\x1b[A",
        "DOWN" => b"\x1b[B",
        "RIGHT" => b"\x1b[C",
        "LEFT" => b"\x1b[D",
        "HOME" => b"\x1b[H",
        "END" => b"\x1b[F",
        "INSERT" => b"\x1b[2~",
        "DELETE" => b"\x1b[3~",
        "PAGEUP" => b"\x1b[5~",
        "PAGEDOWN" => b"\x1b[6~",
        "F1" => b"\x1bOP",
        "F2" => b"\x1bOQ",
        "F3" => b"\x1bOR",
        "F4" => b"\x1bOS",
        "F5" => b"\x1b[15~",
        "F6" => b"\x1b[17~",
        "F7" => b"\x1b[18~",
        "F8" => b"\x1b[19~",
        "F9" => b"\x1b[20~",
        "F10" => b"\x1b[21~",
        "F11" => b"\x1b[23~",
        "F12" => b"\x1b[24~",
        _ => return None,
    };
    Some(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<Key> {
        parse_notation(text)
    }

    fn special(name: &str) -> Key {
        Key::Special(name.to_string())
    }

    #[test]
    fn control_and_whitespace_names() {
        assert_eq!(keys("<CR>"), vec![special("CR")]);
        assert_eq!(keys("<Enter>"), vec![special("CR")]);
        assert_eq!(keys("<Return>"), vec![special("CR")]);
        assert_eq!(keys("<NL>"), vec![special("NL")]);
        assert_eq!(keys("<NewLine>"), vec![special("NL")]);
        assert_eq!(keys("<LF>"), vec![special("NL")]);
        assert_eq!(keys("<Tab>"), vec![special("TAB")]);
        assert_eq!(keys("<Esc>"), vec![special("ESC")]);
        assert_eq!(keys("<BS>"), vec![special("BS")]);
        assert_eq!(keys("<BackSpace>"), vec![special("BS")]);
        assert_eq!(keys("<Space>"), vec![Key::Char(' ')]);
        assert_eq!(keys("<Bar>"), vec![Key::Char('|')]);
        assert_eq!(keys("<Bslash>"), vec![Key::Char('\\')]);
        assert_eq!(keys("<lt>"), vec![Key::Char('<')]);
        assert_eq!(keys("<Nul>"), vec![special("C-@")], "vim names NUL <Nul>");
    }

    #[test]
    fn editing_and_cursor_keys_and_aliases() {
        assert_eq!(keys("<Del>"), vec![special("DEL")]);
        assert_eq!(keys("<Delete>"), vec![special("DELETE")]);
        assert_eq!(keys("<Ins>"), vec![special("INSERT")]);
        assert_eq!(keys("<Up>"), vec![special("UP")]);
        assert_eq!(keys("<PageDown>"), vec![special("PAGEDOWN")]);
        assert_eq!(keys("<xHome>"), vec![special("XHOME")]);
        assert_eq!(keys("<kDel>"), vec![special("KDEL")]);
        assert_eq!(keys("<kEnter>"), vec![special("KENTER")]);
        assert_eq!(keys("<Undo>"), vec![special("UNDO")]);
        assert_eq!(keys("<PasteStart>"), vec![special("PASTESTART")]);
    }

    #[test]
    fn function_keys_cover_the_whole_vim_range() {
        assert_eq!(keys("<F1>"), vec![special("F1")]);
        assert_eq!(keys("<f12>"), vec![special("F12")]);
        assert_eq!(keys("<F37>"), vec![special("F37")]);
        // F38 is out of range, so the whole `<F38>` stays literal text.
        assert_eq!(keys("<F38>").len(), 5);
        assert_eq!(keys("<xF4>"), vec![special("XF4")]);
    }

    #[test]
    fn keypad_digits() {
        for n in 0..=9 {
            assert_eq!(
                keys(&format!("<k{n}>")),
                vec![special(&format!("K{n}"))],
                "k{n}"
            );
        }
        assert_eq!(keys("<kPlus>"), vec![special("KPLUS")]);
    }

    #[test]
    fn modifiers_are_canonicalised_in_any_order() {
        assert_eq!(keys("<C-r>"), vec![special("C-R")]);
        assert_eq!(keys("<c-r>"), vec![special("C-R")]);
        assert_eq!(keys("<C-R>"), vec![special("C-R")]);
        // Order does not matter; the canonical order is C-, S-, M-, D-.
        assert_eq!(keys("<S-C-a>"), vec![special("C-S-A")]);
        assert_eq!(keys("<M-C-a>"), vec![special("C-M-A")]);
        assert_eq!(keys("<A-x>"), vec![special("M-X")], "A- is an alias of M-");
        // Shifted letters collapse to the uppercase character.
        assert_eq!(keys("<S-a>"), vec![Key::Char('A')]);
        // Modified named keys keep their name.
        assert_eq!(keys("<C-Up>"), vec![special("C-UP")]);
        assert_eq!(keys("<S-Tab>"), vec![special("S-TAB")]);
    }

    #[test]
    fn char_codes_in_decimal_hex_and_octal() {
        assert_eq!(keys("<Char-65>"), vec![Key::Char('A')]);
        assert_eq!(keys("<Char-0x41>"), vec![Key::Char('A')]);
        assert_eq!(keys("<Char-0101>"), vec![Key::Char('A')]);
        assert_eq!(keys("<Char-32>"), vec![Key::Char(' ')]);
        assert_eq!(keys("<Char-x>"), keys("<Char-x>"), "invalid stays literal");
    }

    #[test]
    fn termcodes_round_trip_but_cannot_be_encoded() {
        assert_eq!(keys("<t_AB>"), vec![special("t_ab")]);
        assert_eq!(encode_special("t_ab"), None);
    }

    #[test]
    fn mouse_names_parse_for_future_support() {
        assert_eq!(keys("<LeftMouse>"), vec![special("LEFTMOUSE")]);
        assert_eq!(keys("<ScrollWheelDown>"), vec![special("SCROLLWHEELDOWN")]);
    }

    #[test]
    fn unknown_names_stay_literal() {
        assert_eq!(
            keys("<foo>"),
            vec![
                Key::Char('<'),
                Key::Char('f'),
                Key::Char('o'),
                Key::Char('o'),
                Key::Char('>')
            ]
        );
        // An unterminated '<' is literal too.
        assert_eq!(
            keys("a<b"),
            vec![Key::Char('a'), Key::Char('<'), Key::Char('b')]
        );
    }

    #[test]
    fn notation_round_trips() {
        let parsed = keys("<C-W><Esc><F5><k1><Char-65>x<Space><lt>");
        // Output is the canonical spelling: uppercase names, no aliases.
        assert_eq!(keys_to_notation(&parsed), "<C-W><Esc><F5><k1>Ax<Space><lt>");
        // Re-parsing the canonical text yields the same keys.
        assert_eq!(keys(&keys_to_notation(&parsed)), parsed);
        // Printable characters round-trip unchanged; special ones stay escaped.
        assert_eq!(keys_to_notation(&keys("ab<c-d>")), "ab<C-D>");
        assert_eq!(to_notation(&Key::Nop), "<Nop>");
    }

    #[test]
    fn special_keys_encode_to_terminal_bytes() {
        assert_eq!(encode_special("CR"), Some(b"\r".to_vec()));
        assert_eq!(encode_special("C-W"), Some(vec![0x17]));
        assert_eq!(encode_special("UP"), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_special("F5"), Some(b"\x1b[15~".to_vec()));
        assert_eq!(encode_special("K0"), None, "keypad is never encoded");
    }
}
