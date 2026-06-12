//! Day-number suggestion engine, ported from walpurgisbot-v2.
//!
//! In leaf this never gates archiving — it only pre-fills the day field in
//! the archive modal. High confidence = a number following a keyword like
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

static HIGH: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used, reason = "literal regex, covered by tests")]
    Regex::new(r"(?i)(?:day|daily)\s*#?(\d{1,6})\b").unwrap()
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
}
