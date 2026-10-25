use super::{
    features::Input,
    geometry::{center, envelope, Entry},
};
use geo::{CoordsIter, Distance, Euclidean, Geometry, Intersects, Point};
use osmpbfreader::OsmId;
use rstar::{primitives::GeomWithData, Envelope, RTree, AABB};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A code that fewer addresses carry gets a position only where no other code has more addresses
/// within `NEAR_M` of it. So a few mistagged addresses among the addresses of another code give
/// none, also in an input cut to a box that keeps only them of their code; the addresses of a
/// small village with its own code do.
const MIN_ADDRESSES: usize = 5;
const NEAR_M: f64 = 250.;

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
        let mut points: BTreeMap<(String, String), Vec<Point>> = BTreeMap::new();
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
            points.entry((country.to_string(), code)).or_default().push(p);
        }
        // The median, not the mean: a few addresses far away, also outside the input, do not move it.
        let median = |mut values: Vec<f64>| {
            values.sort_by(f64::total_cmp);
            let n = values.len();
            if n % 2 == 1 {
                values[n / 2]
            } else {
                (values[n / 2 - 1] + values[n / 2]) / 2.
            }
        };
        let around = |p: Point, metres: f64| {
            let dy = metres / 111320.;
            let dx = dy / p.y().to_radians().cos().abs().max(0.01);
            AABB::from_corners([p.x() - dx, p.y() - dy], [p.x() + dx, p.y() + dy])
        };
        let points: Vec<_> = points.into_iter().collect();
        // Each address point, with the index of its code.
        let near = RTree::bulk_load(
            points
                .iter()
                .enumerate()
                .flat_map(|(i, (_, points))| points.iter().map(move |p| GeomWithData::new([p.x(), p.y()], i)))
                .collect(),
        );
        for (i, ((country, code), points)) in points.iter().enumerate() {
            let p = Point::new(
                median(points.iter().map(|p| p.x()).collect()),
                median(points.iter().map(|p| p.y()).collect()),
            );
            if points.len() < MIN_ADDRESSES {
                let mut counts = HashMap::<usize, usize>::new();
                for point in near.locate_in_envelope(&around(p, NEAR_M)) {
                    *counts.entry(point.data).or_default() += 1;
                }
                let own = counts.get(&i).copied().unwrap_or(0);
                if counts.values().any(|&n| n > own) {
                    continue;
                }
            }
            entries.push(Postcode {
                country: country.clone(),
                code: code.clone(),
                geometry: None,
                center: p,
                envelope: around(p, policy.postcode_extent(country)),
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
