//! Key decoding: raw terminal bytes -> `map` keys (CSI/SS3/modifyOtherKeys).
//!
//! Vim's equivalent is `key_names` + the terminal code in `term.c`/`os_win32.c`.
//! The resulting [`Key`] type is the same one mappings use
//! (`rvim-editor::maps::Key`) so typed keys can be fed straight into the
//! expansion engine.

use rvim_editor::maps::Key;

/// Result of decoding one chunk of input.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub keys: Vec<Key>,
    /// Bytes consumed; the caller keeps the remainder for the next read when a
    /// sequence was split across reads.
    pub consumed: usize,
}

/// Decode as many keys as possible from `input`.
///
/// An incomplete escape sequence at the end is *not* consumed: it is reported
/// via `consumed` so the caller can read more bytes.
pub fn decode_keys(input: &[u8]) -> Decoded {
    let mut out = Decoded::default();
    let mut i = 0;
    while i < input.len() {
        match decode_one_key(&input[i..]) {
            Some((key, used)) => {
                out.keys.push(key);
                i += used;
            }
            None => break,
        }
    }
    out.consumed = i;
    out
}

/// Decode the first key at the start of `input`, returning it with the number of
/// bytes it occupied. `None` means the sequence is incomplete.
pub fn decode_one_key(input: &[u8]) -> Option<(Key, usize)> {
    decode_one(input)
}

fn decode_one(input: &[u8]) -> Option<(Key, usize)> {
    let first = *input.first()?;
    match first {
        0x1b => decode_escape(input),
        b'\r' => Some((Key::Special("CR".into()), 1)),
        b'\n' => Some((Key::Special("CR".into()), 1)),
        b'\t' => Some((Key::Special("TAB".into()), 1)),
        0x7f => Some((Key::Special("BS".into()), 1)),
        0x08 => Some((Key::Special("BS".into()), 1)),
        0x00 => Some((Key::Special("C-@".into()), 1)),
        1..=0x1a => {
            let letter = (b'a' + first - 1) as char;
            Some((
                Key::Special(format!("C-{}", letter.to_ascii_uppercase())),
                1,
            ))
        }
        0x1c..=0x1f => Some((
            Key::Special(format!("C-{}", (b'\\' + first - 0x1c) as char)),
            1,
        )),
        b if b < 0x80 => Some((Key::Char(b as char), 1)),
        _ => decode_utf8(input),
    }
}

fn decode_utf8(input: &[u8]) -> Option<(Key, usize)> {
    let len = match input[0] {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return Some((Key::Char('\u{fffd}'), 1)),
    };
    if input.len() < len {
        return None;
    }
    let text = std::str::from_utf8(&input[..len]).ok()?;
    text.chars().next().map(|c| (Key::Char(c), len))
}

fn decode_escape(input: &[u8]) -> Option<(Key, usize)> {
    if input.len() == 1 {
        // A lone ESC: vim waits for 'timeoutlen', we treat it as <Esc>.
        return Some((Key::Special("ESC".into()), 1));
    }
    match input[1] {
        b'[' => decode_csi(input),
        b'O' => decode_ss3(input),
        _ => {
            // ESC followed by a character is Alt-<char>; report the ESC itself
            // and let the next call decode the rest.
            Some((Key::Special("ESC".into()), 1))
        }
    }
}

fn decode_csi(input: &[u8]) -> Option<(Key, usize)> {
    // ESC [ parameters final
    let mut i = 2;
    let mut params = String::new();
    while i < input.len() {
        let b = input[i];
        if b.is_ascii_digit() || b == b';' || b == b'?' {
            params.push(b as char);
            i += 1;
        } else {
            break;
        }
    }
    let final_byte = *input.get(i)?;
    let used = i + 1;
    let name = match final_byte {
        b'A' => "UP",
        b'B' => "DOWN",
        b'C' => "RIGHT",
        b'D' => "LEFT",
        b'H' => "HOME",
        b'F' => "END",
        b'Z' => "S-TAB",
        // An unknown key code (F13+, bracketed paste, xterm extensions) must be
        // consumed: reporting "incomplete" would make the reader wait for bytes
        // that never arrive and swallow every later key.
        b'~' => match tilde_name(&params) {
            Some(name) => name,
            None => return Some((Key::Special(format!("CSI-{params}~")), used)),
        },
        _ => return Some((Key::Special(format!("CSI-{}", final_byte as char)), used)),
    };
    Some((Key::Special(with_modifier(name, &params)), used))
}

fn decode_ss3(input: &[u8]) -> Option<(Key, usize)> {
    let b = *input.get(2)?;
    let name = match b {
        b'A' => "UP",
        b'B' => "DOWN",
        b'C' => "RIGHT",
        b'D' => "LEFT",
        b'H' => "HOME",
        b'F' => "END",
        b'P' => "F1",
        b'Q' => "F2",
        b'R' => "F3",
        b'S' => "F4",
        _ => return Some((Key::Special(format!("SS3-{}", b as char)), 3)),
    };
    Some((Key::Special(name.to_string()), 3))
}

fn tilde_name(params: &str) -> Option<&'static str> {
    let code: u32 = params.split(';').next()?.parse().ok()?;
    Some(match code {
        1 | 7 => "HOME",
        2 => "INSERT",
        3 => "DELETE",
        4 | 8 => "END",
        5 => "PAGEUP",
        6 => "PAGEDOWN",
        11 => "F1",
        12 => "F2",
        13 => "F3",
        14 => "F4",
        15 => "F5",
        17 => "F6",
        18 => "F7",
        19 => "F8",
        20 => "F9",
        21 => "F10",
        23 => "F11",
        24 => "F12",
        _ => return None,
    })
}

