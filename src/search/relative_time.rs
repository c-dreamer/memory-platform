//! Relative-time phrase parsing for search queries.
//!
//! Turns "what did I decide last week" into a date range so results from
//! that window can be boosted. Deliberately a *soft* signal: the range
//! adjusts ranking, it never filters, so a query whose phrasing happens to
//! look time-scoped can't hide an otherwise-relevant result.
//!
//! Windows are rolling (`last week` = 7–14 days before now), not calendar
//! boundaries. Calendar-exact windows would need a session timezone, which
//! nothing in this codebase pins; for a ranking hint the difference is not
//! worth that dependency.

use chrono::{DateTime, Duration, Utc};

/// Phrase → (days ago the window starts, days ago it ends).
///
/// Ordered longest-match-first: "last week" must be tested before "week",
/// otherwise the shorter phrase swallows it.
const PHRASES: &[(&str, i64, i64)] = &[
    ("last year", 730, 365),
    ("this year", 365, 0),
    ("last month", 60, 30),
    ("this month", 30, 0),
    ("last week", 14, 7),
    ("this week", 7, 0),
    ("past week", 7, 0),
    ("yesterday", 2, 1),
    ("today", 1, 0),
    ("recently", 7, 0),
];

/// Parse a relative-time phrase out of `query`, resolved against `now`.
///
/// Returns `(start, end)` with `start < end`, or `None` when the query
/// carries no recognized time phrase — which is the common case, so callers
/// can skip any extra work entirely.
#[must_use]
pub fn parse_range(query: &str, now: DateTime<Utc>) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let q = query.to_lowercase();

    if let Some(days) = parse_n_days(&q) {
        return Some((now - Duration::days(days), now));
    }

    PHRASES
        .iter()
        .find(|(phrase, _, _)| q.contains(phrase))
        .map(|(_, start_days, end_days)| {
            (
                now - Duration::days(*start_days),
                now - Duration::days(*end_days),
            )
        })
}

/// Match "last 7 days" / "past 30 days" and return the day count.
///
/// Capped at 3650 so a wild number can't produce a range that overflows
/// `Duration` arithmetic.
fn parse_n_days(q: &str) -> Option<i64> {
    let tokens: Vec<&str> = q.split_whitespace().collect();
    tokens.windows(3).find_map(|w| match w {
        [lead, count, unit] if (*lead == "last" || *lead == "past") && unit.starts_with("day") => {
            count
                .parse::<i64>()
                .ok()
                .filter(|n| *n > 0)
                .map(|n| n.min(3650))
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-21T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn no_time_phrase_returns_none() {
        assert!(parse_range("how does the decay engine work", now()).is_none());
    }

    #[test]
    fn last_week_is_seven_to_fourteen_days_back() {
        let (start, end) = parse_range("what did I decide last week", now()).unwrap();
        assert_eq!((now() - start).num_days(), 14);
        assert_eq!((now() - end).num_days(), 7);
        assert!(start < end);
    }

    #[test]
    fn this_week_ends_at_now() {
        let (start, end) = parse_range("notes from this week", now()).unwrap();
        assert_eq!((now() - start).num_days(), 7);
        assert_eq!(end, now());
    }

    #[test]
    fn longer_phrase_wins_over_shorter() {
        // "last month" must not be parsed as "this month" or a bare "month".
        let (start, end) = parse_range("the migration last month", now()).unwrap();
        assert_eq!((now() - start).num_days(), 60);
        assert_eq!((now() - end).num_days(), 30);
    }

    #[test]
    fn explicit_day_count_parses() {
        let (start, end) = parse_range("errors in the last 30 days", now()).unwrap();
        assert_eq!((now() - start).num_days(), 30);
        assert_eq!(end, now());
    }

    #[test]
    fn explicit_day_count_beats_phrase_table() {
        // Contains "today" as a substring of nothing, but does contain a
        // numeric form that should take priority over any phrase match.
        let (start, _) = parse_range("past 3 days of work this week", now()).unwrap();
        assert_eq!((now() - start).num_days(), 3);
    }

    #[test]
    fn zero_and_garbage_day_counts_fall_through() {
        assert!(parse_range("last 0 days", now()).is_none());
        assert!(parse_range("last many days", now()).is_none());
    }

    #[test]
    fn absurd_day_count_is_capped() {
        let (start, _) = parse_range("last 99999999 days", now()).unwrap();
        assert_eq!((now() - start).num_days(), 3650);
    }

    #[test]
    fn case_is_ignored() {
        assert!(parse_range("What Happened YESTERDAY", now()).is_some());
    }

    #[test]
    fn every_phrase_yields_a_forward_range() {
        for (phrase, _, _) in PHRASES {
            let (start, end) = parse_range(phrase, now()).unwrap();
            assert!(start < end, "{phrase} produced an inverted range");
        }
    }
}
