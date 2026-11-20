//! Unit tests for the MVP regex engine derived from <https://vimregex.com/>.
//!
//! The tutorial's sections are the specification: \4.1 anchors and word
//! boundaries, \4.2 the metacharacter/class table, \4.3 quantifiers (greedy and
//! non-greedy), \4.4 character ranges, \4.5 grouping and backreferences, \4.6
//! alternation and \4.7 precedence. Each test quotes the tutorial example it
//! checks, so the suite doubles as a compatibility record.
//!
//! The engine implements vim's "magic" dialect (what VimL patterns and `:s`
//! use): `.` `*` `[` are special, `+` `?` `|` `(` need a backslash, and the
//! extra classes and `\{-}` are the vim forms.

use rvim_viml::regex_mvp::find;

/// Byte range of the first match.
fn m(pattern: &str, text: &str) -> Option<(usize, usize)> {
    find(pattern, text, false)
}

/// The text of the first match.
fn matched<'a>(pattern: &str, text: &'a str) -> Option<&'a str> {
    m(pattern, text).map(|(start, end)| &text[start..end])
}

fn is_match(pattern: &str, text: &str) -> bool {
    m(pattern, text).is_some()
}

// ------------------------------------------------ \4.1 anchors and boundaries

#[test]
fn word_boundaries_match_only_whole_words() {
    // "The right way is to put special word boundary symbols \< and \> around vi."
    assert_eq!(matched(r"\<vi\>", "a vi b"), Some("vi"));
    assert_eq!(matched(r"\<vi\>", "navigator"), None);
    assert_eq!(matched(r"\<vi\>", "evil"), None);
    // Punctuation is a boundary too, unlike wrapping the pattern in spaces.
    assert_eq!(matched(r"\<vi\>", "vi, meet (vi)."), Some("vi"));
    assert_eq!(matched(r"\<vi\>", "vi"), Some("vi"));
    assert_eq!(matched(r"\<vi\>", "avi"), None);
}

#[test]
fn line_anchors_bind_the_start_and_the_end() {
    // "^vi\>" only at the start of the line; "^vi$" only for a lone word.
    assert_eq!(matched(r"^vi\>", "vi x"), Some("vi"));
    assert_eq!(matched(r"^vi\>", "x vi"), None);
    assert_eq!(matched(r"^vi$", "vi"), Some("vi"));
    assert_eq!(matched(r"^vi$", "vi "), None);
    assert_eq!(matched(r"^vi$", "xvi"), None);
}

#[test]
fn classes_stand_in_for_a_case_variant() {
    // ":%s:[Vv]i:VIM: will match vi and Vi."
    assert_eq!(matched(r"[Vv]i", "Vi and vi"), Some("Vi"));
    assert_eq!(matched(r"[Vv]i", "vi and Vi"), Some("vi"));
    // The \c flag does the same for the whole pattern.
    assert!(find(r"\cvi", "VI", false).is_some());
    assert!(find(r"\cVI", "vi", false).is_some());
    assert!(find(r"\Cvi", "VI", false).is_none());
}

// ------------------------------------------------- \4.2 metacharacter classes

#[test]
fn the_metacharacter_table_matches_its_examples() {
    // "\d\d/\d\d/\d\d\d\d" - a date like 09/01/2000.
    assert_eq!(
        matched(r"\d\d/\d\d/\d\d\d\d", "on 09/01/2000 ok"),
        Some("09/01/2000")
    );
    assert!(!is_match(r"\d\d/\d\d/\d\d\d\d", "on 9/1/2000 ok"));
    // "\u\w\w\w\w\w" - a six letter word starting with a capital.
    assert_eq!(matched(r"\u\w\w\w\w\w", "Hacker there"), Some("Hacker"));
    assert_eq!(matched(r"\u\w\w\w\w\w", "hacker there"), None);
}

#[test]
fn every_predefined_class_positive_and_negative() {
    // The tutorial's table: each class and its negation.
    let cases: &[(&str, &str, &str)] = &[
        (r"\d", "7", "a"),
        (r"\D", "a", "7"),
        (r"\x", "f", "g"),
        (r"\X", "g", "f"),
        (r"\o", "7", "8"),
        (r"\O", "8", "7"),
        (r"\h", "_", "-"),
        (r"\H", "-", "_"),
        (r"\w", "_", "-"),
        (r"\W", "-", "_"),
        (r"\a", "z", "1"),
        (r"\A", "1", "z"),
        (r"\l", "a", "A"),
        (r"\L", "A", "a"),
        (r"\u", "A", "a"),
        (r"\U", "a", "A"),
        (r"\s", " ", "x"),
        (r"\S", "x", " "),
        (r"\p", "~", "\x01"),
        (r"\f", "a", " "),
    ];
    for (pattern, yes, no) in cases {
        assert!(is_match(pattern, yes), "{pattern} should match {yes:?}");
        assert!(!is_match(pattern, no), "{pattern} should not match {no:?}");
    }
}

