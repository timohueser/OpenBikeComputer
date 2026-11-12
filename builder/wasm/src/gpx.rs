//! Browser GPX import: the points and waypoints of a GPX file.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub lat: f64,
    pub lon: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ele: Option<f64>,
    /// Unix seconds, with the fraction of the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<f64>,
    /// The first point of the file, and of each `trk` and `trkseg`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub segment_start: bool,
}

#[derive(Debug, Serialize)]
pub struct Waypoint {
    pub lat: f64,
    pub lon: f64,
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Import {
    pub name: String,
    pub points: Vec<Point>,
    pub waypoints: Vec<Waypoint>,
    /// The `<type>` of the first track, such as `cycling`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

fn whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

fn number(text: &str) -> f64 {
    let text = text.trim_matches(whitespace);
    if text.is_empty() {
        return 0.0;
    }
    for (prefix, base) in [("0x", 16), ("0b", 2), ("0o", 8)] {
        if text.get(..2).is_some_and(|s| s.eq_ignore_ascii_case(prefix)) {
            return u64::from_str_radix(&text[2..], base).map_or(f64::NAN, |v| v as f64);
        }
    }
    // JavaScript Number does not accept Rust's abbreviated infinity spelling.
    if text.contains("inf") || text.contains("Inf") && !matches!(text, "Infinity" | "+Infinity" | "-Infinity") {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

fn word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

struct Element<'a> {
    kind: &'a str,
    attributes: &'a str,
    body: &'a str,
    start: usize,
    end: usize,
}

/// Tags remain case-sensitive and unnamespaced. Unclosed point elements are skipped.
fn elements<'a>(text: &'a str, kinds: &[&'a str]) -> Vec<Element<'a>> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(offset) = text[at..].find('<') {
        let start = at + offset;
        at = start + 1;
        let Some(kind) = kinds.iter().copied().find(|kind| {
            text[at..].starts_with(kind) && !text.as_bytes().get(at + kind.len()).is_some_and(|c| word(*c))
        }) else {
            continue;
        };
        let attributes = at + kind.len();
        let Some(close) = text[attributes..].find('>').map(|n| attributes + n) else { continue };
        let (body, end, attribute_end) = if text.as_bytes()[close - 1] == b'/' {
            ("", close + 1, close - 1)
        } else {
            let marker = format!("</{kind}");
            let mut seek = close + 1;
            let mut closing = None;
            while let Some(n) = text[seek..].find(&marker) {
                let body_end = seek + n;
                seek = body_end + marker.len();
                let tail = text[seek..].trim_start_matches(whitespace);
                if tail.starts_with('>') {
                    closing = Some((body_end, text.len() - tail.len() + 1));
                    break;
                }
            }
            let Some((body_end, end)) = closing else { continue };
            (&text[close + 1..body_end], end, close)
        };
        out.push(Element { kind, attributes: &text[attributes..attribute_end], body, start, end });
        at = end;
    }
    out
}

fn attribute(text: &str, key: &str) -> Option<f64> {
    for (at, _) in text.match_indices(key) {
        if at > 0 && word(text.as_bytes()[at - 1]) {
            continue;
        }
        let rest = text[at + key.len()..].trim_start_matches(whitespace);
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start_matches(whitespace);
        if !rest.starts_with(['\'', '"']) {
            continue;
        }
        let rest = &rest[1..];
        let Some(end) = rest.find(['\'', '"']) else { continue };
        if end > 0 {
            return Some(number(&rest[..end]));
        }
    }
    None
}

fn point(attributes: &str) -> Option<Point> {
    let lat = attribute(attributes, "lat")?;
    let lon = attribute(attributes, "lon")?;
    (lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0).then(|| Point {
        // JavaScript Math.round breaks negative ties towards positive infinity.
        lat: round(lat * 1e6),
        lon: round(lon * 1e6),
        ele: None,
        time: None,
        segment_start: false,
    })
}

fn round(value: f64) -> f64 {
    if (-0.5..0.0).contains(&value) {
        return -0.0;
    }
    let floor = value.floor();
    if value - floor < 0.5 {
        floor
    } else {
        floor + 1.0
    }
}

/// `YYYY-MM-DDTHH:MM:SS`, an optional fraction, then `Z`, `±HH:MM` or no zone, which is UTC.
fn time(text: &str) -> Option<f64> {
    let b = text.as_bytes();
    let digits = |at: usize, len: usize| -> Option<i64> {
        let part = b.get(at..at + len)?;
        part.iter().all(u8::is_ascii_digit).then(|| part.iter().fold(0, |n, c| n * 10 + i64::from(c - b'0')))
    };
    let [year, month, day, hour, minute, second] =
        [(0, 4), (5, 2), (8, 2), (11, 2), (14, 2), (17, 2)].map(|(at, len)| digits(at, len));
    let (year, month, day, hour, minute, second) = (year?, month?, day?, hour?, minute?, second?);
    if b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &b[19..];
    let mut fraction = 0.0;
    if let Some(after) = rest.strip_prefix(b".") {
        let end = after.iter().position(|c| !c.is_ascii_digit()).unwrap_or(after.len());
        fraction = after[..end].iter().rev().fold(0.0, |f, c| (f + f64::from(c - b'0')) / 10.0);
        rest = &after[end..];
    }
    let offset = match rest {
        b"" | b"Z" | b"z" => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] | [sign @ (b'+' | b'-'), h1, h2, m1, m2] => {
            let zone = [h1, h2, m1, m2];
            if !zone.iter().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let [h1, h2, m1, m2] = zone.map(|c| i64::from(c - b'0'));
            let seconds = (h1 * 10 + h2) * 3600 + (m1 * 10 + m2) * 60;
            if *sign == b'-' {
                -seconds
            } else {
                seconds
            }
        }
        _ => return None,
    };
    // Days since 1970-01-01 in the Gregorian calendar (`days_from_civil` of Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some((days * 86_400 + hour * 3600 + minute * 60 + second - offset) as f64 + fraction)
}

fn first_text(text: &str, tag: &str) -> Option<String> {
    let begin = text.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = text[begin..].find(&format!("</{tag}>"))? + begin;
    let value = text[begin..end].trim_matches(whitespace);
    if value.is_empty() {
        return None;
    }
    Some(
        value
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

fn scoped_name(text: &str) -> Option<String> {
    for (at, _) in text.match_indices('<') {
        let rest = &text[at + 1..];
        if !["trk", "rte"]
            .iter()
            .any(|tag| rest.starts_with(tag) && !rest.as_bytes().get(tag.len()).is_some_and(|c| word(*c)))
        {
            continue;
        }
        // A self-closing opener can precede the named track in this text scope.
        let body = &rest[rest.find('>')? + 1..];
        let end = [body.find("</trk>"), body.find("</rte>")].into_iter().flatten().min()?;
        return first_text(&body[..end], "name");
    }
    None
}

pub fn read(text: &str, fallback: &str) -> Result<Import, String> {
    let entries = elements(text, &["trkpt", "rtept"]);
    let trks = elements(text, &["trk"]);
    // The openers of the tracks and their segments, in the order of the file.
    let mut openers: Vec<usize> = trks.iter().chain(&elements(text, &["trkseg"])).map(|e| e.start).collect();
    openers.sort_unstable();
    let mut openers = openers.into_iter().peekable();
    let mut tracks: Vec<Point> = Vec::new();
    let mut routes: Vec<Point> = Vec::new();
    let mut malformed = 0;
    let mut segment_start = true;
    for entry in &entries {
        if entry.kind == "trkpt" {
            while openers.next_if(|&at| at < entry.start).is_some() {
                segment_start = true;
            }
        }
        let Some(mut p) = point(entry.attributes) else {
            malformed += 1;
            continue;
        };
        let mut rest = entry.body;
        while let Some(at) = rest.find("<ele>") {
            rest = &rest[at + 5..];
            let Some(end) = rest.find("</ele>") else { break };
            let value = &rest[..end];
            if !value.contains('<') && !value.trim_matches(whitespace).is_empty() {
                let value = number(value);
                p.ele = value.is_finite().then_some(value);
                break;
            }
        }
        p.time = first_text(entry.body, "time").and_then(|t| time(&t));
        if entry.kind == "trkpt" {
            p.segment_start = std::mem::take(&mut segment_start);
            tracks.push(p);
        } else {
            p.segment_start = routes.is_empty();
            routes.push(p);
        }
    }
    let points = if tracks.is_empty() { routes } else { tracks };
    if points.len() < 2 {
        return Err(if entries.is_empty() {
            "no track or route points found — is this a GPX file?".into()
        } else if malformed > 0 {
            format!("no usable points — {malformed} of {} carried malformed coordinates", entries.len())
        } else {
            "the file has fewer than two points, which is not a route".into()
        });
    }
    let wpts = elements(text, &["wpt"]);
    let waypoints = wpts
        .iter()
        .filter_map(|w| {
            point(w.attributes).map(|p| Waypoint {
                lat: p.lat,
                lon: p.lon,
                name: first_text(w.body, "name"),
                note: first_text(w.body, "desc").or_else(|| first_text(w.body, "cmt")),
            })
        })
        .collect();
    let scoped = scoped_name(text);
    let mut file = String::new();
    let mut at = 0;
    for w in wpts {
        file.push_str(&text[at..w.start]);
        at = w.end;
    }
    file.push_str(&text[at..]);
    let name = scoped.or_else(|| first_text(&file, "name")).unwrap_or_else(|| fallback.into());
    // Only the head of the track: a point can have a `<type>` too.
    let kind = trks.first().and_then(|trk| {
        let head = ["<trkseg", "<trkpt"].iter().filter_map(|tag| trk.body.find(tag)).min().unwrap_or(trk.body.len());
        first_text(&trk.body[..head], "type")
    });
    Ok(Import { name, points, waypoints, kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_keeps_elevation_notes_and_negative_rounding() {
        let text = "<gpx><metadata><name>File</name></metadata><wpt lat='47' lon='8'><name>Water</name><cmt>Open &amp; free</cmt></wpt><trk><name>Track</name><trkpt lat='-0.0000005' lon='-0.0000015'><ele>1.23456789123</ele></trkpt><trkpt lat='47.1' lon='8'/></trk></gpx>";
        let import = read(text, "fallback").unwrap();
        assert_eq!(import.name, "Track");
        assert!(import.points[0].lat.is_sign_negative());
        assert_eq!(import.points[0].lon, -1.0);
        assert_eq!(import.points[0].ele, Some(1.23456789123));
        assert_eq!(import.waypoints[0].name.as_deref(), Some("Water"));
        assert_eq!(import.waypoints[0].note.as_deref(), Some("Open & free"));
    }
}
