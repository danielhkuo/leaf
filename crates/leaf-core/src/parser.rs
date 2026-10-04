//! Day-number suggestion engine, ported from walpurgisbot-v2, plus the
//! lenient reader for the day a creator types into the archive modal.
//!
//! The suggestion never gates archiving — it only pre-fills the day field in
//! the archive modal. High confidence = a number following the whole word
//! "day"/"daily"; low = any standalone number.

use regex_lite::Regex;
use std::sync::LazyLock;

/// How sure the parser is that the numbers are day numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Keyword-adjacent number ("Day 101", "daily #7").
    High,
    /// Bare number with no keyword.
    Low,
    /// No numbers at all.
    None,
}

/// Parse outcome: every matched number, in order of appearance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResult {
    /// Detected day numbers.
    pub days: Vec<i64>,
    /// Detection confidence.
    pub confidence: Confidence,
}

/// Highest day number leaf accepts anywhere (six digits, matching the
/// suggestion patterns below).
pub const MAX_DAY: i64 = 999_999;

// The leading `\b` keeps "Friday 13", "birthday 30" and "today 3" out. There
// is deliberately no boundary after the keyword, so "day107" still matches.
static HIGH: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used, reason = "literal regex, covered by tests")]
    Regex::new(r"(?i)\b(?:day|daily)\s*#?(\d{1,6})\b").unwrap()
});
static LOW: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used, reason = "literal regex, covered by tests")]
    Regex::new(r"\b(\d{1,6})\b").unwrap()
});

/// Extracts candidate day numbers from message content.
#[must_use]
pub fn parse_day_numbers(content: &str) -> ParseResult {
    let collect = |re: &Regex| -> Vec<i64> {
        re.captures_iter(content)
            .filter_map(|c| c.get(1))
            .filter_map(|m| m.as_str().parse().ok())
            .collect()
    };

    let high = collect(&HIGH);
    if !high.is_empty() {
        return ParseResult {
            days: high,
            confidence: Confidence::High,
        };
    }
    let low = collect(&LOW);
    if low.is_empty() {
        ParseResult {
            days: Vec::new(),
            confidence: Confidence::None,
        }
    } else {
        ParseResult {
            days: low,
            confidence: Confidence::Low,
        }
    }
}

/// The single day to pre-fill in the archive modal, if the parser found a
/// confident, unambiguous one.
#[must_use]
pub fn suggested_day(content: &str) -> Option<i64> {
    let parsed = parse_day_numbers(content);
    match (parsed.confidence, parsed.days.as_slice()) {
        (Confidence::High, [one]) => Some(*one),
        _ => None,
    }
}

/// Why typed day input was not accepted. `Display` is the user-facing
/// message; it never echoes the input, so it is safe for any length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DayInputError {
    /// No digits at all.
    #[error("That isn't a day number. Enter just the number, for example 42.")]
    NoNumber,
    /// More than one separate run of digits ("42 43", "1,000", "4.5").
    #[error("That has more than one number in it. Enter just the day, for example 42.")]
    SeveralNumbers,
    /// Zero, negative, or above [`MAX_DAY`].
    #[error("Day numbers go from 1 to 999999. Enter a number in that range.")]
    OutOfRange,
}

/// Value of an ASCII or full-width (`０`–`９`, common on CJK keyboards)
/// decimal digit.
fn digit_value(c: char) -> Option<i64> {
    let ascii = match c {
        '０'..='９' => char::from_u32(u32::from(c) - u32::from('０') + u32::from('0'))?,
        _ => c,
    };
    ascii.to_digit(10).map(i64::from)
}