#[test]
fn dot_matches_anything_but_a_newline() {
    assert!(is_match("a.c", "abc"));
    assert!(is_match("a.c", "a c"));
    assert!(!is_match("a.c", "a\nc"));
}

#[test]
fn p_matches_printable_but_p_excludes_digits() {
    // vim's \p is printable, \P printable except digits.
    assert!(is_match(r"\p", "7"));
    assert!(!is_match(r"\P", "7"));
    assert!(is_match(r"\P", "a"));
    assert!(!is_match(r"\p", "\x01"));
}

// ------------------------------------------------------- \4.3 quantifiers

#[test]
fn greedy_quantifiers_take_as_much_as_possible() {
    assert_eq!(matched("ab*c", "ac"), Some("ac"));
    assert_eq!(matched("ab*c", "abbbc"), Some("abbbc"));
    assert_eq!(matched(r"ab\+c", "ac"), None);
    assert_eq!(matched(r"ab\+c", "abbc"), Some("abbc"));
    assert_eq!(matched(r"ab\=c", "ac"), Some("ac"));
    assert_eq!(matched(r"ab\?c", "abc"), Some("abc"));
    assert_eq!(matched(r"a\{2,3\}", "aaaa"), Some("aaa"));
    assert_eq!(matched(r"a\{2\}", "aaa"), Some("aa"));
    assert_eq!(matched(r"a\{,2\}", "aaa"), Some("aa"));
    assert_eq!(matched(r"a\{2,\}", "aaaa"), Some("aaaa"));
    assert_eq!(matched(r"a\{2,3\}", "a"), None);
}

