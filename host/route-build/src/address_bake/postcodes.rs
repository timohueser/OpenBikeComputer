use super::{
    geometry::{center, distance2, envelope, Entry},
    input::Input,
};
use geo::{CoordsIter, Geometry, Intersects, Point};
use rstar::{Envelope, RTree, AABB};
use std::collections::{BTreeMap, BTreeSet};

struct Postcode {
    country: String,
    code: String,
    geometry: Option<Geometry>,
    center: Point,
    envelope: AABB<[f64; 2]>,
}

pub struct Postcodes {
    entries: Vec<Postcode>,
    tree: RTree<Entry>,
}

pub fn normalized(code: &str, country: &str) -> Option<String> {
    let code = code.trim();
    let code = if country == "us" && code.len() == 10 && code.as_bytes()[5] == b'-' { &code[..5] } else { code };
    let length = if country == "ch" { 4 } else { 5 };
    (code.len() == length && code.bytes().all(|c| c.is_ascii_digit())).then(|| code.to_string())
}

impl Postcodes {
    pub fn new(input: &Input, countries: &[&str]) -> Self {
        let mut entries = Vec::new();
        let mut covered = BTreeSet::new();
        for (i, f) in input.features.iter().enumerate() {
            let country = countries[i];
            if f.tag("boundary") != "postal_code"
                || !matches!(f.geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_))
            {
                continue;
            }
            let code = if f.tag("postal_code").is_empty() { f.tag("addr:postcode") } else { f.tag("postal_code") };
            if let Some(code) = normalized(code, country) {
                covered.insert((country.to_string(), code.clone()));
                entries.push(Postcode {
                    country: country.to_string(),
                    code,
                    center: center(&f.geometry),
                    envelope: envelope(&f.geometry),
                    geometry: Some(f.geometry.clone()),
                });
            }
        }
        let mut sums: BTreeMap<(String, String), (f64, f64, usize)> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let areas = RTree::bulk_load(
            entries.iter().enumerate().map(|(index, pc)| Entry { index, envelope: pc.envelope }).collect(),
        );
        for (i, f) in input.features.iter().enumerate() {
            let country = countries[i];
            let code = if f.tag("place") == "postcode" { f.tag("postal_code") } else { f.tag("addr:postcode") };
            let Some(code) = normalized(code, country) else {
                continue;
            };
            if covered.contains(&(country.to_string(), code.clone())) {
                continue;
            }
            let p = center(&f.geometry);
            if areas
                .locate_in_envelope_intersecting(&super::geometry::expanded(p, 0.))
                .any(|e| entries[e.index].geometry.as_ref().is_some_and(|g| g.intersects(&p)))
            {
                continue;
            }
            let point = ((p.x() * 1e7).round() as i64, (p.y() * 1e7).round() as i64);
            if !seen.insert((country.to_string(), code.clone(), point)) {
                continue;
            }
            let sum = sums.entry((country.to_string(), code)).or_default();
            sum.0 += p.x();
            sum.1 += p.y();
            sum.2 += 1;
        }
        for ((country, code), (x, y, n)) in sums {
            let metres = if country == "ch" { 3000. } else { 5000. };
            let p = Point::new(x / n as f64, y / n as f64);
            let dy = metres / 111320.;
            let dx = dy / p.y().to_radians().cos().abs().max(0.01);
            entries.push(Postcode {
                country,
                code,
                geometry: None,
                center: p,
                envelope: AABB::from_corners([p.x() - dx, p.y() - dy], [p.x() + dx, p.y() + dy]),
            });
        }
        let tree = RTree::bulk_load(
            entries.iter().enumerate().map(|(index, p)| Entry { index, envelope: p.envelope }).collect(),
        );
        Self { entries, tree }
    }

    pub fn lookup(&self, geometry: &Geometry, country: &str) -> Option<&str> {
        let bbox = envelope(geometry);
        let p = center(geometry);
        if matches!(geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_)) {
            let entries: Vec<_> = self
                .tree
                .locate_in_envelope_intersecting(&bbox)
                .map(|e| &self.entries[e.index])
                .filter(|pc| pc.country == country && geometry.intersects(&pc.center))
                .collect();
            return if entries.len() == 1 { Some(entries[0].code.as_str()) } else { None };
        }
        self.tree
            .locate_in_envelope_intersecting(&bbox)
            .map(|e| &self.entries[e.index])
            .filter(|pc| pc.country == country)
            .filter(|pc| {
                pc.envelope.contains_envelope(&bbox)
                    && pc.geometry.as_ref().is_none_or(|g| geometry.coords_iter().all(|c| g.intersects(&Point(c))))
            })
            .min_by(|a, b| {
                a.geometry
                    .is_none()
                    .cmp(&b.geometry.is_none())
                    .then(distance2(a.center, p).total_cmp(&distance2(b.center, p)))
                    .then(a.code.cmp(&b.code))
            })
            .map(|pc| pc.code.as_str())
    }
}
