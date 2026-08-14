//! Validation for catalog values that cross the wire boundary.
//!
//! This module owns canonical cell/region identifiers and UTC date/timestamp
//! parsing. Scanner-specific agreement checks remain with their scanners.

use crate::grid::{axis_cells, id_width, CellId, MAX_CELL_LOG2, MIN_CELL_LOG2};

/// Parse the canonical `<log2>/<i>/<j>` id (`OBCA_Spec.md` §1.3), **strictly**.
///
/// [`CellId::parse`] is deliberately lenient about the zero padding — a human types ids at
/// a CLI. A catalog cannot be: producers MUST widen rather than truncate, so `18/1204/52`
/// and `18/01204/1052` are *different strings for the same cell* and exactly the kind of
/// ambiguity a content-addressed store must not have. Every id this module reads out of a
/// document or off a path comes through here.
pub fn parse_strict_id(s: &str) -> Result<CellId, String> {
    let mut parts = s.split('/');
    let (Some(log2), Some(i), Some(j), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(format!("cell id `{s}` is not `<log2>/<i>/<j>`"));
    };
    if log2.is_empty() || log2.len() > 2 || !log2.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("cell id `{s}`: `{log2}` is not a 1–2 digit cell size"));
    }
    let log2: u32 = log2.parse().map_err(|_| format!("cell id `{s}`: bad cell size"))?;
    if !(MIN_CELL_LOG2..=MAX_CELL_LOG2).contains(&log2) {
        return Err(format!("cell id `{s}`: cell size 2^{log2} µdeg is outside 2^{MIN_CELL_LOG2}..=2^{MAX_CELL_LOG2}"));
    }
    let width = id_width(log2);
    let count = axis_cells(log2);
    let mut idx = [0i64; 2];
    for (slot, (text, axis)) in idx.iter_mut().zip([(i, "i"), (j, "j")]) {
        if text.len() != width || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("cell id `{s}`: `{axis}` must be {width} digits, zero-padded (got `{text}`)"));
        }
        let v: i64 = text.parse().map_err(|_| format!("cell id `{s}`: `{text}` is not a number"))?;
        if v >= count {
            return Err(format!("cell id `{s}`: `{axis}` = {v} is off the grid (0..{count})"));
        }
        *slot = v;
    }
    Ok(CellId { log2, i: idx[0], j: idx[1] })
}

pub(super) fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("`` is empty".to_string());
    }
    let valid = id
        .split('-')
        .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()));
    if !valid {
        return Err(format!("`{id}` must be lowercase kebab-case (a-z, 0-9, single hyphens)"));
    }
    Ok(())
}

/// A slash-separated region/extract id.
pub(super) fn validate_region_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("`` is empty".to_string());
    }
    for segment in id.split('/') {
        validate_id(segment).map_err(|e| format!("`{id}`: segment {e}"))?;
    }
    Ok(())
}

pub fn validate_timestamp(value: &str) -> Result<i64, String> {
    let bytes = value.as_bytes();
    if bytes.len() != 20 || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' || bytes[19] != b'Z' {
        return Err(format!("`{value}` is not an RFC 3339 UTC instant (YYYY-MM-DDTHH:MM:SSZ)"));
    }
    let days = civil_days(&value[..10]).map_err(|e| format!("`{value}`: {e}"))?;
    let (hours, minutes, seconds) = (number(&value[11..13])?, number(&value[14..16])?, number(&value[17..19])?);
    if hours > 23 || minutes > 59 || seconds > 59 {
        return Err(format!("`{value}` has an out-of-range time"));
    }
    Ok(days * 86_400 + i64::from(hours) * 3_600 + i64::from(minutes) * 60 + i64::from(seconds))
}

pub fn validate_date(value: &str) -> Result<(), String> {
    if value.len() != 10 {
        return Err(format!("`{value}` is not a YYYY-MM-DD date"));
    }
    civil_days(value).map(|_| ()).map_err(|e| format!("`{value}`: {e}"))
}

pub fn now_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64);
    format_timestamp(seconds)
}

pub fn format_timestamp(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let seconds = unix.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", seconds / 3_600, (seconds % 3_600) / 60, seconds % 60)
}

fn number(value: &str) -> Result<u32, String> {
    value.parse::<u32>().map_err(|_| format!("`{value}` is not a number"))
}

fn civil_days(date: &str) -> Result<i64, String> {
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err("not a YYYY-MM-DD date".to_string());
    }
    let (year, month, day) = (number(&date[..4])? as i64, number(&date[5..7])?, number(&date[8..10])?);
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return Err("is not a real calendar date".to_string());
    }
    Ok(days_from_civil(year, month, day))
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = i64::from((month + 9) % 12);
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}
