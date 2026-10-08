//! Parsing X's timestamps.
//!
//! The GraphQL payload uses the old Ruby-era Twitter format:
//!
//! ```text
//! Wed Oct 10 20:19:24 +0000 2018
//! ```
//!
//! Note the day is **space-padded**, not zero-padded: `Oct  5`, with two
//! spaces. That detail is why this is a named format string rather than
//! something clever, and why there is a test for it.

use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;

/// `Wed Oct 10 20:19:24 +0000 2018`
const X_TIMESTAMP: &[FormatItem<'static>] = format_description!(
    "[weekday repr:short] [month repr:short] [day padding:space] \
     [hour]:[minute]:[second] [offset_hour sign:mandatory][offset_minute] [year]"
);

/// Parse an X `created_at` string into a Unix timestamp.
///
/// Returns `None` rather than erroring: a tweet whose date we cannot read is
/// still a tweet worth keeping, and X has changed this format before.
pub fn parse_x_timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    if let Ok(dt) = OffsetDateTime::parse(s, X_TIMESTAMP) {
        return Some(dt.unix_timestamp());
    }

    // Some payloads hand back RFC 3339 instead, and a few tools emit it when
    // re-exporting. Cheap to accept, so accept it.
    if let Ok(dt) = OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339) {
        return Some(dt.unix_timestamp());
    }

    None
}

/// Render a Unix timestamp the way X renders tweet dates in the timeline.
///
/// X shows `2:19 PM · Oct 10, 2018` in the detail view. We keep the formatting
/// in the frontend, but the CLI wants something readable, and more importantly
/// the *timezone semantics* need deciding once: X renders dates in the
/// viewer's local time, so we do too.
pub fn format_local(unix_seconds: i64) -> String {
    let Ok(dt) = OffsetDateTime::from_unix_timestamp(unix_seconds) else {
        return String::new();
    };
    let dt = dt.to_offset(time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC));
    format!(
        "{:02}:{:02} · {:04}-{:02}-{:02}",
        dt.hour(),
        dt.minute(),
        dt.year(),
        u8::from(dt.month()),
        dt.day()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_standard_form() {
        // 2018-10-10T20:19:24Z
        assert_eq!(parse_x_timestamp("Wed Oct 10 20:19:24 +0000 2018"), Some(1_539_202_764));
    }

    #[test]
    fn parses_a_space_padded_single_digit_day() {
        // The two-space case. This is the one that bites naive parsers.
        let ts = parse_x_timestamp("Fri Oct  5 20:19:24 +0000 2018");
        assert!(ts.is_some(), "space-padded day failed to parse");

        // Same instant as the zero-padded equivalent.
        assert_eq!(ts, parse_x_timestamp("Fri Oct 05 20:19:24 +0000 2018"));
    }

    #[test]
    fn respects_a_non_utc_offset() {
        let utc = parse_x_timestamp("Wed Oct 10 20:19:24 +0000 2018").unwrap();
        let plus2 = parse_x_timestamp("Wed Oct 10 22:19:24 +0200 2018").unwrap();
        assert_eq!(utc, plus2, "offset was ignored");
    }

    #[test]
    fn accepts_rfc3339_as_a_fallback() {
        assert_eq!(parse_x_timestamp("2018-10-10T20:19:24Z"), Some(1_539_202_764));
    }

    #[test]
    fn degrades_instead_of_panicking() {
        assert_eq!(parse_x_timestamp(""), None);
        assert_eq!(parse_x_timestamp("not a date"), None);
        assert_eq!(parse_x_timestamp("Wed Oct 10 20:19:24 +0000"), None);
    }
}
