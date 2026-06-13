//! Pure milestone logic: which archived days are worth celebrating, and how
//! the announcement reads. Kept Discord-free so the thresholds are tested
//! in isolation.

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
    /// Short human label, e.g. "1 year" or "day 500".
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::First => "the first post".to_owned(),
            Self::Years(1) => "1 year".to_owned(),
            Self::Years(n) => format!("{n} years"),
            Self::Hundred(d) => format!("day {d}"),
        }
    }
}

/// Classifies `day`, most significant first: day 1, then year marks, then
/// round hundreds. Returns `None` for ordinary days. `day` is the number
/// just archived; non-positive days never qualify.
#[must_use]
pub const fn classify(day: i64) -> Option<Milestone> {
    if day <= 0 {
        return None;
    }
    if day == 1 {
        return Some(Milestone::First);
    }
    if day % 365 == 0 {
        return Some(Milestone::Years(day / 365));
    }
    if day % 100 == 0 {
        return Some(Milestone::Hundred(day));
    }
    None
}

/// Renders the announcement. A creator template may use `{day}`, `{name}`,
/// `{creator}`, and `{milestone}`; absent a template, a default is used.
/// `creator_mention` should already be a `<@id>` mention.
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
        _ => format!(
            "🎉 **{name}** reached {} — Day {day}, archived by {creator_mention}. 🍃",
            milestone.label()
        ),
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
    fn labels_pluralize() {
        assert_eq!(Milestone::Years(1).label(), "1 year");
        assert_eq!(Milestone::Years(3).label(), "3 years");
        assert_eq!(Milestone::Hundred(500).label(), "day 500");
        assert_eq!(Milestone::First.label(), "the first post");
    }

    #[test]
    fn default_render_mentions_everything() {
        let out = render(None, Milestone::Hundred(100), 100, "daily-sketch", "<@7>");
        assert!(out.contains("daily-sketch"));
        assert!(out.contains("Day 100"));
        assert!(out.contains("<@7>"));
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
        assert!(out.contains("the first post"));
    }
}
