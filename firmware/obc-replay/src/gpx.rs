//! GPX track parsing — a **simulator-only** concern.
//!
//! The real device has a GPS chip; it never parses a GPX file. Replaying a
//! recorded track is purely a host convenience, so this lives in this host crate (it
//! needs `std`) and produces nothing the shared crates know about — the
//! [`GpxPlayer`](crate::gpx_player::GpxPlayer) turns a [`Track`] into the same
//! [`Fix`](obc_ports::Fix)es a GPS driver would emit.
//!
//! The parser is a small hand-rolled scan rather than a full XML stack: GPX track
//! points are a regular `<trkpt lat=".." lon="..">` with an optional `<ele>` and
//! `<time>`, which is all we need. Timestamps are ISO-8601 UTC (`...Z`); we keep
//! only the *relative* time within the track, so a tiny civil-date → epoch
//! conversion is enough (no `chrono`).

use std::path::Path;

/// One recorded track point: position in microdegrees (matching the rest of the
/// pipeline), optional `<ele>` in meters (fed to the simulated barometer), and `t`,
/// seconds elapsed since the first point of the track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPoint {
    pub lat: i32,
    pub lon: i32,
    pub ele: Option<f32>,
    pub t: f64,
}

/// A parsed GPX track: its points in time order, with `t` rebased so the first
/// point is `0.0`.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub points: Vec<TrackPoint>,
}

impl Track {
    /// Read and parse a GPX file from disk.
    pub fn load(path: &Path) -> Result<Track, String> {
        let xml = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Track::parse(&xml)
    }

    /// Parse GPX XML text. Pulls every `<trkpt>` (across all `<trkseg>`/`<trk>`),
    /// converts degrees → microdegrees, and rebases timestamps to seconds from the
    /// first point. If a track has no `<time>` stamps at all, falls back to a
    /// uniform one-second spacing so it can still be replayed.
    pub fn parse(xml: &str) -> Result<Track, String> {
        let mut raw: Vec<(i32, i32, Option<f32>, Option<f64>)> = Vec::new();
        let mut rest = xml;
        while let Some(start) = rest.find("<trkpt") {
            // Opening tag spans `<trkpt ... >`; attributes (lat/lon) live inside it.
            let after = &rest[start..];
            let tag_end = after.find('>').ok_or("unterminated <trkpt> tag")?;
            let open_tag = &after[..tag_end];
            let lat = attr_f64(open_tag, "lat").ok_or("trkpt missing lat")?;
            let lon = attr_f64(open_tag, "lon").ok_or("trkpt missing lon")?;

            // A self-closing `<trkpt .../>` has no children; otherwise read up to the
            // matching `</trkpt>` and look for <ele> (→ the barometer) and <time> inside.
            let self_closing = open_tag.trim_end().ends_with('/');
            let (ele, time) = if self_closing {
                (None, None)
            } else {
                let body_start = start + tag_end + 1;
                let body = &rest[body_start..];
                let close = body.find("</trkpt>").ok_or("unterminated <trkpt> element")?;
                let inner = &body[..close];
                (parse_ele_tag(inner), parse_time_tag(inner))
            };

            raw.push((deg_to_microdeg(lat), deg_to_microdeg(lon), ele, time));
            // Advance past this point. (`<trkpt` again can only appear after `>`.)
            rest = &after[tag_end + 1..];
        }

        if raw.is_empty() {
            return Err("no <trkpt> elements found in GPX".into());
        }

        // Rebase time. If every point is timestamped, use real elapsed seconds;
        // otherwise fall back to uniform 1 s/point so replay still works.
        let all_timed = raw.iter().all(|(_, _, _, t)| t.is_some());
        let points = if all_timed {
            let t0 = raw[0].3.unwrap();
            raw.iter().map(|&(lat, lon, ele, t)| TrackPoint { lat, lon, ele, t: t.unwrap() - t0 }).collect()
        } else {
            raw.iter().enumerate().map(|(i, &(lat, lon, ele, _))| TrackPoint { lat, lon, ele, t: i as f64 }).collect()
        };

        Ok(Track { points })
    }

