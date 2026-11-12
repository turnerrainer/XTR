//! XSD regular expressions (XSD 1.0, Appendix F) for the `pattern` facet.
//!
//! The grammar of XSD differs from the syntax of the `regex` crate:
//! a pattern always matches the whole value (no `^` and `$` operators),
//! `.` excludes only line ends, `\s` and `\d` have their own definitions,
//! and the syntax of the `regex` crate that XSD does not define is an
//! error. The parser accepts the XSD grammar only and writes an
//! anchored `regex` expression. Pattern constructs outside the supported
//! subset are errors, so that a boot fails and no value passes by
//! accident. Compilation has limits for the size of the pattern and of
//! the automaton.

use regex::{Regex, RegexBuilder};
use std::fmt::Write;

const MAX_PATTERN_CHARS: usize = 1000;
const MAX_GROUP_DEPTH: usize = 20;
const MAX_REPEAT: u32 = 1000;
const SIZE_LIMIT: usize = 1 << 20;

/// General category names that XSD defines (XSD 1.0, Appendix F). Block
/// names (`IsBasicLatin`) are not supported.
const CATEGORIES: &[&str] = &[
    "L", "Lu", "Ll", "Lt", "Lm", "Lo", "M", "Mn", "Mc", "Me", "N", "Nd", "Nl", "No", "P", "Pc",
    "Pd", "Ps", "Pe", "Pi", "Pf", "Po", "Z", "Zs", "Zl", "Zp", "S", "Sm", "Sc", "Sk", "So", "C",
    "Cc", "Cf", "Co", "Cn",
];

/// Compiles an XSD pattern into a regular expression that matches
/// the whole value.
pub fn compile(pattern: &str) -> Result<Regex, String> {
    let chars: Vec<char> = pattern.chars().collect();
    if chars.len() > MAX_PATTERN_CHARS {
        return Err(format!(
            "pattern longer than {MAX_PATTERN_CHARS} characters"
        ));
    }
    let mut p = Parser {
        c: chars,
        i: 0,
        out: String::new(),
        depth: 0,
    };
    p.alternation()?;
    if p.i != p.c.len() {
        return Err("unbalanced ')'".into());
    }
    let src = format!("\\A(?:{})\\z", p.out);
    RegexBuilder::new(&src)
        .size_limit(SIZE_LIMIT)
        .dfa_size_limit(SIZE_LIMIT)
        .nest_limit(64)
        .build()
        .map_err(|_| "pattern is too complex".to_string())
}

struct Parser {
    c: Vec<char>,
    i: usize,
    out: String,
    depth: usize,
}

