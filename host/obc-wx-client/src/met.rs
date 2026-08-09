//! MET Norway Locationforecast 2.0 — the one third party that ever receives a rider coordinate.
//!
//! The rules here are MET's terms and the WX1 decision record, not preferences:
//!
//! - an **identifying `User-Agent`** on every request (MET blocks anonymous traffic);
//! - coordinates rounded to **four decimals** (~11 m) — simultaneously the privacy contract and,
//!   because the URL is the cache key, the distance threshold below which a moving rider produces
//!   no request at all;
//! - **`Expires` is absolute**: inside it, no request is made, period;
//! - past it, revalidate with `If-Modified-Since` carrying MET's own `Last-Modified` string
//!   verbatim — never a reformatted date;
//! - a failure keeps the last good document, visibly timestamped; only a cold cache is an error.
//!
//! Missing optional fields (gust, probability) are *unavailable*, never inferred. A present but
//! wrong-typed or out-of-range field is malformed and rejects the whole document — a forecast the
//! parser had to guess at is not a forecast.

use std::collections::BTreeMap;

use obc_formats::obcw::{
    HourlyRecord, CONDITION_CLEAR, CONDITION_DRIZZLE, CONDITION_FOG, CONDITION_MOSTLY_CLEAR, CONDITION_OVERCAST,
    CONDITION_PARTLY_CLOUDY, CONDITION_RAIN, CONDITION_SHOWERS, CONDITION_SLEET, CONDITION_SNOW,
    CONDITION_THUNDERSTORM, CONDITION_UNAVAILABLE, HOURLY_COUNT, HOURLY_INTERVAL_SECONDS, PRECIP_UNAVAILABLE,
    PROBABILITY_UNAVAILABLE, TEMP_UNAVAILABLE, WIND_SPEED_UNAVAILABLE,
};
use serde::Deserialize;

use crate::http::{Http, Request, MET_CAP};

pub const ENDPOINT: &str = "https://api.met.no/weatherapi/locationforecast/2.0/complete";
pub const ATTRIBUTION_TEXT: &str = "Data from MET Norway";
pub const ATTRIBUTION_URL: &str = "https://docs.api.met.no/doc/License.html";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetError {
    Http(crate::http::HttpError),
    /// The document parsed as JSON but broke the contract.
    Malformed(String),
    RateLimited {
        retry_after: Option<String>,
    },
}

impl std::fmt::Display for MetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetError::Http(error) => write!(f, "{error}"),
            MetError::Malformed(why) => write!(f, "malformed MET response: {why}"),
            MetError::RateLimited { .. } => write!(f, "MET rate-limited this client"),
        }
    }
}

/// 24 hourly records plus the instant they start at, ready for the OBCW hourly section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hourly {
    pub valid_from: i64,
    pub records: [HourlyRecord; HOURLY_COUNT],
    /// This document came out of the cache after a failed refresh — the freshness line must say
    /// so rather than pretend it is current.
    pub from_cache: bool,
    pub retrieved_at: i64,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    hourly: Hourly,
    last_modified: Option<String>,
    expires: Option<i64>,
}

/// The adapter, holding the per-URL cache that the throttle rules are built on.
#[derive(Debug, Default)]
pub struct MetClient {
    endpoint: Option<String>,
    cache: BTreeMap<String, CacheEntry>,
    pub requests: u32,
}