#[test]
fn greedy_dot_star_spans_to_the_last_quote() {
    // "this file is normally \"$VIM/.gvimrc\". You can check this with \":version\"."
    let text = r#"this file is normally "$VIM/.gvimrc". You can check this with ":version"."#;
    assert_eq!(
        matched(r#"".*""#, text),
        Some(r#""$VIM/.gvimrc". You can check this with ":version""#)
    );
}

#[test]
fn non_greedy_quantifier_stops_at_the_first_quote() {
    let text = r#"this file is normally "$VIM/.gvimrc". You can check this with ":version"."#;
    assert_eq!(matched(r#"".\{-\}""#, text), Some(r#""$VIM/.gvimrc""#));
    // Non-greedy repetition is still a real repetition: at least one char.
    assert_eq!(matched(r#"".\{-1,\}""#, text), Some(r#""$VIM/.gvimrc""#));
    // The tutorial's warning: .\{-} is happy with matching nothing.
    assert_eq!(m(r".\{-\}", "ab"), Some((0, 0)));
}

#[test]
fn non_greedy_counts_take_the_smallest_number() {
    assert_eq!(matched(r"a\{-1,3\}", "aaaa"), Some("a"));
    assert_eq!(matched(r"a\{-2,3\}", "aaaa"), Some("aa"));
    assert_eq!(matched(r"a\{-2,3\}b", "aaab"), Some("aaab"));
    // A non-greedy repeat still has to satisfy the rest of the pattern.
    assert_eq!(matched(r"<.\{-1,\}>", "<a><b>"), Some("<a>"));
}

// -------------------------------------------------- \4.4 character ranges

#[test]
fn character_ranges_are_sets_of_single_characters() {
    assert_eq!(matched(r"[012345]", "5!"), Some("5"));
    assert_eq!(matched(r"[0-5]", "7"), None);
    assert_eq!(matched(r"[a-z]", "Q"), None);
    assert_eq!(matched(r"[a-zA-Z]", "Q"), Some("Q"));
    assert_eq!(matched(r"[0-9a-zA-Z]", "-"), None);
    // The tutorial's \"[65]\" example: each character is replaced alone,
    // while the literal pattern "65" replaces both.
    assert_eq!(m(r"[65]", "High 65 to 70"), Some((5, 6)));
    assert_eq!(m(r"65", "High 65 to 70"), Some((5, 7)));
}

#[test]
fn negated_ranges_and_literal_dash() {
    assert_eq!(matched(r"[^A-Z]", "aB"), Some("a"));
    assert_eq!(matched(r"[^A-Z]", "AB"), None);
    // "-" first in the class is a literal.
    assert_eq!(matched(r"[-0-9]", "x-1"), Some("-"));
    assert_eq!(matched(r#"[^"]"#, r#""a""#), Some("a"));
    // "\.\s\+[a-z]" from the grammar-check example.
    assert_eq!(matched(r"\.\s\+[a-z]", "done.  next"), Some(".  n"));
    assert_eq!(matched(r"\.\s\+[a-z]", "done. Next"), None);
}

#[test]
fn quoted_text_without_the_greedy_trap() {
    // "/\"[^\"]\+\"/" is the tutorial's fix for the .* trap.
    let text = r#"a "one" b "two" c"#;
    assert_eq!(matched(r#""[^"]\+""#, text), Some(r#""one""#));
}

// --------------------------------------- \4.5 grouping, \4.6 alternation

#[test]
fn groups_scope_repetition_and_alternation() {
    assert_eq!(matched(r"\(ab\)\+", "ababab"), Some("ababab"));
    assert_eq!(matched(r"\(foo\|bar\)baz", "xxbarbaz"), Some("barbaz"));
    assert_eq!(matched(r"\(foo\|bar\)baz", "xxbaz"), None);
    // vimregex \4.6: \%(...\) is the non-capturing form.
    assert_eq!(matched(r"\%(a\|b\)c", "bc"), Some("bc"));
    assert_eq!(matched(r"\%(ab\)\{2\}", "abab"), Some("abab"));
}

#[test]
fn alternation_is_the_lowest_precedence_operator() {
    // "red\|blue" is (red)|(blue); concatenation binds tighter.
    assert_eq!(matched(r"red\|blue", "a blue b"), Some("blue"));
    assert_eq!(matched(r"red\|blue", "a red b"), Some("red"));
    assert_eq!(matched(r"ab\|cd", "abcd"), Some("ab"));
    // Precedence: grouping > concatenation > alternation.
    assert_eq!(matched(r"a\(b\|c\)d", "acd"), Some("acd"));
    assert_eq!(matched(r"a\(b\|c\)d", "abd"), Some("abd"));
    assert_eq!(matched(r"a\(b\|c\)d", "ad"), None);
    // The first alternative to match wins, left to right.
    assert_eq!(matched(r"b\|ab", "ab"), Some("ab"));
}

#[test]
fn swapping_two_words_matches_its_three_groups() {
    // s:\(\w\+\)\(\s\+\)\(\w\+\):\3\2\1:
    let text = "first   second rest";
    assert_eq!(
        matched(r"\(\w\+\)\(\s\+\)\(\w\+\)", text),
        Some("first   second")
    );
}

#[test]
fn backreferences_are_not_supported_yet() {
    // vimregex \4.5 relies on \1..\9, but the engine has no capture tracking
    // yet (docs/compat_deviations.md \7.5: the NFA engine will add it).
    assert!(!is_match(r"\(a\)\1", "aa"));
}

// --------------------------------------------- vim-specific escapes and UTF-8

#[test]
fn percent_escapes_are_single_characters() {
    assert_eq!(matched(r"\%d65", "A"), Some("A"));
    assert_eq!(matched(r"\%x41", "A"), Some("A"));
    assert_eq!(matched(r"\%o101", "A"), Some("A"));
    assert_eq!(matched(r"\%u00e9", "caf\u{e9}"), Some("\u{e9}"));
}

#[test]
fn finds_report_byte_offsets_in_utf8_text() {
    assert_eq!(m("b", "abc"), Some((1, 2)));
    assert_eq!(m("\u{e9}", "a\u{e9}b"), Some((1, 3)));
    assert_eq!(m("[^x]", "\u{e9}"), Some((0, 2)));
}

#[test]
fn case_folding_applies_to_classes_and_literals() {
    assert!(
        find(r"\u", "a", true).is_some(),
        "case-insensitive uppercase class matches a lowercase letter"
    );
    assert!(find(r"[A-Z]\+", "abc", true).is_some());
    assert!(find(r"\c[A-Z]", "q", false).is_some());
}

#[test]
fn empty_pattern_and_anchors_are_zero_width() {
    assert_eq!(m("", "abc"), Some((0, 0)));
    assert_eq!(m("^", "abc"), Some((0, 0)));
    assert_eq!(m("$", "abc"), Some((3, 3)));
    assert_eq!(m("^$", ""), Some((0, 0)));
}
