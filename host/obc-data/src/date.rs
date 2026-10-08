//! Calendar dates as days since 1970-01-01, and UTC times: the age of a version, a retrieval time, an HTTP date.

use std::time::{SystemTime, UNIX_EPOCH};

/// A `YYYY-MM-DD` date as days since 1970-01-01, or `None` when it is not a real date.
pub fn parse(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let shape = bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, b)| if i == 4 || i == 7 { *b == b'-' } else { b.is_ascii_digit() });
    if !shape {
        return None;
    }
    let year: i64 = text[0..4].parse().ok()?;
    let month: i64 = text[5..7].parse().ok()?;
    let day: i64 = text[8..10].parse().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (1..=days_in_month).contains(&day).then(|| days_from_civil(year, month, day))
}

/// Today in UTC, as days since 1970-01-01.
pub fn today() -> i64 {
    (now() / 86_400) as i64
}

/// Seconds since 1970-01-01 UTC.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

/// Days since 1970-01-01 as `YYYY-MM-DD`.
pub fn format(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Seconds since 1970-01-01 as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn timestamp(seconds: u64) -> String {
    let time = seconds % 86_400;
    format!("{}T{:02}:{:02}:{:02}Z", format((seconds / 86_400) as i64), time / 3600, time / 60 % 60, time % 60)
}

/// An RFC 3339 time, `YYYY-MM-DDTHH:MM:SS` with an optional fraction and then `Z`, `+HH:MM` or
/// `-HH:MM`, as seconds since 1970-01-01 UTC; the fraction is left out. R2 writes `Z`; rclone's
/// local backend writes the offset of the local zone.
pub fn seconds(text: &str) -> Option<u64> {
    let two = |part: Option<&str>| part.filter(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit()))?.parse().ok();
    let day = parse(text.get(..10)?)?;
    if (text.get(10..11)?, text.get(13..14)?, text.get(16..17)?) != ("T", ":", ":") {
        return None;
    }
    let (hours, minutes, seconds): (i64, i64, i64) =
        (two(text.get(11..13))?, two(text.get(14..16))?, two(text.get(17..19))?);
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    let zone = text.get(19..)?.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match (zone, zone.get(..1), zone.get(3..4)) {
        ("Z", _, _) => 0,
        (_, Some(sign @ ("+" | "-")), Some(":")) if zone.len() == 6 => {
            let offset: i64 = two(zone.get(1..3))? * 3600 + two(zone.get(4..6))? * 60;
            if sign == "+" {
                offset
            } else {
                -offset
            }
        }
        _ => return None,
    };
    u64::try_from(day * 86_400 + hours * 3600 + minutes * 60 + seconds - offset).ok()
}

/// The day of an HTTP date such as `Sun, 06 Nov 1994 08:49:37 GMT`, as `YYYY-MM-DD`.
pub fn from_http(value: &str) -> Option<String> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut words = value.split_whitespace().skip(1);
    let (day, month, year) = (words.next()?, words.next()?, words.next()?);
    let month = MONTHS.iter().position(|name| *name == month)? + 1;
    let text = format!("{year}-{month:02}-{day:0>2}");
    parse(&text).map(|_| text)
}

/// Howard Hinnant's `days_from_civil`: a year starts in March, so the leap day is the last day.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of `days_from_civil`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::{format, from_http, parse, seconds, timestamp};

    #[test]
    fn dates_count_days_and_reject_impossible_days() {
        assert_eq!(parse("1970-01-01"), Some(0));
        assert_eq!(parse("2024-03-01").unwrap() - parse("2024-02-28").unwrap(), 2);
        assert_eq!(parse("2023-03-01").unwrap() - parse("2023-02-28").unwrap(), 1);
        for bad in ["2023-02-29", "2024-13-01", "2024-1-01", "v1.11", "+024-01-01"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn dates_format_back_and_read_from_http_headers() {
        for text in ["1970-01-01", "2024-02-29", "2026-10-05", "1999-12-31"] {
            assert_eq!(format(parse(text).unwrap()), text);
        }
        assert_eq!(timestamp(86_400 + 3_723), "1970-01-02T01:02:03Z");
        assert_eq!(seconds("1970-01-02T01:02:03.278071679Z"), Some(86_400 + 3_723), "an R2 upload time");
        assert_eq!(seconds("1970-01-02T03:02:03.5+02:00"), Some(86_400 + 3_723), "a local backend time");
        assert_eq!(seconds("1970-01-01T20:32:03-04:30"), Some(86_400 + 3_723));
        for bad in ["1970-01-02T25:02:03Z", "1970-01-02T01:02:03", "1970-01-02T01:02:03+2", "1970-01-02 01:02:03Z"] {
            assert_eq!(seconds(bad), None, "{bad}");
        }
        assert_eq!(from_http("Mon, 05 Oct 2026 03:43:59 GMT").as_deref(), Some("2026-10-05"));
        assert_eq!(from_http("Mon, 32 Oct 2026 03:43:59 GMT"), None);
        assert_eq!(from_http("yesterday"), None);
    }
}
