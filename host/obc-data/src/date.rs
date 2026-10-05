//! Calendar dates as days since 1970-01-01, and UTC timestamps.

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
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| (elapsed.as_secs() / 86_400) as i64)
}

/// The current time in UTC as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now() -> String {
    timestamp(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs()))
}

fn timestamp(seconds: u64) -> String {
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let time = seconds % 86_400;
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", time / 3600, time % 3600 / 60, time % 60)
}

/// Howard Hinnant's `civil_from_days`, the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
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

#[cfg(test)]
mod tests {
    use super::{parse, timestamp};

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
    fn timestamps_are_utc_and_invert_dates() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        let leap_day = parse("2024-02-29").unwrap() as u64 * 86_400;
        assert_eq!(timestamp(leap_day + 3661), "2024-02-29T01:01:01Z");
    }
}