    /// Total track duration in seconds (`t` of the last point), or `0.0` if there
    /// are fewer than two points.
    pub fn duration(&self) -> f64 {
        self.points.last().map_or(0.0, |p| p.t)
    }
}

/// Degrees (as parsed from GPX) → microdegrees, rounded.
fn deg_to_microdeg(deg: f64) -> i32 {
    (deg * 1e6).round() as i32
}

/// Read a quoted attribute value (`name="..."` or `name='...'`) from an opening
/// tag, parsing it as `f64`. Tolerates surrounding whitespace and either quote.
fn attr_f64(tag: &str, name: &str) -> Option<f64> {
    // Find `name` as a whole attribute token (preceded by whitespace or the tag
    // name) so `lat` can't match inside another attribute.
    let mut search = tag;
    loop {
        let idx = search.find(name)?;
        let before_ok = idx == 0 || search.as_bytes()[idx - 1].is_ascii_whitespace();
        let after = &search[idx + name.len()..];
        let after_trim = after.trim_start();
        if before_ok && after_trim.starts_with('=') {
            let after_eq = after_trim[1..].trim_start();
            let quote = after_eq.chars().next()?;
            if quote == '"' || quote == '\'' {
                let val = &after_eq[1..];
                let end = val.find(quote)?;
                return val[..end].trim().parse().ok();
            }
        }
        // Keep scanning past this (non-)match.
        search = &search[idx + name.len()..];
    }
}

/// Pull the `<ele>...</ele>` text (meters) from a trkpt body. Returns `None` if there's
/// no elevation element — the barometer then has no reading for that point.
fn parse_ele_tag(body: &str) -> Option<f32> {
    let start = body.find("<ele>")? + "<ele>".len();
    let end = body[start..].find("</ele>")? + start;
    body[start..end].trim().parse().ok()
}

/// Pull the `<time>...</time>` text from a trkpt body and parse it to epoch
/// seconds. Returns `None` if there's no time element.
fn parse_time_tag(body: &str) -> Option<f64> {
    let start = body.find("<time>")? + "<time>".len();
    let end = body[start..].find("</time>")? + start;
    parse_iso8601_utc(body[start..end].trim())
}

/// Parse an ISO-8601 UTC timestamp (`YYYY-MM-DDThh:mm:ss[.fff][Z]`) to seconds
/// since the Unix epoch. Only the fields we get from GPX are handled; any zone
/// suffix is treated as UTC (Komoot et al. emit `Z`), which is fine because we
/// only ever use *differences* within a single track.
fn parse_iso8601_utc(s: &str) -> Option<f64> {
    let (date, time) = s.split_once(['T', 't'])?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;

    // Strip a trailing zone marker; we assume UTC.
    let time = time.trim_end_matches(['Z', 'z']);
    let time = time.split(['+', '-']).next()?; // drop any explicit offset
    let mut t = time.split(':');
    let hour: f64 = t.next()?.parse().ok()?;
    let minute: f64 = t.next()?.parse().ok()?;
    let second: f64 = t.next()?.parse().ok()?;

    let days = days_from_civil(year, month, day);
    Some(days as f64 * 86_400.0 + hour * 3600.0 + minute * 60.0 + second)
}

/// Days from the Unix epoch (1970-01-01) for a proleptic-Gregorian civil date.
/// Howard Hinnant's `days_from_civil` — exact for all dates we'll see.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version='1.0'?>
<gpx><trk><trkseg>
  <trkpt lat="48.122905" lon="7.814438"><ele>200.8</ele><time>2026-05-28T05:36:45.000Z</time></trkpt>
  <trkpt lat="48.123147" lon="7.814564"><ele>200.8</ele><time>2026-05-28T05:37:05.000Z</time></trkpt>
  <trkpt lat="48.123220" lon="7.814625"><ele>200.8</ele><time>2026-05-28T05:37:07.000Z</time></trkpt>