impl MetClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Point the adapter at a stand-in origin (fixtures, a local capture). Production uses
    /// [`ENDPOINT`].
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// The request URL for a position — and, being the cache key, the throttle's identity.
    pub fn url(&self, lat_udeg: i32, lon_udeg: i32) -> String {
        let endpoint = self.endpoint.as_deref().unwrap_or(ENDPOINT);
        format!(
            "{endpoint}?lat={}&lon={}",
            four_decimals(f64::from(lat_udeg) / 1e6),
            four_decimals(f64::from(lon_udeg) / 1e6)
        )
    }

    /// Fetch (or reuse) the hourly forecast for a position.
    pub fn hourly<H: Http>(
        &mut self,
        http: &mut H,
        lat_udeg: i32,
        lon_udeg: i32,
        now: i64,
    ) -> Result<Hourly, MetError> {
        let url = self.url(lat_udeg, lon_udeg);
        // Rule 1: inside `Expires`, do not contact MET at all.
        if let Some(entry) = self.cache.get(&url) {
            if entry.expires.is_some_and(|expires| now < expires) {
                return Ok(entry.hourly.clone());
            }
        }
        let request = Request {
            url: url.clone(),
            range: None,
            if_none_match: None,
            if_modified_since: self.cache.get(&url).and_then(|entry| entry.last_modified.clone()),
        };
        self.requests += 1;
        let response = match http.perform(&request, MET_CAP) {
            Ok(response) => response,
            // Rule 5: a failure keeps the last good document, marked.
            Err(error) => {
                return match self.cache.get(&url) {
                    Some(entry) => Ok(Hourly { from_cache: true, ..entry.hourly.clone() }),
                    None => Err(match error {
                        crate::http::HttpError::Status { code: 429 | 503, retry_after } => {
                            MetError::RateLimited { retry_after }
                        }
                        other => MetError::Http(other),
                    }),
                }
            }
        };
        if matches!(response.status, 429 | 503) {
            return match self.cache.get(&url) {
                Some(entry) => Ok(Hourly { from_cache: true, ..entry.hourly.clone() }),
                None => Err(MetError::RateLimited { retry_after: response.retry_after }),
            };
        }
        let expires = response.expires.as_deref().and_then(parse_http_date);
        // Rule 2: a 304 keeps the body-less document and adopts the new validity.
        if response.is_not_modified() {
            if let Some(entry) = self.cache.get_mut(&url) {
                entry.expires = expires.or(entry.expires);
                return Ok(entry.hourly.clone());
            }
        }
        if !response.is_success() {
            return match self.cache.get(&url) {
                Some(entry) => Ok(Hourly { from_cache: true, ..entry.hourly.clone() }),
                None => Err(MetError::Http(crate::http::HttpError::Status {
                    code: response.status,
                    retry_after: response.retry_after,
                })),
            };
        }
        let hourly = decode(&response.body, now)?;
        self.cache.insert(url, CacheEntry { hourly: hourly.clone(), last_modified: response.last_modified, expires });
        Ok(hourly)
    }
}

fn four_decimals(value: f64) -> String {
    format!("{:.4}", (value * 10_000.0).round() / 10_000.0)
}

/// RFC 7231 IMSF-fixdate, the only shape MET emits.
fn parse_http_date(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(text).ok().map(|time| time.timestamp()).or_else(|| {
        chrono::NaiveDateTime::parse_from_str(text.trim(), "%a, %d %b %Y %H:%M:%S GMT")
            .ok()
            .map(|time| time.and_utc().timestamp())
    })
}

// ── the document ───────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Document {
    properties: Properties,
}

#[derive(Deserialize)]
struct Properties {
    meta: Meta,
    timeseries: Vec<Series>,
}

#[derive(Deserialize)]
struct Meta {
    units: Units,
}

#[derive(Deserialize)]
struct Units {
    air_temperature: Option<String>,
    wind_speed: Option<String>,
    wind_from_direction: Option<String>,
    precipitation_amount: Option<String>,
    wind_speed_of_gust: Option<String>,
    probability_of_precipitation: Option<String>,
}

#[derive(Deserialize)]
struct Series {
    time: String,
    data: SeriesData,
}

#[derive(Deserialize)]
struct SeriesData {
    instant: Instant,
    next_1_hours: Option<NextHour>,
}

#[derive(Deserialize)]
struct Instant {
    details: InstantDetails,
}

#[derive(Deserialize)]
struct InstantDetails {
    air_temperature: Option<f64>,
    wind_from_direction: Option<f64>,
    wind_speed: Option<f64>,
    wind_speed_of_gust: Option<f64>,
}