/// `CSI 1;5A` is Ctrl-Up: fold the xterm modifier parameter into the key name.
fn with_modifier(name: &str, params: &str) -> String {
    let Some(modifier) = params.split(';').nth(1) else {
        return name.to_string();
    };
    let Ok(code) = modifier.parse::<u8>() else {
        return name.to_string();
    };
    let bits = code.saturating_sub(1);
    let mut prefix = String::new();
    if bits & 1 != 0 {
        prefix.push_str("S-");
    }
    if bits & 2 != 0 {
        prefix.push_str("M-");
    }
    if bits & 4 != 0 {
        prefix.push_str("C-");
    }
    format!("{prefix}{name}")
}

/// Encode keys back to the byte sequence a terminal would send.
///
/// Used by the scripted driver so tests can exercise the decoder end to end.
pub fn encode_keys(keys: &[Key]) -> Vec<u8> {
    let mut out = Vec::new();
    for key in keys {
        match key {
            Key::Char(c) => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            Key::Nop => {}
            Key::Special(name) => out.extend_from_slice(encode_special(name).as_bytes()),
        }
    }
    out
}

fn encode_special(name: &str) -> String {
    // The notation table owns the byte encodings so decode/encode agree.
    match rvim_editor::keynames::encode_special(name) {
        Some(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        None => format!("<{name}>"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn decode(text: &[u8]) -> Vec<Key> {
        decode_keys(text).keys
    }

    #[test]
    fn printable_and_control_characters() {
        assert_eq!(decode(b"a"), vec![Key::Char('a')]);
        assert_eq!(decode(b" "), vec![Key::Char(' ')]);
        assert_eq!(decode(b"\r"), vec![Key::Special("CR".into())]);
        assert_eq!(decode(b"\t"), vec![Key::Special("TAB".into())]);
        assert_eq!(decode(&[0x7f]), vec![Key::Special("BS".into())]);
        assert_eq!(decode(&[0x01]), vec![Key::Special("C-A".into())]);
        assert_eq!(decode(&[0x17]), vec![Key::Special("C-W".into())]);
    }

    #[test]
    fn arrows_and_function_keys() {
        assert_eq!(decode(b"\x1b[A"), vec![Key::Special("UP".into())]);
        assert_eq!(decode(b"\x1b[B"), vec![Key::Special("DOWN".into())]);
        assert_eq!(decode(b"\x1b[3~"), vec![Key::Special("DELETE".into())]);
        assert_eq!(decode(b"\x1b[5~"), vec![Key::Special("PAGEUP".into())]);
        assert_eq!(decode(b"\x1b[6~"), vec![Key::Special("PAGEDOWN".into())]);
        assert_eq!(decode(b"\x1bOP"), vec![Key::Special("F1".into())]);
        assert_eq!(decode(b"\x1b[24~"), vec![Key::Special("F12".into())]);
        assert_eq!(decode(b"\x1b[H"), vec![Key::Special("HOME".into())]);
    }

    #[test]
    fn modified_arrows_and_backtab() {
        assert_eq!(decode(b"\x1b[1;5A"), vec![Key::Special("C-UP".into())]);
        assert_eq!(decode(b"\x1b[1;6B"), vec![Key::Special("S-C-DOWN".into())]);
        assert_eq!(decode(b"\x1b[Z"), vec![Key::Special("S-TAB".into())]);
    }

    #[test]
    fn unknown_key_codes_are_consumed() {
        // F13 (CSI 25~ on xterm) and bracketed-paste markers are not in the
        // table; they must be consumed instead of stalling the reader forever.
        let decoded = decode_keys(b"\x1b[25~a");
        assert_eq!(decoded.consumed, 6, "sequence and following key consumed");
        assert_eq!(decoded.keys[0], Key::Special("CSI-25~".into()));
        assert_eq!(
            decoded.keys[1],
            Key::Char('a'),
            "following keys still decode"
        );

        let decoded = decode_keys(b"\x1b[200~x\x1b[201~");
        assert_eq!(decoded.keys.last(), Some(&Key::Special("CSI-201~".into())));
        assert!(decoded.consumed > 0);
        assert_eq!(
            decode_keys(b"\x1b[~").consumed,
            3,
            "a parameter-less code is consumed too"
        );
    }

    #[test]
    fn incomplete_sequence_is_not_consumed() {
        let decoded = decode_keys(b"\x1b[");
        assert_eq!(decoded.keys, vec![], "wait for the final byte");
        assert_eq!(decoded.consumed, 0);

        let decoded = decode_keys(b"\x1b[1;");
        assert_eq!(decoded.consumed, 0);

        // The same bytes split across two chunks decode once complete.
        let mut all = b"\x1b[1;".to_vec();
        all.extend_from_slice(b"5A");
        assert_eq!(decode(&all), vec![Key::Special("C-UP".into())]);
    }

    #[test]
    fn utf8_characters_round_trip() {
        let keys = vec![Key::Char('é'), Key::Char('漢')];
        let bytes = encode_keys(&keys);
        assert_eq!(decode(&bytes), keys);
    }

    #[test]
    fn encode_special_matches_terminal_sequences() {
        assert_eq!(encode_keys(&[Key::Special("UP".into())]), b"\x1b[A");
        assert_eq!(encode_keys(&[Key::Special("C-W".into())]), vec![0x17]);
        assert_eq!(encode_keys(&[Key::Special("CR".into())]), b"\r");
    }
}