</trkseg></trk></gpx>"#;

    #[test]
    fn parses_points_and_relative_time() {
        let t = Track::parse(SAMPLE).unwrap();
        assert_eq!(t.points.len(), 3);
        assert_eq!(t.points[0].lat, 48_122_905);
        assert_eq!(t.points[0].lon, 7_814_438);
        assert_eq!(t.points[0].ele, Some(200.8)); // <ele> feeds the barometer
                                                  // First point rebased to zero, others relative.
        assert_eq!(t.points[0].t, 0.0);
        assert_eq!(t.points[1].t, 20.0);
        assert_eq!(t.points[2].t, 22.0);
        assert_eq!(t.duration(), 22.0);
    }

    #[test]
    fn attr_order_independent() {
        // lon before lat, single quotes, extra whitespace.
        let xml = "<gpx><trkpt   lon='7.5'   lat='48.5' /></gpx>";
        let t = Track::parse(xml).unwrap();
        assert_eq!(t.points.len(), 1);
        assert_eq!(t.points[0].lat, 48_500_000);
        assert_eq!(t.points[0].lon, 7_500_000);
    }

    #[test]
    fn untimed_track_falls_back_to_uniform_spacing() {
        let xml = r#"<gpx><trkpt lat="1.0" lon="2.0"/><trkpt lat="1.1" lon="2.1"/></gpx>"#;
        let t = Track::parse(xml).unwrap();
        assert_eq!(t.points[0].t, 0.0);
        assert_eq!(t.points[1].t, 1.0);
    }

    #[test]
    fn empty_is_an_error() {
        assert!(Track::parse("<gpx></gpx>").is_err());
    }

    /// `Track::parse` is strict: a `<trkpt>` missing `lat`/`lon` errors with the exact message
    /// (a UI surfaces it) rather than dropping the point. Divergence: `obc-route`'s `GpxScanner`
    /// *skips* the same point (see its `scanner_skips_a_missing_coordinate`).
    #[test]
    fn missing_coordinate_is_an_error() {
        // Missing lon → Err, and specifically the "missing lon" message.
        let err = Track::parse(r#"<gpx><trkpt lat="48.0"/></gpx>"#).unwrap_err();
        assert_eq!(err, "trkpt missing lon", "a lon-less point errors (obc-route skips it instead)");

        // Missing lat → the matching "missing lat" error.
        let err = Track::parse(r#"<gpx><trkpt lon="7.8"/></gpx>"#).unwrap_err();
        assert_eq!(err, "trkpt missing lat");

        // One bad point aborts the whole parse, even with a valid point alongside it.
        assert!(Track::parse(r#"<gpx><trkpt lat="48.0" lon="7.8"/><trkpt lat="48.1"/></gpx>"#).is_err());
    }

    /// A truncated opening tag (no `>`) and an opening tag with no matching `</trkpt>` each error
    /// with their distinct message, so a half-written GPX fails loudly rather than replaying a
    /// partial track.
    #[test]
    fn unterminated_tag_is_an_error() {
        // No '>' closing the opening tag at end of input.
        let err = Track::parse(r#"<gpx><trkpt lat="48.0" lon="7.8""#).unwrap_err();
        assert_eq!(err, "unterminated <trkpt> tag");

        // Opening tag closes, but there's no </trkpt> for the (non-self-closing) element.
        let err = Track::parse(r#"<gpx><trkpt lat="48.0" lon="7.8"><ele>5</ele>"#).unwrap_err();
        assert_eq!(err, "unterminated <trkpt> element");
    }

    #[test]
    fn iso8601_epoch_matches_known_value() {
        // 2026-05-28T05:36:45Z == 1779946605 s since epoch (Python datetime).
        let s = parse_iso8601_utc("2026-05-28T05:36:45.000Z").unwrap();
        assert_eq!(s as i64, 1_779_946_605);
    }

    #[test]
    fn civil_epoch_day_zero() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 1, 1), 10_957);
    }
}
