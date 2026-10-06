//! Browser GPX import: full planner points and the bounded corridor preview.

use serde::Serialize;

pub const MAX_ROUTE_POINTS: usize = 2048;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Point {
    pub lat: f64,
    pub lon: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ele: Option<f64>,
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub name: String,
    pub points: Vec<Point>,
    pub distance_km: f64,
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

pub fn read(text: &str, fallback: &str) -> Result<Import, String> {
    let entries = elements(text, &["trkpt", "rtept"]);
    let mut tracks = Vec::new();
    let mut routes = Vec::new();
    let mut malformed = 0;
    for entry in &entries {
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
        if entry.kind == "trkpt" {
            tracks.push(p);
        } else {
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
    let scoped = elements(text, &["trk", "rte"]).first().and_then(|e| first_text(e.body, "name"));
    let mut file = String::new();
    let mut at = 0;
    for w in wpts {
        file.push_str(&text[at..w.start]);
        at = w.end;
    }
    file.push_str(&text[at..]);
    let name = scoped.or_else(|| first_text(&file, "name")).unwrap_or_else(|| fallback.into());
    Ok(Import { name, points, waypoints })
}

pub fn parse(text: &str, fallback: &str) -> Result<Route, String> {
    let Import { name, points, .. } = read(text, fallback)?;
    let count = points.len().min(MAX_ROUTE_POINTS);
    let step = (points.len() - 1) as f64 / (count - 1) as f64;
    let points: Vec<_> = (0..count).map(|k| Point { ele: None, ..points[round(k as f64 * step) as usize] }).collect();
    let distance_km = points
        .windows(2)
        .map(|p| {
            let cos = (((p[0].lat + p[1].lat) / 2.0) * std::f64::consts::PI / 180e6).cos();
            let dlat = (p[1].lat - p[0].lat) * 111_320.0 / 1e6;
            let dlon = (p[1].lon - p[0].lon) * 111_320.0 * cos / 1e6;
            dlat.hypot(dlon)
        })
        .sum::<f64>()
        / 1000.0;
    Ok(Route { name, points, distance_km })
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

    #[test]
    fn corridor_keeps_endpoints_and_uniform_source_points() {
        let text: String =
            (0..10_000).map(|i| format!("<trkpt lat='{}' lon='8'/>", 47.0 + i as f64 / 10_000.0)).collect();
        let full = read(&text, "fallback").unwrap();
        let route = parse(&text, "fallback").unwrap();
        assert_eq!(full.points.len(), 10_000);
        assert_eq!(route.points.len(), MAX_ROUTE_POINTS);
        let samples: Vec<_> =
            [0, 1, 2, 3, 512, 1023, 1024, 2046, 2047].into_iter().map(|i| route.points[i].lat).collect();
        assert_eq!(
            samples,
            [
                47_000_000.0,
                47_000_500.0,
                47_001_000.0,
                47_001_500.0,
                47_250_100.0,
                47_499_700.0,
                47_500_200.0,
                47_999_400.0,
                47_999_900.0
            ]
        );
        assert!(route.points.iter().all(|point| point.ele.is_none()));
    }
}
