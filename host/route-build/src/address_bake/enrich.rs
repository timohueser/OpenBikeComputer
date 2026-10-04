use super::{
    geometry::{center, distance2, envelope, expanded, line_distance2, Entry},
    input::{Feature, Input},
};
use geo::{Geometry, Intersects, Point};
use osmpbfreader::{OsmId, Tags};
use rstar::{RTree, AABB};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

pub type Address = BTreeMap<String, String>;

fn normalized(s: &str) -> String {
    s.nfkd().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>().replace('ß', "ss")
}

fn ranks(f: &Feature, country: &str) -> (u8, u8) {
    if f.tag("boundary") == "administrative" {
        let rank = f.tag("admin_level").parse::<u8>().ok().filter(|r| (2..=12).contains(r)).map(|r| 2 * r).unwrap_or(0);
        return (rank, if country == "de" && rank == 10 { 0 } else { rank });
    }
    match f.tag("place") {
        "country" => (4, 0),
        "state" | "province" => (8, 0),
        "county" | "district" => (12, if country == "de" { 0 } else { 12 }),
        "municipality" => (14, 14),
        "city" => (16, 16),
        "town" => (18, 16),
        "village" => (19, 16),
        "suburb" => (19, 20),
        "hamlet" | "croft" => (20, 20),
        "quarter" => (20, 22),
        "neighbourhood" => (24, 24),
        "isolated_dwelling" | "farm" => (22, 25),
        "locality" | "square" | "city_block" => (25, 25),
        _ if f.road() => (26, 26),
        _ => (30, 30),
    }
}

fn part(rank: u8) -> &'static str {
    match rank {
        4 => "country",
        5..=9 => "state",
        10..=12 => "county",
        13..=16 => "city",
        17..=21 => "district",
        22..=25 => "locality",
        26..=28 => "street",
        _ => "other",
    }
}

pub struct Index<'a> {
    input: &'a Input,
    countries: &'a [&'a str],
    centers: Vec<Point>,
    areas: RTree<Entry>,
    prepared_areas: BTreeMap<usize, super::areas::Area>,
    buildings: RTree<Entry>,
    roads: RTree<Entry>,
    named_roads: BTreeMap<String, RTree<Entry>>,
    sources: BTreeMap<OsmId, usize>,
    linked: BTreeMap<usize, usize>,
    postcodes: super::postcodes::Postcodes,
    road_contexts: BTreeMap<usize, Address>,
}

