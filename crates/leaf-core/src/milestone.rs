//! Pure milestone logic: which archived days are worth celebrating, and how
//! the announcement reads. Kept Discord-free so the thresholds are tested
//! in isolation.
//!
//! This module only answers "is this day number a milestone?". Whether to
//! announce it at all (public and active series, new highest day, recent
//! post) is the caller's decision.

use crate::domain::Cadence;

/// Why a day is a milestone (its celebratory label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Milestone {
    /// The very first archived day.
    First,
    /// A whole number of years (every 365 days).
    Years(i64),
    /// A round hundred.
    Hundred(i64),
}

impl Milestone {
    /// Short label for creator templates (`{milestone}`), e.g. "1 year" or
    /// "Day 500". Reads naturally after "reached" or "hit".
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::First => "Day 1".to_owned(),
            Self::Years(1) => "1 year".to_owned(),
            Self::Years(n) => format!("{n} years"),
            Self::Hundred(d) => format!("Day {d}"),
        }
    }
}

/// Classifies `day` for a daily series. Prefer [`classify_for`], which knows
/// that 365 posts are only a year when the series posts every day.
#[must_use]
pub const fn classify(day: i64) -> Option<Milestone> {
    classify_for(day, Cadence::Daily)
}

/// Classifies `day`, most significant first: day 1, then year marks, then
/// round hundreds. Returns `None` for ordinary days. `day` is the number
/// just archived; non-positive days never qualify.
///
/// Year marks exist only for [`Cadence::Daily`]: post 365 of a weekly or
/// weekdays series is not a year, so there it is an ordinary day (or a round
/// hundred when it happens to be one).
#[must_use]
pub const fn classify_for(day: i64, cadence: Cadence) -> Option<Milestone> {
    if day <= 0 {
        return None;
    }
    if day == 1 {
        return Some(Milestone::First);
    }
    if matches!(cadence, Cadence::Daily) && day % 365 == 0 {
        return Some(Milestone::Years(day / 365));
    }
    if day % 100 == 0 {
        return Some(Milestone::Hundred(day));
    }
    None
}

/// Renders the announcement. A creator template may use `{day}`, `{name}`,
/// `{creator}`, and `{milestone}`; absent a template, a default is used.
///
/// `creator_mention` should already be a `<@id>` mention; send the result
/// with mention parsing off so it shows the name without pinging.
#[must_use]
#[allow(
    clippy::literal_string_with_formatting_args,
    reason = "the {placeholder} literals are our own template tokens, not format args"
)]
pub fn render(
    template: Option<&str>,
    milestone: Milestone,
    day: i64,
    name: &str,
    creator_mention: &str,
) -> String {
    match template {
        Some(t) if !t.trim().is_empty() => t
            .replace("{day}", &day.to_string())
            .replace("{name}", name)
            .replace("{creator}", creator_mention)
            .replace("{milestone}", &milestone.label()),
        _ => match milestone {
            Milestone::First => {
                format!("🌱 **{name}** by {creator_mention} has begun: Day 1 is archived.")
            }
            Milestone::Years(1) => format!(
                "🎉 **{name}** by {creator_mention} reached Day {day}: one year of daily posts."
            ),
            Milestone::Years(n) => format!(
                "🎉 **{name}** by {creator_mention} reached Day {day}: {n} years of daily posts."
            ),
            Milestone::Hundred(_) => {
                format!("🎉 **{name}** by {creator_mention} reached Day {day}.")
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_priority() {
        assert_eq!(classify(1), Some(Milestone::First));
        assert_eq!(classify(100), Some(Milestone::Hundred(100)));
        assert_eq!(classify(365), Some(Milestone::Years(1)));
        assert_eq!(classify(730), Some(Milestone::Years(2)));
        // 36500 is both a hundred and 100 years — years wins (more significant).
        assert_eq!(classify(36500), Some(Milestone::Years(100)));
    }

    #[test]
    fn ordinary_days_are_not_milestones() {
        for d in [2, 7, 42, 99, 101, 364, 501] {
            assert_eq!(classify(d), None, "day {d}");
        }
        assert_eq!(classify(0), None);
        assert_eq!(classify(-5), None);
    }

    #[test]
    fn year_marks_are_for_daily_series_only() {
        for cadence in [Cadence::Weekly, Cadence::Weekdays, Cadence::Freeform] {
            // Post 365 is not a year unless the series posts every day.
            assert_eq!(classify_for(365, cadence), None, "{cadence:?}");
            assert_eq!(classify_for(730, cadence), None, "{cadence:?}");
            // Where a year mark would have hidden a round hundred, the
            // hundred shows through.
            assert_eq!(
                classify_for(36500, cadence),
                Some(Milestone::Hundred(36500)),
                "{cadence:?}"
            );
            // Everything else is cadence-independent.
            assert_eq!(classify_for(1, cadence), Some(Milestone::First));
            assert_eq!(classify_for(100, cadence), Some(Milestone::Hundred(100)));
            assert_eq!(classify_for(42, cadence), None);
            assert_eq!(classify_for(0, cadence), None);
        }
        assert_eq!(classify_for(365, Cadence::Daily), Some(Milestone::Years(1)));
    }

    #[test]
    fn labels_pluralize() {
        assert_eq!(Milestone::Years(1).label(), "1 year");
        assert_eq!(Milestone::Years(3).label(), "3 years");
        assert_eq!(Milestone::Hundred(500).label(), "Day 500");
        assert_eq!(Milestone::First.label(), "Day 1");
    }

    #[test]
    fn default_render_reads_as_a_sentence_per_variant() {
        assert_eq!(
            render(None, Milestone::First, 1, "daily-sketch", "<@7>"),
            "🌱 **daily-sketch** by <@7> has begun: Day 1 is archived."
        );
        assert_eq!(
            render(None, Milestone::Hundred(100), 100, "daily-sketch", "<@7>"),
            "🎉 **daily-sketch** by <@7> reached Day 100."
        );
        assert_eq!(
            render(None, Milestone::Years(1), 365, "daily-sketch", "<@7>"),
            "🎉 **daily-sketch** by <@7> reached Day 365: one year of daily posts."
        );
        assert_eq!(
            render(None, Milestone::Years(2), 730, "daily-sketch", "<@7>"),
            "🎉 **daily-sketch** by <@7> reached Day 730: 2 years of daily posts."
        );
    }

    #[test]
    fn template_substitutes_all_placeholders() {
        let out = render(
            Some("{creator} hit {milestone} on {name}! ({day})"),
            Milestone::Years(1),
            365,
            "art",
            "<@7>",
        );
        assert_eq!(out, "<@7> hit 1 year on art! (365)");
    }

    #[test]
    fn blank_template_falls_back_to_default() {
        let out = render(Some("   "), Milestone::First, 1, "art", "<@7>");
        assert!(out.contains("has begun"));
    }
}
