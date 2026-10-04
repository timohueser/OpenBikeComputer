use super::{
    geometry::{coordinates, distance2},
    input::Feature,
};
use geo::{LineString, Point};
use osmpbfreader::{OsmId, Tags};

pub fn point(line: &LineString, fraction: f64) -> Point {
    let length: f64 = line.lines().map(|s| distance2(Point(s.start), Point(s.end)).sqrt()).sum();
    let mut remaining = length * fraction;
    for s in line.lines() {
        let length = distance2(Point(s.start), Point(s.end)).sqrt();
        if length > 0. && remaining <= length {
            return Point::new(
                s.start.x + (s.end.x - s.start.x) * remaining / length,
                s.start.y + (s.end.y - s.start.y) * remaining / length,
            );
        }
        remaining -= length;
    }
    Point(*line.0.last().unwrap())
}

fn section(line: &LineString, from: f64, to: f64) -> LineString {
    let total: f64 = line.lines().map(|s| distance2(Point(s.start), Point(s.end)).sqrt()).sum();
    let mut result = vec![point(line, from).0];
    let mut covered = 0.;
    for s in line.lines() {
        covered += distance2(Point(s.start), Point(s.end)).sqrt();
        if covered > from * total && covered < to * total {
            result.push(s.end);
        }
    }
    result.push(point(line, to).0);
    for c in &mut result {
        c.x = (c.x * 1e7).round() / 1e7;
        c.y = (c.y * 1e7).round() / 1e7;
    }
    LineString(result)
}

pub fn ranges(source: OsmId, line: &LineString, tags: &Tags, endpoints: &[(usize, &Tags)]) -> Vec<Feature> {
    let kind = tags.get("addr:interpolation").map(|s| s.as_str()).unwrap_or("");
    let step = match kind {
        "odd" | "even" => 2,
        "all" => 1,
        _ => kind.parse::<i64>().unwrap_or(0),
    };
    if !(1..10).contains(&step) {
        return vec![];
    }
    let numbered: Vec<_> = endpoints
        .iter()
        .filter_map(|(pos, tags)| {
            let house = tags.get("addr:housenumber")?;
            if house.len() > 6 || !house.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            Some((*pos, house.parse::<i64>().ok()?, *tags))
        })
        .collect();
    let mut result = Vec::new();
    for pair in numbered.windows(2) {
        let ((a, first, at), (b, last, bt)) = (pair[0], pair[1]);
        if a >= b || (last - first).abs() <= step {
            continue;
        }
        let mut segment = coordinates(line.0[a..=b].iter().map(|p| [p.x, p.y]));
        let (first, last) = if first > last {
            segment.0.reverse();
            (last, first)
        } else {
            (first, last)
        };
        let mut start = first + if kind == "all" || kind.parse::<i64>().is_ok() { step } else { 1 };
        if kind == "odd" && start % 2 == 0 || kind == "even" && start % 2 != 0 {
            start += 1;
        }
        let end = start + ((last - 1 - start) / step) * step;
        if end < start || end - start >= 600 {
            continue;
        }
        let mut inherited = tags.clone();
        for (_, value) in [("addr:postcode", at.get("addr:postcode")), ("addr:postcode", bt.get("addr:postcode"))] {
            if let Some(code) = value {
                inherited.insert("addr:postcode".into(), code.clone());
                break;
            }
        }
        for (key, value) in at.iter() {
            if key.starts_with("addr:") && !inherited.contains_key(key) {
                inherited.insert(key.clone(), value.clone());
            }
        }
        inherited.insert(
            "addr:housenumber".into(),
            (start..=end).step_by(step as usize).map(|n| n.to_string()).collect::<Vec<_>>().join(";").into(),
        );
        inherited.insert("_interpolation_range".into(), format!("{start}:{end}").into());
        let segment = section(
            &segment,
            (start - first) as f64 / (last - first) as f64,
            (end - first) as f64 / (last - first) as f64,
        );
        result.push(Feature {
            source,
            tags: inherited,
            geometry: if start == end { Point(segment.0[0]).into() } else { segment.into() },
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_use_inner_numbers_and_follow_bends_in_both_directions() {
        let tags = Tags::from_iter([("addr:interpolation".into(), "even".into())]);
        let a = Tags::from_iter([("addr:housenumber".into(), "10".into())]);
        let b = Tags::from_iter([("addr:housenumber".into(), "2".into())]);
        let line = coordinates([[0., 0.], [1., 0.], [1., 1.]]);
        let ranges = ranges(OsmId::Way(osmpbfreader::WayId(1)), &line, &tags, &[(0, &a), (2, &b)]);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].tag("addr:housenumber"), "4;6;8");
        let geo::Geometry::LineString(line) = &ranges[0].geometry else { panic!("line") };
        assert_eq!(point(line, 0.5), Point::new(1., 0.));
        assert_eq!(point(line, 0.), Point::new(1., 0.5));
        assert_eq!(point(line, 1.), Point::new(0.5, 0.));
    }
}
