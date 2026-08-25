//! Temporal validity helpers for bitemporal facts.
//!
//! Records carry a validity window (`valid_from`/`valid_to`) describing
//! when the fact was true in the real world, separate from when the row
//! was written.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A closed-open interval `[from, to)` during which a fact holds.
///
/// `None` bounds mean unbounded past or future respectively.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidityWindow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<DateTime<Utc>>,
}

impl ValidityWindow {
    /// Window covering all time.
    pub const ALWAYS: ValidityWindow = ValidityWindow {
        from: None,
        to: None,
    };

    /// Window starting at `from` with no end.
    pub fn from_instant(from: DateTime<Utc>) -> Self {
        Self {
            from: Some(from),
            to: None,
        }
    }

    /// True when `at` falls inside the window.
    pub fn contains(&self, at: DateTime<Utc>) -> bool {
        self.from.is_none_or(|f| f <= at) && self.to.is_none_or(|t| at < t)
    }

    /// True if the two windows share at least one instant.
    pub fn overlaps(&self, other: &ValidityWindow) -> bool {
        let starts_before_other_ends = other.to.is_none_or(|o| {
            self.from.is_none_or(|s| s < o)
        });
        let ends_after_other_starts = other.from.is_none_or(|o| {
            self.to.is_none_or(|e| o < e)
        });
        starts_before_other_ends && ends_after_other_starts
    }

    /// True if this window has already ended at `now`.
    pub fn has_ended(&self, now: DateTime<Utc>) -> bool {
        self.to.is_some_and(|t| t <= now)
    }
}

impl Default for ValidityWindow {
    fn default() -> Self {
        Self::ALWAYS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn at(hours: i64) -> DateTime<Utc> {
        Utc::now() + Duration::hours(hours)
    }

    #[test]
    fn always_contains_everything() {
        assert!(ValidityWindow::ALWAYS.contains(at(-100)));
        assert!(ValidityWindow::ALWAYS.contains(at(100)));
    }

    #[test]
    fn bounded_window_is_half_open() {
        let start = at(0);
        let end = at(10);
        let w = ValidityWindow {
            from: Some(start),
            to: Some(end),
        };
        assert!(!w.contains(start - Duration::seconds(1)));
        assert!(w.contains(start));
        assert!(w.contains(end - Duration::seconds(1)));
        assert!(!w.contains(end), "to is exclusive");
    }

    #[test]
    fn overlap_detects_partial_and_adjacent_cases() {
        let a = ValidityWindow {
            from: Some(at(0)),
            to: Some(at(10)),
        };
        let overlapping = ValidityWindow {
            from: Some(at(5)),
            to: Some(at(20)),
        };
        let adjacent = ValidityWindow {
            from: Some(at(10)),
            to: Some(at(20)),
        };
        let disjoint = ValidityWindow {
            from: Some(at(50)),
            to: Some(at(60)),
        };

        assert!(a.overlaps(&overlapping));
        assert!(!a.overlaps(&adjacent), "adjacent half-open windows do not overlap");
        assert!(!a.overlaps(&disjoint));
    }

    #[test]
    fn ended_windows_report_completion() {
        let w = ValidityWindow {
            from: Some(at(-10)),
            to: Some(at(-1)),
        };
        assert!(w.has_ended(at(0)));

        let open = ValidityWindow::from_instant(at(-1));
        assert!(!open.has_ended(at(0)));
    }

    #[test]
    fn serde_roundtrip_of_unbounded_window() {
        let json = serde_json::to_string(&ValidityWindow::ALWAYS).expect("serialize");
        assert_eq!(json, "{}");
        let back: ValidityWindow = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ValidityWindow::ALWAYS);
    }
}
