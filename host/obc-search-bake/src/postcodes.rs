use super::{
    geometry::{center, envelope, Entry},
    input::Input,
};
use geo::{CoordsIter, Distance, Euclidean, Geometry, Intersects, Point};
use osmpbfreader::OsmId;
use rstar::{Envelope, RTree, AABB};
use std::collections::{BTreeMap, BTreeSet};

struct Postcode {
    country: String,
    code: String,
    geometry: Option<super::areas::Area>,
    center: Point,
    envelope: AABB<[f64; 2]>,
}

pub struct Postcodes {
    entries: Vec<Postcode>,
    tree: RTree<Entry>,
}

impl Postcodes {
    pub fn new(input: &Input, countries: &[&str], policy: &super::policy::Policy) -> Self {
        let mut entries = Vec::new();
        let mut covered = BTreeSet::new();
        for (i, f) in input.features.iter().enumerate() {
            let country = countries[i];
            if f.tag("boundary") != "postal_code"
                || !matches!(f.source, OsmId::Relation(_))
                || !matches!(f.geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_))
            {
                continue;
            }
            let code = if f.tag("postal_code").is_empty() { f.tag("addr:postcode") } else { f.tag("postal_code") };
            if let Some(code) = policy.postcode(code, country) {
                covered.insert((country.to_string(), code.clone()));
                entries.push(Postcode {
                    country: country.to_string(),
                    code,
                    center: center(&f.geometry),
                    envelope: envelope(&f.geometry),
                    geometry: Some(super::areas::Area::new(&f.geometry)),
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
            let code = f.tag("addr:postcode");
            if code.contains([',', ';']) || f.tags.contains_key("_interpolation_range") {
                continue;
            }
            let Some(code) = policy.postcode(code, country) else {
                continue;
            };
            if covered.contains(&(country.to_string(), code.clone())) {
                continue;
            }
            let p = center(&f.geometry);
            if areas
                .locate_in_envelope_intersecting(&super::geometry::expanded(p, 0.))
                .any(|e| entries[e.index].geometry.as_ref().is_some_and(|g| g.contains(p)))
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
            let metres = policy.postcode_extent(&country);
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
                    && pc.geometry.as_ref().is_none_or(|g| geometry.coords_iter().all(|c| g.contains(Point(c))))
            })
            .min_by(|a, b| {
                a.geometry
                    .is_none()
                    .cmp(&b.geometry.is_none())
                    .then(Euclidean.distance(&a.center, geometry).total_cmp(&Euclidean.distance(&b.center, geometry)))
                    .then(a.code.cmp(&b.code))
            })
            .map(|pc| pc.code.as_str())
    }
}
