//! Unit tests for the Vim-compatible regex engine.
//!
//! The tutorial at <https://vimregex.com/> is the specification for the basic
//! sections: \4.1 anchors and word boundaries, \4.2 the metacharacter/class
//! table, \4.3 quantifiers (greedy and non-greedy), \4.4 character ranges,
//! \4.5 grouping and backreferences, \4.6 alternation and \4.7 precedence.
//! Later sections cover the extensions: capture offsets, \zs / \ze,
//! back-references, \@= / \@!, multiline atoms and magic levels.  Behaviour
//! matches Vim 9.9 (checked with the vim binary as an oracle).

use rvim_viml::regex_mvp::{Match, find, find_captures, find_from, find_multiline};

/// Byte range of the first match.
fn m(pattern: &str, text: &str) -> Option<(usize, usize)> {
    find(pattern, text, false)
}

/// The text of the first match.
fn matched<'a>(pattern: &str, text: &'a str) -> Option<&'a str> {
    m(pattern, text).map(|(start, end)| &text[start..end])
}

/// The first match with capture groups.
fn caps(pattern: &str, text: &str) -> Option<Match> {
    find_captures(pattern, text, false)
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
    // Vim accepts the closing brace without a backslash too.
    assert_eq!(matched(r"a\{2,3}", "aaaa"), Some("aaa"));
    assert_eq!(matched(r"a\{2,3}", "a"), None);
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
    // The tutorial's "[65]" example: each character is replaced alone, while
    // the literal pattern "65" replaces both.
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
fn backreferences_repeat_the_text_of_a_group() {
    // vimregex \4.5: \1 refers back to the text matched by the first group.
    assert_eq!(matched(r"\(a\)\1", "aa"), Some("aa"));
    assert_eq!(matched(r"\(ab\)\1", "abab"), Some("abab"));
    assert_eq!(m(r"\(a\)\1", "ab"), None);
    assert_eq!(
        matched(r"\(foo\)\(bar\)\2\1", "foobarbarfoo"),
        Some("foobarbarfoo")
    );
    assert_eq!(matched(r"\<\(\w\+\)\s\+\1\>", "the the"), Some("the the"));
    // A backreference to a group that did not participate matches the empty
    // string, exactly like Vim.
    assert_eq!(m(r"\(a\)\?b\1", "b"), Some((0, 1)));
    assert_eq!(matched(r"\(a\)\?b\1", "aba"), Some("aba"));
    assert_eq!(m(r"\(a\)\@!\1", "b"), Some((0, 0)));
    // \0 is not a backreference.
    assert!(is_match(r"\(a\)\0", "a0"));
}

#[test]
fn backreferences_honour_ignorecase() {
    assert_eq!(m(r"\(a\)\1", "aA"), None);
    assert_eq!(m(r"\c\(a\)\1", "aA"), Some((0, 2)));
    assert!(find(r"\(a\)\1", "aA", true).is_some());
    // \C overrides 'ignorecase'.
    assert!(find(r"\C\(a\)\1", "aA", true).is_none());
}

// ------------------------------------------------------ capture group offsets

#[test]
fn capture_groups_are_reported() {
    let hit = caps(r"\(\w\+\)\s\+\(\w\+\)", "  hello   world").unwrap();
    assert_eq!((hit.start, hit.end), (2, 15));
    assert_eq!(hit.groups, vec![Some((2, 7)), Some((10, 15))]);
    // A non-capturing group does not consume a slot.
    let hit = caps(r"\%(a\)\(b\)", "ab").unwrap();
    assert_eq!(hit.groups, vec![Some((1, 2))]);
    // An optional group that did not participate is None.
    let hit = caps(r"\(a\)\=b", "b").unwrap();
    assert_eq!(hit.groups, vec![None]);
    // A group that matched an empty string is Some((pos, pos)).
    let hit = caps(r"\(a*\)b", "b").unwrap();
    assert_eq!(hit.groups, vec![Some((0, 0))]);
    // No groups at all.
    let hit = caps("abc", "xabcx").unwrap();
    assert_eq!((hit.start, hit.end), (1, 4));
    assert!(hit.groups.is_empty());
}

#[test]
fn nested_groups_are_numbered_in_open_order() {
    let hit = caps(r"\(a\(b\)\(c\)\)", "zabc").unwrap();
    assert_eq!(hit.groups, vec![Some((1, 4)), Some((2, 3)), Some((3, 4))]);
}

// -------------------------------------------------------- \zs / \ze markers

#[test]
fn zero_width_markers_adjust_the_match() {
    assert_eq!(matched(r"foo\zs bar", "foo bar"), Some(" bar"));
    assert_eq!(m(r"foo\zs bar", "foo bar"), Some((3, 7)));
    assert_eq!(matched(r"foo\ze bar", "foo bar"), Some("foo"));
    assert_eq!(m(r"foo\ze bar", "foo bar"), Some((0, 3)));
    // \ze cuts the reported match short while matching continues past it.
    assert_eq!(m(r"foo\zebar", "foobar"), Some((0, 3)));
    // The last \zs encountered wins.
    assert_eq!(m(r"foo\zsbar\zs", "foobar"), Some((6, 6)));
    // Markers inside groups move the overall match, not the capture offsets.
    let hit = caps(r"foo\(ba\zsr\)", "foobar").unwrap();
    assert_eq!((hit.start, hit.end), (5, 6));
    assert_eq!(hit.groups, vec![Some((3, 6))]);
    let hit = caps(r"foo\(ba\zer\)", "foobar").unwrap();
    assert_eq!((hit.start, hit.end), (0, 5));
    assert_eq!(hit.groups, vec![Some((3, 6))]);
    // A leading \zs makes the match start where it stands.
    assert_eq!(m(r"\zsfoo", "xxfoo"), Some((2, 5)));
}

#[test]
fn markers_work_inside_lookahead_and_alternation() {
    // A \zs after a successful assertion still moves the reported start.
    assert_eq!(m(r"\(foo\)\@=foo\zsbar", "foobar"), Some((3, 6)));
    assert_eq!(m(r"foo\zs\(bar\)\@=", "foobar"), Some((3, 3)));
    // Vim discards \zs / \ze recorded inside \@= (the assertion is zero
    // width and does not move the outer match boundaries).
    assert_eq!(m(r"\(foo\zs\)\@=foobar", "foobar"), Some((0, 6)));
}

// -------------------------------------------------------- \@= and \@! lookahead

#[test]
fn lookahead_assertions_are_zero_width() {
    assert_eq!(m(r"foo\(bar\)\@=", "foobar"), Some((0, 3)));
    assert_eq!(m(r"foo\(bar\)\@=", "foobaz"), None);
    assert_eq!(m(r"foo\(bar\)\@!", "foobaz"), Some((0, 3)));
    assert_eq!(m(r"foo\(bar\)\@!", "foobar"), None);
    assert_eq!(m(r"\(foo\)\@=foobar", "foobar"), Some((0, 6)));
    // Captures made inside \@= are kept, as in Vim.
    let hit = caps(r"\(foo\)\@=foobar", "foobar").unwrap();
    assert_eq!(hit.groups, vec![Some((0, 3))]);
    // Nested assertions.
    assert_eq!(m(r"\(a\(b\)\@=\)\@=ab", "ab"), Some((0, 2)));
    // The assertion applies to the atom before it, not the whole pattern.
    assert_eq!(m(r"ab\@=", "ab"), Some((0, 1)));
    assert_eq!(m(r"ba\@=", "ab"), None);
}

// --------------------------------------------------------------- multiline

#[test]
fn multiline_matches_cross_line_breaks() {
    let text = "one\ntwo\nthree";
    assert_eq!(matched(r"one\ntwo", text), Some("one\ntwo"));
    assert_eq!(
        find_multiline(r"one\_.two", text, false).map(|m| (m.start, m.end)),
        Some((0, 7))
    );
    // \_. matches any character including a newline.
    assert_eq!(
        find_multiline(r"\_.", "a\nb", false).map(|m| (m.start, m.end)),
        Some((0, 1))
    );
    // \n only matches a newline.
    assert!(find_multiline(r"\n", text, false).is_some());
    assert!(m(r"\n", "abc").is_none());
}

#[test]
fn underscore_classes_include_the_newline() {
    assert!(find_multiline(r"\_s", "a\nb", false).is_some());
    assert!(find_multiline(r"a\_s\+b", "a \n\tb", false).is_some());
    assert!(find_multiline(r"a\_p", "a\n", false).is_some());
    assert!(find_multiline(r"a\_[a-z]", "a\n", false).is_some());
    // Without the underscore the same atoms do not match a newline.
    assert!(m(r"a\p", "a\n").is_none());
    assert!(m(r"a[a-z]", "a\n").is_none());
    assert!(m(r"a\s", "a\n").is_none());
    // A plain negated collection does match an end-of-line (Vim/Vi compatible).
    assert_eq!(m(r"a[^x]", "a\n"), Some((0, 2)));
}

#[test]
fn multiline_anchors_bind_lines() {
    let text = "alpha\nbeta\ngamma";
    assert_eq!(
        find_multiline(r"^beta", text, false).map(|m| (m.start, m.end)),
        Some((6, 10))
    );
    assert_eq!(
        find_multiline(r"alpha$", text, false).map(|m| (m.start, m.end)),
        Some((0, 5))
    );
    assert_eq!(
        find_multiline(r"^gamma$", text, false).map(|m| (m.start, m.end)),
        Some((11, 16))
    );
    // find stays line oriented: ^ and $ only bind the ends of the text.
    assert!(m(r"^beta", text).is_none());
    assert!(m(r"alpha$", text).is_none());
    // \_^ and \_$ are line anchors even in line-oriented mode.
    assert_eq!(m(r"a\_$", "a\nb"), Some((0, 1)));
    assert_eq!(m(r"\_^beta", "alpha\nbeta"), Some((6, 10)));
    // \%^ and \%$ are text anchors.
    assert!(find_multiline(r"\%^alpha", text, false).is_some());
    assert!(find_multiline(r"gamma\%$", text, false).is_some());
}

#[test]
fn multiline_star_spans_the_whole_text() {
    let text = "a\nb\nc";
    let hit = find_multiline(r"\_.*", text, false).unwrap();
    assert_eq!((hit.start, hit.end), (0, text.len()));
    // In line-oriented mode '.' stops at the first newline.
    assert_eq!(m(r"a.*", text), Some((0, 1)));
}

// ------------------------------------------------------------- magic levels

#[test]
fn very_magic_makes_punctuation_special() {
    assert_eq!(matched(r"\v(a|b)+", "abab"), Some("abab"));
    assert_eq!(matched(r"\v\d+\.\d+", "pi 3.14"), Some("3.14"));
    // As in Vim the switch lasts until the end of the pattern.
    assert_eq!(m(r"\v(a|b)c|d", "x|d"), Some((2, 3)));
    // A backslash makes very-magic punctuation literal again.
    assert_eq!(matched(r"\vfoo\.bar", "foo.bar"), Some("foo.bar"));
    assert_eq!(m(r"\vfoo\.bar", "fooXbar"), None);
    // Bare braces, '=' and '+' are quantifiers in very magic.
    assert_eq!(matched(r"\va{2,3}", "aaaa"), Some("aaa"));
    assert_eq!(matched(r"\vab=", "ab"), Some("ab"));
    assert!(is_match(r"\vab=", "a"));
}

#[test]
fn very_nomagic_treats_everything_literally() {
    assert_eq!(matched(r"\Vfoo.bar", "foo.bar and more"), Some("foo.bar"));
    assert!(!is_match(r"\Vfoo.bar", "fooXbar"));
    assert!(!is_match(r"\Vfoo*bar", "foooobar"));
    assert!(is_match(r"\Vfoo*bar", "foo*bar"));
    // Parentheses are literal too (Vim 9.9 behaviour).
    assert_eq!(matched(r"\V(foo)", "x(foo)y"), Some("(foo)"));
    // A backslash re-enables a special meaning: \. is "any character", so it
    // matches the dot as well as any other character.
    assert_eq!(matched(r"\Vfoo\.bar", "fooXbar"), Some("fooXbar"));
    assert_eq!(matched(r"\Vfoo\.bar", "foo.bar"), Some("foo.bar"));
    assert!(!is_match(r"\Vfoo\.bar", "fooXbaz"));
}

#[test]
fn magic_levels_can_be_switched_inside_a_pattern() {
    // \M is nomagic: '.' is literal, '\.' is any character.
    assert_eq!(matched(r"\Mfoo.bar", "foo.bar"), Some("foo.bar"));
    assert!(!is_match(r"\Mfoo.bar", "fooXbar"));
    assert!(is_match(r"\Mfoo\.bar", "fooXbar"));
    // Default magic: '\+' works, '+' is literal.
    assert!(is_match(r"a\+b", "aaab"));
    assert!(is_match(r"a+b", "a+b"));
    // Switching back and forth.
    assert!(is_match(r"\v(a|b)\m\|c", "c"));
    assert!(is_match(r"\v(a|b)\m\|c", "b"));
}

// -------------------------------------------------------------- find_from

#[test]
fn find_from_starts_at_the_requested_offset() {
    let hit = find_from("b", "abab", 1, false).unwrap();
    assert_eq!((hit.start, hit.end), (1, 2));
    let hit = find_from("b", "abab", 2, false).unwrap();
    assert_eq!((hit.start, hit.end), (3, 4));
    assert!(find_from("b", "abab", 4, false).is_none());
    // Capture offsets stay relative to the whole text.
    let hit = find_from(r"\(b\)", "abab", 2, false).unwrap();
    assert_eq!(hit.groups, vec![Some((3, 4))]);
    // Byte offsets do not need to be char boundaries; a position inside a
    // multi-byte character rounds up to the next boundary.
    assert_eq!(
        find_from("\u{e9}", "a\u{e9}", 1, false).map(|m| (m.start, m.end)),
        Some((1, 3))
    );
    assert!(find_from("\u{e9}", "a\u{e9}", 2, false).is_none());
}

// --------------------------------------------- vim-specific escapes and UTF-8

#[test]
fn percent_escapes_are_single_characters() {
    assert_eq!(matched(r"\%d65", "A"), Some("A"));
    assert_eq!(matched(r"\%x41", "A"), Some("A"));
    assert_eq!(matched(r"\%o101", "A"), Some("A"));
    assert_eq!(matched(r"\%u00e9", "caf\u{e9}"), Some("\u{e9}"));
    // A zero code point is how vim spells a newline.
    assert_eq!(matched(r"\%d0", "\n"), Some("\n"));
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

// ------------------------------------------------- collections and classes

#[test]
fn posix_classes_and_escapes_inside_collections() {
    assert_eq!(matched(r"[[:alpha:]]\+", "123abc456"), Some("abc"));
    assert_eq!(matched(r"[[:alnum:]_]\+", "  foo_1  "), Some("foo_1"));
    assert!(is_match(r"[[:digit:]]", "7"));
    assert!(!is_match(r"[[:digit:]]", "x"));
    assert!(is_match(r"[[:space:]]", "\t"));
    assert!(is_match(r"[[:punct:]]", "!"));
    assert!(is_match(r"[\x41-\x5a]\+", "HELLO"));
    assert!(is_match(r"[\n]", "a\nb"));
    // A collection without a closing bracket is a literal '['.
    assert!(is_match(r"[abc", "[abc"));
    assert!(is_match(r"[]]", "]"));
}

// ------------------------------------------------------- pathological input

#[test]
fn zero_width_repeats_terminate() {
    // An empty-able body must not make the matcher loop forever.
    assert_eq!(m(r"\(a*\)*", "aaa"), Some((0, 3)));
    assert_eq!(m(r"\(a*\)*b", "aaab"), Some((0, 4)));
    assert_eq!(m(r"\(\)*", "abc"), Some((0, 0)));
    assert_eq!(m(r"\(a*\)\{2,3\}", "aa"), Some((0, 2)));
    assert_eq!(m(r"\(a*\)\+", "aaa"), Some((0, 3)));
    // The step budget cuts off catastrophic backtracking instead of hanging.
    let text = "a".repeat(60);
    assert!(find(r"\(a*\)*b", &text, false).is_none());
}

// ----------------------------------------- oracle-checked corner cases (Vim 9.9)

#[test]
fn very_magic_percent_and_braces() {
    // Bare %(...) is the non-capturing group in very magic.
    assert_eq!(matched(r"\v%(a|b)+", "ab"), Some("ab"));
    assert!(!is_match(r"\v%(a|b)c", "ad"));
    // \% is a literal percent in very magic.
    assert_eq!(matched(r"\v\%d65", "%d65"), Some("%d65"));
    assert!(!is_match(r"\v\%d65", "A"));
    // Bare braces are quantifiers; the closing brace may be plain or escaped.
    assert_eq!(matched(r"\v(a|b){2}", "abab"), Some("ab"));
    assert_eq!(matched(r"a\{2}b", "aab"), Some("aab"));
    assert_eq!(matched(r"a\{2\}b", "aab"), Some("aab"));
    assert_eq!(matched(r"a\{3,2\}b", "aab"), Some("aab"));
}

#[test]
fn case_flags_follow_vims_precedence() {
    // Vim's \c wins over \C regardless of where it appears.
    assert_eq!(m(r"\c\(a\)\1\C", "aA"), Some((0, 2)));
    assert_eq!(m(r"\C\(a\)\1\c", "aA"), Some((0, 2)));
    assert!(find(r"\cFOO\C", "foo", false).is_some());
    assert!(find(r"\CFOO\c", "foo", false).is_some());
    // A lone \C overrides 'ignorecase'.
    assert!(find(r"\CFOO", "foo", true).is_none());
}

#[test]
fn repeated_groups_keep_the_last_iteration() {
    let hit = caps(r"\(a\)*", "aaa").unwrap();
    assert_eq!((hit.start, hit.end), (0, 3));
    assert_eq!(hit.groups, vec![Some((2, 3))]);
    // An empty-able group matched with zero width reports an empty capture.
    let hit = caps(r"\(a*\)*", "aaa").unwrap();
    assert_eq!((hit.start, hit.end), (0, 3));
    assert_eq!(hit.groups, vec![Some((3, 3))]);
}

#[test]
fn very_magic_assertions_and_boundaries() {
    assert_eq!(m(r"\v(a)@=a", "a"), Some((0, 1)));
    assert_eq!(m(r"\v(a)@!c", "bc"), Some((1, 2)));
    assert_eq!(m(r"\v<abc>", "x abc y"), Some((2, 5)));
    assert_eq!(m(r"\v\d{2,3}", "a1234"), Some((1, 4)));
    // \V makes '^' a line anchor unless escaped; \M keeps \^ literal.
    assert!(m(r"\V\^b", "^b").is_none());
    assert_eq!(m(r"\M\^b", "^b"), Some((0, 2)));
    // Nomagic: '*' is literal, '\*' repeats.
    assert!(m(r"\Mfoo*bar", "foooobar").is_none());
    assert_eq!(m(r"\Mfoo*bar", "foo*bar"), Some((0, 7)));
    assert_eq!(m(r"\Mfoo\*bar", "foooobar"), Some((0, 8)));
}

#[test]
fn newline_collections_and_anchors() {
    // A positive collection only matches a newline with \n or the \_ prefix.
    assert!(m(r"[^\n]", "\n").is_none());
    assert_eq!(m(r"\_[^x]", "\n"), Some((0, 1)));
    assert!(m(r"[^\n]", "a").is_some());
    // ^ is not a line anchor in line-oriented mode, even after \n.
    assert!(m(r"\n^b", "a\nb").is_none());
    assert_eq!(
        find_multiline(r"\n^b", "a\nb", false).map(|m| (m.start, m.end)),
        Some((1, 3))
    );
    // $ before a trailing newline in multiline mode.
    assert_eq!(
        find_multiline(r"a$", "a\n", false).map(|m| (m.start, m.end)),
        Some((0, 1))
    );
    // \_s and friends still see a newline in line-oriented mode.
    assert_eq!(m(r"\_s*\_.", "a\nb"), Some((0, 1)));
}