/// One item of a character class that can start a range.
enum Single {
    Char(char),
    Class,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.c.get(self.i + n).copied()
    }

    fn lit(&mut self, ch: char) {
        push_lit(&mut self.out, ch);
    }

    fn alternation(&mut self) -> Result<(), String> {
        loop {
            while !matches!(self.peek(), None | Some('|') | Some(')')) {
                self.piece()?;
            }
            if self.peek() == Some('|') {
                self.out.push('|');
                self.i += 1;
            } else {
                return Ok(());
            }
        }
    }

    fn piece(&mut self) -> Result<(), String> {
        self.atom()?;
        match self.peek() {
            Some(q @ ('?' | '*' | '+')) => {
                self.out.push(q);
                self.i += 1;
            }
            Some('{') => self.repeat()?,
            _ => {}
        }
        Ok(())
    }

    fn number(&mut self) -> Result<u32, String> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if start == self.i {
            return Err("number expected in a quantifier".into());
        }
        let n: u32 = self.c[start..self.i]
            .iter()
            .collect::<String>()
            .parse()
            .map_err(|_| "number too large in a quantifier".to_string())?;
        if n > MAX_REPEAT {
            return Err(format!("quantifier above {MAX_REPEAT}"));
        }
        Ok(n)
    }

    fn repeat(&mut self) -> Result<(), String> {
        self.i += 1; // '{'
        let min = self.number()?;
        let mut max = Some(min);
        let mut open = false;
        if self.peek() == Some(',') {
            self.i += 1;
            if self.peek() == Some('}') {
                open = true;
                max = None;
            } else {
                max = Some(self.number()?);
            }
        }
        if self.peek() != Some('}') {
            return Err("'}' expected after a quantifier".into());
        }
        self.i += 1;
        if max.is_some_and(|m| m < min) {
            return Err("quantifier maximum below minimum".into());
        }
        let _ = match (max, open) {
            (Some(m), _) if m == min => write!(self.out, "{{{min}}}"),
            (Some(m), _) => write!(self.out, "{{{min},{m}}}"),
            (None, _) => write!(self.out, "{{{min},}}"),
        };
        Ok(())
    }

    fn atom(&mut self) -> Result<(), String> {
        let Some(ch) = self.peek() else {
            return Err("atom expected".into());
        };
        match ch {
            '(' => {
                if self.depth >= MAX_GROUP_DEPTH {
                    return Err("groups nested too deep".into());
                }
                self.depth += 1;
                self.i += 1;
                self.out.push_str("(?:");
                self.alternation()?;
                if self.peek() != Some(')') {
                    return Err("')' expected".into());
                }
                self.i += 1;
                self.out.push(')');
                self.depth -= 1;
            }
            '[' => self.class()?,
            '.' => {
                self.i += 1;
                self.out.push_str("[^\\n\\r]");
            }
            '\\' => {
                self.i += 1;
                match self.escape(false)? {
                    Single::Char(c) => self.lit(c),
                    Single::Class => {}
                }
            }
            '?' | '*' | '+' | '{' | '}' | ']' | ')' | '|' => {
                return Err(format!("unexpected '{ch}'"));
            }
            c => {
                self.i += 1;
                self.lit(c);
            }
        }
        Ok(())
    }

    /// After a backslash. A single-character escape returns the
    /// character. A class escape writes its class to the output.
    fn escape(&mut self, in_class: bool) -> Result<Single, String> {
        let Some(e) = self.peek() else {
            return Err("pattern ends after a backslash".into());
        };
        self.i += 1;
        let single = match e {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '\\' | '|' | '.' | '-' | '^' | '?' | '*' | '+' | '{' | '}' | '(' | ')' | '[' | ']' => e,
            'd' => return Ok(self.emit_class(in_class, "\\p{Nd}", None)),
            'D' => return Ok(self.emit_class(in_class, "\\P{Nd}", None)),
            's' => return Ok(self.emit_class(in_class, "\\x{20}\\x{9}\\x{a}\\x{d}", Some(false))),
            'S' => return Ok(self.emit_class(in_class, "\\x{20}\\x{9}\\x{a}\\x{d}", Some(true))),
            'w' => return Ok(self.emit_class(in_class, "\\p{P}\\p{Z}\\p{C}", Some(true))),
            'W' => return Ok(self.emit_class(in_class, "\\p{P}\\p{Z}\\p{C}", Some(false))),
            'p' | 'P' => {
                if self.peek() != Some('{') {
                    return Err("'{' expected after \\p".into());
                }
                let end = self.c[self.i..]
                    .iter()
                    .position(|&c| c == '}')
                    .ok_or("'}' expected after \\p{")?;
                let name: String = self.c[self.i + 1..self.i + end].iter().collect();
                self.i += end + 1;
                if name.starts_with("Is") {
                    return Err("block escapes (\\p{Is...}) are not supported".into());
                }
                if !CATEGORIES.contains(&name.as_str()) {
                    return Err("unknown category in \\p{...}".into());
                }
                let body = format!("\\{e}{{{name}}}");
                return Ok(self.emit_class(in_class, &body, None));
            }
            'i' | 'I' | 'c' | 'C' => {
                return Err("the escapes \\i, \\I, \\c and \\C are not supported".into())
            }
            _ => return Err("unknown escape".into()),
        };
        Ok(Single::Char(single))
    }

    /// Writes a class escape. `body` is a set of class items. `neg`
    /// `None`: `body` is a single item that is valid inside and
    /// outside a bracket class. `Some(false)`: the set of the items;
    /// `Some(true)`: the complement of the set.
    fn emit_class(&mut self, in_class: bool, body: &str, neg: Option<bool>) -> Single {
        match (neg, in_class) {
            (None, false) => self.out.push_str(&format!("[{body}]")),
            (None, true) => self.out.push_str(body),
            (Some(false), false) => self.out.push_str(&format!("[{body}]")),
            (Some(true), false) => self.out.push_str(&format!("[^{body}]")),
            (Some(false), true) => self.out.push_str(body),
            (Some(true), true) => self.out.push_str(&format!("[^{body}]")),
        }
        Single::Class
    }

    fn class(&mut self) -> Result<(), String> {
        self.i += 1; // '['
        let negated = self.peek() == Some('^');
        if negated {
            self.i += 1;
        }
        let saved = std::mem::take(&mut self.out);
        let result = self.class_items();
        let items = std::mem::replace(&mut self.out, saved);
        result?;
        self.out.push('[');
        if negated {
            self.out.push('^');
        }
        self.out.push_str(&items);
        self.out.push(']');
        Ok(())
    }

    fn class_items(&mut self) -> Result<(), String> {
        let mut count = 0usize;
        loop {
            let Some(ch) = self.peek() else {
                return Err("']' expected".into());
            };
            match ch {
                ']' => {
                    self.i += 1;
                    return if count == 0 {
                        Err("empty character class".into())
                    } else {
                        Ok(())
                    };
                }
                '[' => return Err("'[' inside a character class".into()),
                '-' => {
                    if self.peek_at(1) == Some('[') {
                        return Err("character class subtraction is not supported".into());
                    }
                    if count == 0 || self.peek_at(1) == Some(']') {
                        self.i += 1;
                        push_lit(&mut self.out, '-');
                    } else {
                        return Err("'-' in the middle of a character class".into());
                    }
                }
                _ => {
                    let lo = self.class_single()?;
                    if let Single::Char(lo) = lo {
                        if self.peek() == Some('-')
                            && self.peek_at(1) != Some(']')
                            && self.peek_at(1) != Some('[')
                        {
                            self.i += 1;
                            let Single::Char(hi) = self.class_single()? else {
                                return Err("a class escape cannot end a range".into());
                            };
                            if lo > hi {
                                return Err("range start above range end".into());
                            }
                            push_lit(&mut self.out, lo);
                            self.out.push('-');
                            push_lit(&mut self.out, hi);
                        } else {
                            push_lit(&mut self.out, lo);
                        }
                    }
                }
            }
            count += 1;
        }
    }

    /// One literal character, single-character escape or class escape
    /// inside a class. A literal or a single-character escape is
    /// returned, not written.
    fn class_single(&mut self) -> Result<Single, String> {
        let Some(ch) = self.peek() else {
            return Err("']' expected".into());
        };
        self.i += 1;
        if ch == '\\' {
            self.escape(true)
        } else {
            Ok(Single::Char(ch))
        }
    }
}

