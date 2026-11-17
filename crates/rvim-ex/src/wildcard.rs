//! Self-contained wildcard (glob) matching for filenames and `:runtime`.
//!
//! Supports Vim's filename wildcards: `*` (any run), `?` (one character),
//! `[abc]` / `[a-z]` / `[^abc]` character classes and `\` escaping. `**` is
//! handled at the path level by the caller (it means "recurse"), not here.
//!
//! Reference: filepath.c `vim_pat_matches()`.

/// Does `text` match the single-component glob `pattern`?
pub fn matches(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    glob(&pat, &txt)
}

fn glob(pat: &[char], txt: &[char]) -> bool {
    // Iterative matcher with backtracking on '*'.
    let mut p = 0usize;
    let mut t = 0usize;
    let mut star: Option<(usize, usize)> = None;
    while t < txt.len() {
        if p < pat.len() {
            match pat[p] {
                '*' => {
                    star = Some((p, t));
                    p += 1;
                    continue;
                }
                '?' => {
                    p += 1;
                    t += 1;
                    continue;
                }
                '[' => match class_len(&pat[p..]) {
                    Some(len) => {
                        if class_matches(&pat[p..p + len], txt[t]) {
                            p += len;
                            t += 1;
                            continue;
                        }
                    }
                    None => {
                        // Unterminated '[' is a literal character.
                        if txt[t] == '[' {
                            p += 1;
                            t += 1;
                            continue;
                        }
                    }
                },
                '\\' => {
                    if p + 1 < pat.len() && txt[t] == pat[p + 1] {
                        p += 2;
                        t += 1;
                        continue;
                    }
                    if p + 1 >= pat.len() && txt[t] == '\\' {
                        p += 1;
                        t += 1;
                        continue;
                    }
                }
                c => {
                    if txt[t] == c {
                        p += 1;
                        t += 1;
                        continue;
                    }
                }
            }
        }
        // Mismatch: backtrack to the last '*' if there is one.
        match star {
            Some((sp, st)) => {
                p = sp + 1;
                t = st + 1;
                star = Some((sp, st + 1));
            }
            None => return false,
        }
    }
    while p < pat.len() && pat[p] == '*' {
        p += 1;
    }
    p == pat.len()
}

/// Length in chars of a `[...]` class starting at `pat[0] == '['`.
fn class_len(pat: &[char]) -> Option<usize> {
    let mut i = 1;
    if i < pat.len() && (pat[i] == '^' || pat[i] == '!') {
        i = 1 + 1;
        if i >= pat.len() {
            return None;
        }
        // A ']' right after the negation marker is a literal member.
        if pat[i] == ']' {
            i += 1;
        }
    } else if i < pat.len() && pat[i] == ']' {
        i += 1;
    }
    while i < pat.len() {
        if pat[i] == ']' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn class_matches(class: &[char], c: char) -> bool {
    let mut i = 1;
    let negated = matches!(class.get(1), Some('^') | Some('!'));
    if negated {
        i = 2;
    }
    // A leading ']' is a literal member.
    if class.get(i) == Some(&']') {
        if c == ']' {
            return !negated;
        }
        i += 1;
    }
    let mut hit = false;
    while i < class.len() && class[i] != ']' {
        if i + 2 < class.len() && class[i + 1] == '-' && class[i + 2] != ']' {
            if class[i] <= c && c <= class[i + 2] {
                hit = true;
            }
            i += 3;
        } else {
            if class[i] == c {
                hit = true;
            }
            i += 1;
        }
    }
    hit != negated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_and_questions() {
        assert!(matches("*.txt", "notes.txt"));
        assert!(matches("*", "anything"));
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "ac"));
        assert!(matches("a*b*c", "aXXbYYc"));
        assert!(!matches("a*b*c", "aXXbYY"));
        assert!(matches("", ""));
        assert!(!matches("", "x"));
    }

    #[test]
    fn character_classes() {
        assert!(matches("[abc]", "b"));
        assert!(!matches("[abc]", "d"));
        assert!(matches("[a-z]", "m"));
        assert!(!matches("[a-z]", "M"));
        assert!(matches("[^abc]", "d"));
        assert!(!matches("[^abc]", "a"));
        assert!(matches("file[0-9].txt", "file7.txt"));
    }

    #[test]
    fn backslash_escapes_meta_characters() {
        assert!(matches(r"a\*b", "a*b"));
        assert!(!matches(r"a\*b", "axb"));
    }

    #[test]
    fn multiple_stars_backtrack_correctly() {
        assert!(matches("*a*b*", "xxaxxbyy"));
        assert!(!matches("*a*b*c", "xxaxxbyy"));
    }
}