impl<'a> Index<'a> {
    pub fn new(input: &'a Input, countries: &'a [&'a str]) -> Self {
        assert_eq!(input.features.len(), countries.len());
        let centers: Vec<_> = input.features.iter().map(|f| center(&f.geometry)).collect();
        let sources: BTreeMap<_, _> = input
            .features
            .iter()
            .enumerate()
            .filter(|(_, f)| f.road() || (matches!(f.source, OsmId::Node(_)) && !f.tag("place").is_empty()))
            .map(|(i, f)| (f.source, i))
            .collect();
        let nodes = RTree::bulk_load(
            input
                .features
                .iter()
                .enumerate()
                .filter(|(i, f)| {
                    matches!(f.source, OsmId::Node(_)) && ranks(f, countries[*i]).1 < 26 && !f.name().is_empty()
                })
                .map(|(index, f)| Entry { index, envelope: envelope(&f.geometry) })
                .collect(),
        );
        let mut linked = BTreeMap::new();
        let mut linked_nodes = BTreeSet::new();
        for (i, f) in input.features.iter().enumerate() {
            let rank = ranks(f, countries[i]).1;
            if rank == 0 || rank >= 26 || !matches!(f.geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_)) {
                continue;
            }
            let label = input
                .labels
                .get(&f.source)
                .into_iter()
                .flatten()
                .filter_map(|id| sources.get(id))
                .find(|&&j| !input.features[j].tag("place").is_empty() && !linked_nodes.contains(&j))
                .copied();
            let matched = nodes
                .locate_in_envelope_intersecting(&envelope(&f.geometry))
                .filter(|e| {
                    let n = &input.features[e.index];
                    !linked_nodes.contains(&e.index)
                        && f.geometry.intersects(&centers[e.index])
                        && ((!f.tag("wikidata").is_empty() && f.tag("wikidata") == n.tag("wikidata"))
                            || (normalized(f.name()) == normalized(n.name()) && rank == ranks(n, countries[e.index]).1))
                })
                .min_by_key(|e| e.index)
                .map(|e| e.index);
            if let Some(j) = label.or(matched) {
                linked.insert(i, j);
                linked_nodes.insert(j);
            }
        }
        let mut areas = Vec::new();
        let mut buildings = Vec::new();
        let mut roads = Vec::new();
        let mut named_roads: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, f) in input.features.iter().enumerate() {
            let (search, rank) = ranks(f, countries[index]);
            let mut extent = envelope(&f.geometry);
            if (4..26).contains(&rank) && !f.name().is_empty() && !linked_nodes.contains(&index) {
                if matches!(f.geometry, Geometry::Point(_)) {
                    let metres = match search {
                        0..=16 => 15000.,
                        17..=18 => 4000.,
                        19 => 2000.,
                        20 => 1000.,
                        _ => 500.,
                    };
                    let p = centers[index];
                    let dy = metres / 111320.;
                    let dx = dy / p.y().to_radians().cos().abs().max(0.01);
                    extent = AABB::from_corners([p.x() - dx, p.y() - dy], [p.x() + dx, p.y() + dy]);
                }
                areas.push(Entry { index, envelope: extent });
            }
            if !f.tag("building").is_empty()
                && f.address_tags()
                && matches!(f.geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_))
            {
                buildings.push(Entry { index, envelope: extent });
            }
            if f.road() {
                roads.push(Entry { index, envelope: extent });
                for (key, name) in f
                    .tags
                    .iter()
                    .filter(|(k, _)| k.as_str() == "name" || k.starts_with("name:") || k.as_str() == "alt_name")
                {
                    let _ = key;
                    for alias in name.split(';') {
                        named_roads.entry(normalized(alias)).or_default().push(index);
                    }
                }
            }
        }
        let named_roads = named_roads
            .into_iter()
            .map(|(name, roads)| {
                (
                    name,
                    RTree::bulk_load(
                        roads
                            .into_iter()
                            .map(|index| Entry { index, envelope: expanded(centers[index], 0.) })
                            .collect(),
                    ),
                )
            })
            .collect();
        let prepared_areas = areas
            .iter()
            .filter(|e| matches!(input.features[e.index].geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_)))
            .map(|e| (e.index, super::areas::Area::new(&input.features[e.index].geometry)))
            .collect();
        let mut result = Self {
            input,
            countries,
            centers,
            areas: RTree::bulk_load(areas),
            prepared_areas,
            buildings: RTree::bulk_load(buildings),
            roads: RTree::bulk_load(roads),
            named_roads,
            sources,
            linked,
            postcodes: super::postcodes::Postcodes::new(input, countries),
            road_contexts: BTreeMap::new(),
        };
        for (i, _) in input.features.iter().enumerate().filter(|(_, f)| f.road()) {
            let context = result.context(i);
            result.road_contexts.insert(i, context);
        }
        result
    }

    pub fn center(&self, i: usize) -> Point {
        self.centers[i]
    }

    pub fn tags(&self, i: usize) -> &Tags {
        let f = &self.input.features[i];
        if !f.address_tags() && matches!(f.source, OsmId::Node(_)) {
            let p = self.centers[i];
            if let Some(building) = self
                .buildings
                .locate_in_envelope_intersecting(&expanded(p, 0.))
                .filter(|e| e.index != i && self.input.features[e.index].geometry.intersects(&p))
                .min_by_key(|e| self.input.features[e.index].source)
            {
                return &self.input.features[building.index].tags;
            }
        }
        &f.tags
    }

    fn road_distance(&self, i: usize, p: Point) -> f64 {
        match &self.input.features[i].geometry {
            Geometry::LineString(l) => line_distance2(p, l),
            _ => distance2(self.centers[i], p),
        }
    }

    fn parent(&self, i: usize, tags: &Tags) -> Option<usize> {
        let p = self.centers[i];
        if let Some(streets) = self.input.associated.get(&self.input.features[i].source) {
            if let Some(parent) = streets
                .iter()
                .filter_map(|s| self.sources.get(s).copied())
                .filter(|&j| self.input.features[j].road() && self.countries[i] == self.countries[j])
                .min_by(|&a, &b| self.road_distance(a, p).total_cmp(&self.road_distance(b, p)).then(a.cmp(&b)))
            {
                return Some(parent);
            }
        }
        if let Some(name) = tags.get("addr:street") {
            if let Some(roads) = self.named_roads.get(&normalized(name)) {
                if let Some(parent) = roads
                    .locate_in_envelope_intersecting(&expanded(p, 0.015))
                    .filter(|e| self.countries[i] == self.countries[e.index])
                    .min_by(|a, b| {
                        distance2(self.centers[a.index], p)
                            .total_cmp(&distance2(self.centers[b.index], p))
                            .then(a.index.cmp(&b.index))
                    })
                {
                    return Some(parent.index);
                }
            }
        }
        if tags.contains_key("addr:place") && !tags.contains_key("addr:street") {
            return None;
        }
        self.roads
            .locate_in_envelope_intersecting(&expanded(p, 0.0512))
            .filter(|e| self.countries[e.index] == self.countries[i])
            .map(|e| (e.index, self.road_distance(e.index, p)))
            .filter(|(_, d)| *d <= 0.0512_f64.powi(2))
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
            .map(|(i, _)| i)
    }

    fn context(&self, i: usize) -> Address {
        let f = &self.input.features[i];
        let p = self.centers[i];
        let maxrank = ranks(f, self.countries[i]).1;
        let maxrank = if maxrank == 0 { 30 } else { maxrank };
        let mut candidates: Vec<_> = self
            .areas
            .locate_in_envelope_intersecting(&envelope(&f.geometry))
            .filter(|e| e.index != i && self.countries[e.index] == self.countries[i])
            .filter(|e| ranks(&self.input.features[e.index], self.countries[e.index]).1 < maxrank)
            .filter(|e| {
                matches!(self.input.features[e.index].geometry, Geometry::Point(_))
                    || self
                        .prepared_areas
                        .get(&e.index)
                        .map(|a| a.intersects(&f.geometry))
                        .unwrap_or_else(|| self.input.features[e.index].geometry.intersects(&f.geometry))
            })
            .map(|e| {
                let item = &self.input.features[e.index];
                let (search, rank) = ranks(item, self.countries[e.index]);
                let guess = matches!(item.geometry, Geometry::Point(_));
                let weight: f64 = match search {
                    16 if rank == 16 => 0.25,
                    18 if rank == 16 => 0.5,
                    _ => 1.,
                };
                (rank, guess, distance2(self.centers[e.index], p) * weight.powi(2), e.index)
            })
            .collect();
        candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)).then(a.3.cmp(&b.3)));
        let mut chosen = BTreeMap::new();
        let mut boundary: Option<usize> = None;
        for (rank, guess, _, index) in candidates {
            if chosen.contains_key(&rank) {
                continue;
            }
            if let Some(b) = boundary {
                if !self
                    .prepared_areas
                    .get(&b)
                    .map(|a| a.contains(self.centers[index]))
                    .unwrap_or_else(|| self.input.features[b].geometry.intersects(&self.centers[index]))
                {
                    continue;
                }
            }
            chosen.insert(rank, index);
            if !guess {
                boundary = Some(index);
            }
        }
        let mut a = Address::new();
        for (rank, index) in chosen.into_iter().rev() {
            let item = &self.input.features[index];
            let key = part(rank);
            let name = self
                .linked
                .get(&index)
                .map(|&i| self.input.features[i].name())
                .filter(|s| !s.is_empty())
                .unwrap_or(item.name());
            a.entry(key.into()).or_insert_with(|| name.into());
            for lang in ["de", "en", "fr", "it"] {
                let name = item.tag(&format!("name:{lang}"));
                if !name.is_empty() {
                    a.entry(format!("{key}:{lang}")).or_insert_with(|| name.into());
                }
            }
            if !item.tag("addr:postcode").is_empty() {
                a.entry("postcode".into()).or_insert_with(|| item.tag("addr:postcode").into());
            }
        }
        if !a.contains_key("postcode") {
            if let Some(postcode) = self.postcodes.lookup(&f.geometry, self.countries[i]) {
                a.insert("postcode".into(), postcode.into());
            }
        }
        a
    }

    pub fn address(&self, i: usize) -> Address {
        let f = &self.input.features[i];
        let tags = self.tags(i);
        let parent = if f.road() { None } else { self.parent(i, tags) };
        let mut a = self.road_contexts.get(&parent.unwrap_or(i)).cloned().unwrap_or_else(|| self.context(i));
        if let Some(parent) = parent {
            a.insert("street".into(), self.input.features[parent].name().into());
            overlay(&mut a, &self.input.features[parent].tags);
        }
        overlay(&mut a, tags);
        if !a.contains_key("postcode") {
            if let Some(postcode) = self.postcodes.lookup(&self.centers[i].into(), self.countries[i]) {
                a.insert("postcode".into(), postcode.into());
            }
        }
        if let Some(code) = a.get("postcode").and_then(|s| super::postcodes::normalized(s, self.countries[i])) {
            a.insert("postcode".into(), code);
        }
        if !tags.contains_key("addr:street") {
            if let Some(place) = tags.get("addr:place") {
                a.insert("street".into(), place.to_string());
            }
        }
        if let Some(block) = tags.get("addr:block_number") {
            a.insert("street".into(), block.to_string());
        }
        a
    }
}

