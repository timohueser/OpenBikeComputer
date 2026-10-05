//! Calendar dates as days since 1970-01-01, which is all a pin's age needs.

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
    use super::parse;

    #[test]
    fn dates_count_days_and_reject_impossible_days() {
        assert_eq!(parse("1970-01-01"), Some(0));
        assert_eq!(parse("2024-03-01").unwrap() - parse("2024-02-28").unwrap(), 2);
        assert_eq!(parse("2023-03-01").unwrap() - parse("2023-02-28").unwrap(), 1);
        for bad in ["2023-02-29", "2024-13-01", "2024-1-01", "v1.11", "+024-01-01"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}