/// Reads the day a creator typed into the archive modal.
///
/// Lenient about decoration: the input must contain exactly one run of
/// digits, and everything around it is ignored, so `42`, `Day 42`, `#42`,
/// `42.` and `４２` all read as 42. The result is always in `1..=MAX_DAY`.
///
/// A minus sign directly in front of the number makes it negative (`-5`,
/// `Day -5`, `#-5`), unless it joins the number to a word (`day-5` is 5).
///
/// # Errors
/// [`DayInputError`] when there is no number, more than one, or the number
/// is out of range (including a negative one).
pub fn parse_day_input(input: &str) -> Result<i64, DayInputError> {
    let mut value: Option<i64> = None;
    let mut negative = false;
    let mut in_run = false;
    let mut runs = 0_u32;
    // The character before `c`, and the one before that.
    let mut prev: Option<char> = None;
    let mut before_prev: Option<char> = None;
    for c in input.trim().chars() {
        if let Some(digit) = digit_value(c) {
            if !in_run {
                in_run = true;
                runs += 1;
                if runs > 1 {
                    return Err(DayInputError::SeveralNumbers);
                }
                // "-5" is one digit run, but reading it as Day 5 would be a
                // surprise. After a letter the sign is a hyphen instead.
                negative = matches!(prev, Some('-' | '−'))
                    && !before_prev.is_some_and(char::is_alphanumeric);
            }
            // Saturate just past the bound so a very long run cannot overflow.
            let next = value.unwrap_or(0) * 10 + digit;
            value = Some(next.min(MAX_DAY + 1));
        } else {
            in_run = false;
        }
        before_prev = prev;
        prev = Some(c);
    }

    match value {
        None => Err(DayInputError::NoNumber),
        Some(day) if negative || !(1..=MAX_DAY).contains(&day) => Err(DayInputError::OutOfRange),
        Some(day) => Ok(day),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_numbers_are_high_confidence() {
        for (text, days) in [
            ("Day 100", vec![100]),
            ("daily #50", vec![50]),
            ("DAY    7", vec![7]),
            ("day107 and day 108", vec![107, 108]),
            ("Daily 3 of trying", vec![3]),
        ] {
            let r = parse_day_numbers(text);
            assert_eq!((r.confidence, r.days), (Confidence::High, days), "{text}");
        }
    }

    #[test]
    fn words_ending_in_day_are_not_keywords() {
        // Regression: these used to pre-fill the modal with high confidence.
        for (text, days) in [
            ("Friday 13 sketch", vec![13]),
            ("Monday 5", vec![5]),
            ("birthday 30", vec![30]),
            ("holiday 2026", vec![2026]),
            ("today 3", vec![3]),
            ("yesterday 2", vec![2]),
            ("everyday 4", vec![4]),
        ] {
            let r = parse_day_numbers(text);
            assert_eq!((r.confidence, r.days), (Confidence::Low, days), "{text}");
            assert_eq!(suggested_day(text), None, "{text}");
        }
        // The keyword still wins when it appears as its own word.
        assert_eq!(suggested_day("Friday the 13th, day 42"), Some(42));
        assert_eq!(suggested_day("birthday sketch (day107)"), Some(107));
    }

    #[test]
    fn bare_numbers_are_low_confidence() {
        let r = parse_day_numbers("just posting 50");
        assert_eq!((r.confidence, r.days), (Confidence::Low, vec![50]));
        let r = parse_day_numbers("100 then 200");
        assert_eq!((r.confidence, r.days), (Confidence::Low, vec![100, 200]));
    }

    #[test]
    fn no_numbers_is_none() {
        let r = parse_day_numbers("hello world");
        assert_eq!((r.confidence, r.days), (Confidence::None, vec![]));
        assert_eq!(parse_day_numbers("").confidence, Confidence::None);
    }

    #[test]
    fn huge_numbers_are_ignored_by_length() {
        // 7+ digit runs match neither pattern (sanity bound): the trailing
        // word boundary stops a partial 6-digit bite out of a longer run.
        assert_eq!(
            parse_day_numbers("day 99999999").confidence,
            Confidence::None
        );
        assert_eq!(parse_day_numbers("99999999").confidence, Confidence::None);
        // ...but 6 digits exactly is still fine.
        assert_eq!(parse_day_numbers("day 999999").days, vec![999_999]);
    }

    #[test]
    fn suggestion_requires_single_high_confidence_match() {
        assert_eq!(suggested_day("Day 42"), Some(42));
        assert_eq!(suggested_day("Day 1 or day 2"), None); // ambiguous
        assert_eq!(suggested_day("just 42"), None); // low confidence
        assert_eq!(suggested_day("no numbers"), None);
    }

    #[test]
    fn day_input_ignores_decoration_around_one_number() {
        for (text, day) in [
            ("42", 42),
            ("  42  ", 42),
            ("Day 42", 42),
            ("day42", 42),
            ("#42", 42),
            ("42.", 42),
            ("４２", 42),
            ("Day ４２!", 42),
            ("day-42", 42), // a hyphen after a word is not a minus sign
            ("42-", 42),
            ("007", 7),
            ("1", 1),
            ("999999", MAX_DAY),
        ] {
            assert_eq!(parse_day_input(text), Ok(day), "{text}");
        }
    }

    #[test]
    fn day_input_rejects_missing_ambiguous_and_out_of_range() {
        for text in ["", "   ", "soon", "#", "day"] {
            assert_eq!(
                parse_day_input(text),
                Err(DayInputError::NoNumber),
                "{text}"
            );
        }
        for text in ["42 43", "1,000", "4.5", "day 4 of 5", "4２ 3"] {
            assert_eq!(
                parse_day_input(text),
                Err(DayInputError::SeveralNumbers),
                "{text}"
            );
        }
        for text in [
            "0",
            "000",
            "-5",
            "−5",
            "Day -5",
            "day: −5",
            "#-5",
            "1000000",
            "99999999999999999999999999",
        ] {
            assert_eq!(
                parse_day_input(text),
                Err(DayInputError::OutOfRange),
                "{text}"
            );
        }
    }

    #[test]
    fn day_input_errors_never_echo_the_input() {
        // A long paste must not push a reply past Discord's 2000 characters.
        let long = "x".repeat(5000);
        let message = parse_day_input(&long).map_err(|e| e.to_string());
        assert!(message.is_err_and(|m| m.len() < 120));
    }
}