/// Writes one literal character so that no `regex` operator can read it.
fn push_lit(out: &mut String, ch: char) {
    if ch.is_ascii_alphanumeric() || !ch.is_ascii() {
        out.push(ch);
    } else {
        let _ = write!(out, "\\x{{{:x}}}", ch as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(p: &str) -> Regex {
        compile(p).unwrap_or_else(|e| panic!("{p:?} must compile: {e}"))
    }

    fn matches(p: &str, v: &str) -> bool {
        re(p).is_match(v)
    }

    fn refused(p: &str) {
        assert!(compile(p).is_err(), "{p:?} must be refused");
    }

    #[test]
    fn a_pattern_matches_the_whole_value() {
        assert!(matches("ab", "ab"));
        assert!(!matches("ab", "xab"));
        assert!(!matches("ab", "abx"));
        assert!(!matches("ab", "ab\n"));
        assert!(matches("", ""));
        assert!(!matches("", "a"));
        assert!(matches("a|", "a") && matches("a|", ""));
    }

    #[test]
    fn caret_and_dollar_are_literal_characters() {
        assert!(matches("^a$", "^a$"));
        assert!(!matches("^a$", "a"));
    }

    #[test]
    fn dot_excludes_only_line_ends() {
        assert!(matches(".", "x") && matches(".", "\u{e4}") && matches(".", "\t"));
        assert!(!matches(".", "\n") && !matches(".", "\r"));
        assert!(matches(".{19}Z", "2026-11-12T10:00:00Z"));
        assert!(!matches(".{19}Z", "2026-11-12T10:00:00"));
    }

    #[test]
    fn backslash_s_is_the_four_xml_white_space_characters() {
        for c in [" ", "\t", "\n", "\r"] {
            assert!(matches(r"\s", c));
        }
        for c in ["\u{a0}", "\u{2003}", "\u{c}", "a"] {
            assert!(!matches(r"\s", c), "{c:?}");
        }
        assert!(matches(r"\S", "a") && !matches(r"\S", " "));
    }

    #[test]
    fn backslash_d_is_the_decimal_digit_category() {
        assert!(matches(r"\d", "5") && matches(r"\d", "\u{663}"));
        assert!(!matches(r"\d", "a"));
        assert!(matches(r"\d{4}-\d{2}-\d{2}", "2026-11-12"));
        assert!(!matches(r"\d{4}-\d{2}-\d{2}", "2026-1-12"));
    }

    #[test]
    fn backslash_w_excludes_punctuation_separators_and_other() {
        assert!(matches(r"\w+", "ab\u{e9}1"));
        for c in [" ", ".", "-", "\n", "\u{2003}"] {
            assert!(!matches(r"\w", c), "{c:?}");
        }
        assert!(matches(r"\W", "-") && !matches(r"\W", "a"));
    }

    #[test]
    fn character_classes() {
        assert!(matches("[0-9a-fA-F]{8}", "09afAF00"));
        assert!(!matches("[0-9a-fA-F]{8}", "09afAG00"));
        assert!(matches("[^abc]", "d") && !matches("[^abc]", "b"));
        assert!(matches("[a-]", "-") && matches("[a-]", "a"));
        assert!(matches("[-a]", "-"));
        assert!(matches(r"[\n\t]", "\t") && matches(r"[\\]", "\\") && matches(r"[\-]", "-"));
        assert!(matches(r"[\d_]", "_") && matches(r"[\d_]", "7"));
        assert!(matches("[^^]", "a") && !matches("[^^]", "^"));
        assert!(matches("[a^]", "^"));
        assert!(matches(r"[\p{Lu}0-9]", "\u{c4}"));
    }

    #[test]
    fn regex_crate_class_operators_are_literal_characters_in_xsd() {
        assert!(matches("[a&&b]", "&") && matches("[a&&b]", "a") && matches("[a&&b]", "b"));
        assert!(matches("[a~~b]", "~"));
        assert!(matches("[a|b]", "|"));
    }

    #[test]
    fn groups_alternation_and_quantifiers() {
        assert!(matches("(a|b)+c", "abbac") && !matches("(a|b)+c", "c"));
        assert!(matches("a{2,3}", "aa") && matches("a{2,3}", "aaa"));
        assert!(!matches("a{2,3}", "a") && !matches("a{2,3}", "aaaa"));
        assert!(matches("a{2}", "aa") && matches("a{2,}", "aaaaa"));
        assert!(matches("a?b*c+", "c") && matches("a?b*c+", "abbcc"));
        assert!(matches(r"\.\?\*\+\{\}\(\)\|\[\]\^$", ".?*+{}()|[]^$"));
    }

    #[test]
    fn general_categories() {
        assert!(matches(r"\p{Lu}+", "\u{c4}B") && !matches(r"\p{Lu}", "a"));
        assert!(matches(r"\P{Lu}", "a") && !matches(r"\P{Lu}", "A"));
        assert!(matches(r"\p{N}", "5") && matches(r"\p{Sc}", "$"));
    }

    #[test]
    fn the_erru_patterns_work() {
        assert!(matches(
            "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
            "123e4567-e89b-12d3-a456-426614174000"
        ));
        assert!(matches("[A-HJ-NPR-Z0-9]{17}", "WVWZZZ1JZXW000001"));
    }

    #[test]
    fn syntax_that_only_the_regex_crate_knows_is_refused() {
        for p in [
            "(?i)a",
            "(?:a)",
            "\\b",
            "\\B",
            "\\A",
            "\\z",
            "a*?",
            "a+?",
            "a{2,}?",
            "\\x41",
            "\\u0041",
            "\\x{41}",
            "[[:alpha:]]",
            "(?P<n>a)",
            "\\1",
            "\\pL",
            "a**",
            "a{,3}",
            "\\e",
            "\\a",
            "\\f",
            "\\v",
        ] {
            refused(p);
        }
    }

    #[test]
    fn constructs_that_xsd_defines_but_the_subset_leaves_out_are_refused() {
        for p in [
            r"\i",
            r"\I",
            r"\c",
            r"\C",
            r"\p{IsBasicLatin}",
            r"\P{IsGreek}",
            "[a-z-[aeiou]]",
            r"[\d-[5]]",
            r"\p{Foo}",
            r"\p{}",
        ] {
            refused(p);
        }
    }

    #[test]
    fn malformed_patterns_are_refused() {
        for p in [
            "(a", "a)", "[a", "a]", "[]", "[^]", "a{", "a}", "a{3,2}", "a{x}", "*a", "+a", "?a",
            "{2}", "|*", "[z-a]", "a\\", "\\", "[a-b-c]",
        ] {
            refused(p);
        }
    }

    #[test]
    fn limits_stop_huge_patterns() {
        refused(&"a".repeat(5_000));
        refused(&format!("{}a{}", "(".repeat(100), ")".repeat(100)));
        refused("a{1,100000}");
        refused("(a{1000}){1000}");
        refused(r"\p{L}{1000}{1000}");
    }
}