fn overlay(a: &mut Address, tags: &Tags) {
    for (key, value) in tags.iter() {
        if let Some(key) = key.strip_prefix("addr:") {
            let key = match key {
                "province" => "state",
                "suburb" => "district",
                "neighbourhood" => "locality",
                _ => key,
            };
            if !value.trim().is_empty() {
                a.insert(key.into(), value.trim().into());
            }
        }
    }
}

pub fn house_numbers(tags: &Tags) -> Vec<&str> {
    tags.get("addr:housenumber")
        .or_else(|| tags.get("addr:streetnumber"))
        .map(|s| {
            s.split([';', ','])
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.len() < 20 && s.chars().any(|c| c.is_ascii_digit()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::geometry::coordinates;
    use super::*;
    use osmpbfreader::{NodeId, WayId};

    fn feature(id: i64, g: Geometry, tags: &[(&str, &str)]) -> Feature {
        Feature {
            source: if matches!(g, Geometry::Point(_)) { OsmId::Node(NodeId(id)) } else { OsmId::Way(WayId(id)) },
            geometry: g,
            tags: tags.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
        }
    }
    #[test]
    fn addresses_use_street_context_then_explicit_tags_and_building_inheritance() {
        let input = Input {
            features: vec![
                feature(1, Point::new(8., 48.).into(), &[("place", "city"), ("name", "City")]),
                feature(
                    2,
                    Geometry::LineString(coordinates([[8., 48.], [8.01, 48.]])),
                    &[("highway", "residential"), ("name", "Main")],
                ),
                feature(
                    3,
                    Geometry::Polygon(geo::Polygon::new(
                        coordinates([[8., 48.], [8.001, 48.], [8.001, 48.001], [8., 48.001], [8., 48.]]),
                        vec![],
                    )),
                    &[
                        ("building", "yes"),
                        ("addr:housenumber", "12;14"),
                        ("addr:street", "Main"),
                        ("addr:city", "Postal City"),
                        ("addr:postcode", "12345"),
                    ],
                ),
                feature(4, Point::new(8.0005, 48.0005).into(), &[("amenity", "cafe")]),
                feature(5, Point::new(8.0006, 48.0005).into(), &[("addr:housenumber", "16"), ("addr:place", "Farm")]),
                feature(
                    6,
                    Geometry::LineString(coordinates([[8.0002, 48.0002], [8.0008, 48.0008]])),
                    &[("amenity", "cafe")],
                ),
            ],
            associated: BTreeMap::new(),
            labels: BTreeMap::new(),
            incomplete_geometries: 0,
            nodes: 0,
        };
        let countries = vec!["de"; input.features.len()];
        let index = Index::new(&input, &countries);
        assert_eq!(index.address(3).get("city").unwrap(), "Postal City");
        assert_eq!(house_numbers(index.tags(3)), ["12", "14"]);
        assert_eq!(index.address(3).get("street").unwrap(), "Main");
        assert_eq!(index.address(4).get("street").unwrap(), "Farm");
        assert_eq!(house_numbers(index.tags(4)), ["16"]);
        assert!(house_numbers(index.tags(5)).is_empty());
    }
    #[test]
    fn associated_street_precedes_proximity_and_city_boundaries_precede_place_guesses() {
        let input = Input {
            features: vec![
                feature(1, Point::new(8., 48.).into(), &[("place", "city"), ("name", "Guessed")]),
                feature(
                    2,
                    Geometry::Polygon(geo::Polygon::new(
                        coordinates([[7.99, 47.99], [8.02, 47.99], [8.02, 48.02], [7.99, 48.02], [7.99, 47.99]]),
                        vec![],
                    )),
                    &[("boundary", "administrative"), ("admin_level", "8"), ("name", "Boundary")],
                ),
                feature(
                    3,
                    Geometry::LineString(coordinates([[8., 48.], [8.01, 48.]])),
                    &[("highway", "residential"), ("name", "Near")],
                ),
                feature(
                    4,
                    Geometry::LineString(coordinates([[8., 48.01], [8.01, 48.01]])),
                    &[("highway", "residential"), ("name", "Associated")],
                ),
                feature(5, Point::new(8.005, 48.0001).into(), &[("addr:housenumber", "1")]),
            ],
            associated: BTreeMap::from([(OsmId::Node(NodeId(5)), vec![OsmId::Way(WayId(4))])]),
            labels: BTreeMap::new(),
            incomplete_geometries: 0,
            nodes: 0,
        };
        let countries = vec!["de"; input.features.len()];
        let index = Index::new(&input, &countries);
        let a = index.address(4);
        assert_eq!(a.get("street").unwrap(), "Associated");
        assert_eq!(a.get("city").unwrap(), "Boundary");
    }

    #[test]
    fn postcode_inference_stays_inside_country_and_coverage_and_normalizes_zip_extensions() {
        let input = Input {
            features: vec![
                feature(
                    1,
                    Point::new(-105., 40.).into(),
                    &[("addr:postcode", "80481-1234"), ("addr:housenumber", "1"), ("addr:place", "Farm")],
                ),
                feature(2, Point::new(-105.001, 40.001).into(), &[("addr:housenumber", "2"), ("addr:place", "Farm")]),
                feature(3, Point::new(-104., 40.).into(), &[("addr:housenumber", "3"), ("addr:place", "Farm")]),
                feature(4, Point::new(-105.001, 40.001).into(), &[("addr:housenumber", "4"), ("addr:place", "Farm")]),
            ],
            associated: BTreeMap::new(),
            labels: BTreeMap::new(),
            incomplete_geometries: 0,
            nodes: 0,
        };
        let countries = vec!["us", "us", "us", "de"];
        let index = Index::new(&input, &countries);
        assert_eq!(index.address(0).get("postcode").unwrap(), "80481");
        assert_eq!(index.address(1).get("postcode").unwrap(), "80481");
        assert!(!index.address(2).contains_key("postcode"));
        assert!(!index.address(3).contains_key("postcode"));
    }
}
