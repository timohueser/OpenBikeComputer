//! Canonical UTC time formatting, shared by the manifest, the object keys and the adapters.
//!
//! Three functions and one convention: every timestamp the bakery writes anywhere is UTC to the
//! second, and every object key segment is UTC to the minute. Keeping them here rather than in the
//! manifest module is what stopped an adapter from formatting a timestamp its own way — a source's
//! own log line and the document that names its bake have to agree to the second, or a replay is
//! comparing two different instants.

use chrono::{DateTime, Utc};

/// Canonical UTC second formatting for every timestamp the bakery writes.
pub fn rfc3339(unix: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix, 0)
        .map(|time| time.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| format!("invalid-{unix}"))
}

pub fn parse_rfc3339(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text).ok().map(|time| time.timestamp())
}

/// The `<generation>` key segment: a cycle's reference time, minute precision.
pub fn key_timestamp(unix: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix, 0)
        .map(|time| time.format("%Y%m%dT%H%MZ").to_string())
        .unwrap_or_else(|| format!("invalid-{unix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_round_trip_canonically() {
        assert_eq!(rfc3339(1_800_000_000), "2027-01-15T08:00:00Z");
        assert_eq!(parse_rfc3339("2027-01-15T08:00:00Z"), Some(1_800_000_000));
        assert_eq!(key_timestamp(1_800_000_000), "20270115T0800Z");
    }
}