#[derive(Deserialize)]
struct NextHour {
    summary: Summary,
    details: NextHourDetails,
}

#[derive(Deserialize)]
struct Summary {
    symbol_code: Option<String>,
}

#[derive(Deserialize)]
struct NextHourDetails {
    precipitation_amount: Option<f64>,
    probability_of_precipitation: Option<f64>,
}

/// Decode 24 consecutive hours. Units are validated *before* any record: a document that
/// switched to Fahrenheit is malformed, not something to convert on a guess.
pub fn decode(bytes: &[u8], now: i64) -> Result<Hourly, MetError> {
    let document: Document = serde_json::from_slice(bytes).map_err(|error| MetError::Malformed(error.to_string()))?;
    let units = &document.properties.meta.units;
    let unit_ok = |actual: &Option<String>, expected: &str| actual.as_deref() == Some(expected);
    if !unit_ok(&units.air_temperature, "celsius")
        || !unit_ok(&units.wind_speed, "m/s")
        || !unit_ok(&units.wind_from_direction, "degrees")
        || !unit_ok(&units.precipitation_amount, "mm")
    {
        return Err(MetError::Malformed("unexpected units".into()));
    }
    if units.wind_speed_of_gust.as_deref().is_some_and(|unit| unit != "m/s")
        || units.probability_of_precipitation.as_deref().is_some_and(|unit| unit != "%")
    {
        return Err(MetError::Malformed("unexpected optional units".into()));
    }
    let series: Vec<&Series> = document.properties.timeseries.iter().take(HOURLY_COUNT).collect();
    if series.len() != HOURLY_COUNT {
        return Err(MetError::Malformed(format!("{} hours, need {HOURLY_COUNT}", series.len())));
    }
    let mut valid_from = 0i64;
    let mut records = [HourlyRecord {
        valid_time_offset_s: 0,
        temperature_deci_c: TEMP_UNAVAILABLE,
        precipitation_tenth_mm: PRECIP_UNAVAILABLE,
        precipitation_probability_pct: PROBABILITY_UNAVAILABLE,
        condition: CONDITION_UNAVAILABLE,
        wind_from_deg: 0,
        wind_speed_deci_ms: WIND_SPEED_UNAVAILABLE,
        wind_gust_deci_ms: WIND_SPEED_UNAVAILABLE,
        flags: 0,
    }; HOURLY_COUNT];
    for (index, entry) in series.iter().enumerate() {
        let time = crate::manifest::parse_rfc3339(&entry.time)
            .ok_or_else(|| MetError::Malformed(format!("hour {index} timestamp")))?;
        if index == 0 {
            valid_from = time;
        } else if time != valid_from + (index as i64) * i64::from(HOURLY_INTERVAL_SECONDS) {
            return Err(MetError::Malformed(format!("hour {index} is not on the hourly lattice")));
        }
        let next = entry
            .data
            .next_1_hours
            .as_ref()
            .ok_or_else(|| MetError::Malformed(format!("hour {index} has no next_1_hours")))?;
        let symbol = next
            .summary
            .symbol_code
            .as_deref()
            .ok_or_else(|| MetError::Malformed(format!("hour {index} has no symbol_code")))?;
        let condition =
            condition_for(symbol).ok_or_else(|| MetError::Malformed(format!("hour {index} symbol_code is empty")))?;
        let details = &entry.data.instant.details;
        let temperature = finite(details.air_temperature)
            .ok_or_else(|| MetError::Malformed(format!("hour {index} air_temperature")))?;
        let wind_from = finite(details.wind_from_direction)
            .ok_or_else(|| MetError::Malformed(format!("hour {index} wind_from_direction")))?;
        let wind_speed = finite(details.wind_speed)
            .filter(|speed| *speed >= 0.0)
            .ok_or_else(|| MetError::Malformed(format!("hour {index} wind_speed")))?;
        let precipitation = finite(next.details.precipitation_amount)
            .filter(|amount| *amount >= 0.0)
            .ok_or_else(|| MetError::Malformed(format!("hour {index} precipitation_amount")))?;
        // Optional, but validated when present: a present-and-wrong value is malformed, never
        // quietly downgraded to unavailable.
        let gust = match details.wind_speed_of_gust {
            None => None,
            Some(value) if value.is_finite() && value >= 0.0 => Some(value),
            Some(_) => return Err(MetError::Malformed(format!("hour {index} wind_speed_of_gust"))),
        };
        let probability = match next.details.probability_of_precipitation {
            None => None,
            Some(value) if value.is_finite() && (0.0..=100.0).contains(&value) => Some(value),
            Some(_) => return Err(MetError::Malformed(format!("hour {index} probability_of_precipitation"))),
        };
        records[index] = HourlyRecord {
            valid_time_offset_s: index as u32 * HOURLY_INTERVAL_SECONDS,
            temperature_deci_c: scaled_i16(temperature * 10.0, -1000, 700),
            precipitation_tenth_mm: match (precipitation * 10.0).round() {
                value if (0.0..=65_534.0).contains(&value) => value as u16,
                _ => PRECIP_UNAVAILABLE,
            },
            precipitation_probability_pct: probability.map_or(PROBABILITY_UNAVAILABLE, |value| value.round() as u8),
            condition,
            wind_from_deg: {
                let folded = wind_from.rem_euclid(360.0).round();
                if folded >= 360.0 {
                    0
                } else {
                    folded as u16
                }
            },
            wind_speed_deci_ms: scaled_u16(wind_speed * 10.0, 2_000),
            wind_gust_deci_ms: gust.map_or(WIND_SPEED_UNAVAILABLE, |value| scaled_u16(value * 10.0, 2_000)),
            flags: 0,
        };
    }
    Ok(Hourly { valid_from, records, from_cache: false, retrieved_at: now })
}

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite())
}

fn scaled_i16(value: f64, min: i16, max: i16) -> i16 {
    let rounded = value.round();
    if rounded >= f64::from(min) && rounded <= f64::from(max) {
        rounded as i16
    } else {
        TEMP_UNAVAILABLE
    }
}

fn scaled_u16(value: f64, max: u16) -> u16 {
    let rounded = value.round();
    if rounded >= 0.0 && rounded <= f64::from(max) {
        rounded as u16
    } else {
        WIND_SPEED_UNAVAILABLE
    }
}

/// The frozen WX1 `symbol_code` → canonical-condition table. Order is load-bearing:
/// `*andthunder*` beats every precipitation family, and a code the table does not know becomes
/// `unavailable` rather than a guess. An **empty** code is malformed (`None`) — that is a broken
/// document, not a truthful gap.
pub fn condition_for(symbol: &str) -> Option<u8> {
    let trimmed = symbol.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return None;
    }
    let base =
        ["_day", "_night", "_polartwilight"].iter().find_map(|suffix| trimmed.strip_suffix(suffix)).unwrap_or(&trimmed);
    if base.is_empty() {
        return None;
    }
    Some(if base.contains("andthunder") {
        CONDITION_THUNDERSTORM
    } else if base.contains("sleet") {
        CONDITION_SLEET
    } else if base.contains("snow") {
        CONDITION_SNOW
    } else if base.contains("showers") {
        CONDITION_SHOWERS
    } else if base == "lightrain" {
        CONDITION_DRIZZLE
    } else if base.contains("rain") {
        CONDITION_RAIN
    } else if base.starts_with("clearsky") {
        CONDITION_CLEAR
    } else if base.starts_with("fair") {
        CONDITION_MOSTLY_CLEAR
    } else if base.starts_with("partlycloudy") {
        CONDITION_PARTLY_CLOUDY
    } else if base == "cloudy" {
        CONDITION_OVERCAST
    } else if base == "fog" {
        CONDITION_FOG
    } else {
        CONDITION_UNAVAILABLE
    })
}
