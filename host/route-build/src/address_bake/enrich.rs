use super::{
    geometry::{center, envelope, expanded, Entry},
    input::Input,
};
use geo::{Distance, Euclidean, Geometry, Intersects, Point};
use osmpbfreader::{OsmId, Tags};
use rstar::{Envelope, RTree, AABB};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

pub type Address = BTreeMap<String, String>;

fn normalized(s: &str) -> String {
    s.nfkd().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>().replace('ß', "ss")
}

fn fuzzy_area(p: Point, search: u8) -> AABB<[f64; 2]> {
    let metres = match search {
        0..=16 => 15000.,
        17..=18 => 4000.,
        19 => 2000.,
        20 => 1000.,
        _ => 500.,
    };
    let dy = metres / 111320.;
    let dx = dy / p.y().to_radians().cos().abs().max(0.01);
    AABB::from_corners([p.x() - dx, p.y() - dy], [p.x() + dx, p.y() + dy])
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
    policy: &'a super::policy::Policy,
    countries: &'a [&'a str],
    centers: Vec<Point>,
    ranks: Vec<(u8, u8)>,
    areas: RTree<Entry>,
    prepared_areas: BTreeMap<usize, super::areas::Area>,
    buildings: RTree<Entry>,
    roads: RTree<Entry>,
    named_roads: BTreeMap<String, RTree<Entry>>,
    named_places: BTreeMap<String, RTree<Entry>>,
    sources: BTreeMap<OsmId, usize>,
    linked: BTreeMap<usize, usize>,
    postcodes: super::postcodes::Postcodes,
    area_postcodes: Vec<Option<String>>,
    road_contexts: BTreeMap<usize, Address>,
}

impl<'a> Index<'a> {
    pub fn new(input: &'a Input, countries: &'a [&'a str], policy: &'a super::policy::Policy) -> Self {
        assert_eq!(input.features.len(), countries.len());
        let mut centers: Vec<_> = input.features.iter().map(|f| center(&f.geometry)).collect();
        let mut ranks: Vec<_> = input.features.iter().enumerate().map(|(i, f)| policy.ranks(f, countries[i])).collect();
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
                .filter(|(i, f)| matches!(f.source, OsmId::Node(_)) && ranks[*i].1 < 26 && !f.name().is_empty())
                .map(|(index, f)| Entry { index, envelope: envelope(&f.geometry) })
                .collect(),
        );
        let mut linked = BTreeMap::new();
        let mut linked_nodes = BTreeSet::new();
        let boundaries = RTree::bulk_load(
            input
                .features
                .iter()
                .enumerate()
                .filter(|(i, f)| {
                    f.tag("boundary") == "administrative"
                        && (1..26).contains(&ranks[*i].1)
                        && matches!(f.geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_))
                })
                .map(|(index, f)| Entry { index, envelope: envelope(&f.geometry) })
                .collect(),
        );
        let boundary_areas: BTreeMap<_, _> =
            boundaries.iter().map(|e| (e.index, super::areas::Area::new(&input.features[e.index].geometry))).collect();
        let mut ordered: Vec<_> = input.features.iter().enumerate().collect();
        ordered.sort_by_key(|(i, f)| (ranks[*i].0, f.source));
        for (i, f) in ordered {
            let admin = f.tag("admin_level").parse::<u8>().unwrap_or(0);
            let parent_level = if f.tag("boundary") == "administrative" {
                boundaries
                    .locate_in_envelope_intersecting(&expanded(centers[i], 0.))
                    .filter(|e| e.index != i && countries[e.index] == countries[i])
                    .filter_map(|e| {
                        input.features[e.index]
                            .tag("admin_level")
                            .parse::<u8>()
                            .ok()
                            .filter(|level| *level > 3 && *level < admin)
                            .map(|level| (level, e.index))
                    })
                    .filter(|(_, j)| boundary_areas[j].contains(centers[i]))
                    .max_by_key(|(level, _)| *level)
                    .map(|(_, j)| ranks[j].1)
                    .unwrap_or(3)
            } else {
                3
            };
            if parent_level >= ranks[i].1 && ranks[i].1 > 0 && f.tag("boundary") == "administrative" {
                ranks[i].1 = (parent_level + 2).min(25);
            }

            let rank = ranks[i].1;
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
                            || (normalized(f.name()) == normalized(n.name()) && rank == ranks[e.index].1))
                })
                .min_by_key(|e| e.index)
                .map(|e| e.index);
            if let Some(j) = label.or(matched) {
                centers[i] = centers[j];
                if ranks[j].1 > parent_level && ranks[j].1 < 26 {
                    ranks[i].1 = ranks[j].1;
                }
                linked.insert(i, j);
                linked_nodes.insert(j);
            } else if f.tag("boundary") == "administrative" && (4..26).contains(&ranks[i].1) {
                let rank = policy.place_rank(f.tag("place"), countries[i]);
                if rank > parent_level && rank < 26 {
                    ranks[i].1 = rank;
                }
            }
        }
        let mut areas = Vec::new();
        for (i, f) in input.features.iter().enumerate() {
            if matches!(f.source, OsmId::Node(_))
                && !f.tag("place").is_empty()
                && (16..24).contains(&ranks[i].1)
                && !linked_nodes.contains(&i)
                && boundaries.locate_in_envelope_intersecting(&expanded(centers[i], 0.)).any(|e| {
                    countries[e.index] == countries[i]
                        && ranks[e.index].1 == ranks[i].1
                        && boundary_areas[&e.index].contains(centers[i])
                })
            {
                ranks[i].1 += 2;
            }
        }
        let mut buildings = Vec::new();
        let mut roads = Vec::new();
        let mut named_roads: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut named_places: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        for (index, f) in input.features.iter().enumerate() {
            let (search, rank) = ranks[index];
            let mut extent = envelope(&f.geometry);
            if (4..26).contains(&rank) && !f.name().is_empty() && !linked_nodes.contains(&index) {
                if matches!(f.geometry, Geometry::Point(_)) {
                    extent = fuzzy_area(centers[index], search);
                }
                areas.push(Entry { index, envelope: extent });
                if rank >= 16 {
                    for (key, name) in f
                        .tags
                        .iter()
                        .filter(|(k, _)| k.as_str() == "name" || k.starts_with("name:") || k.as_str() == "alt_name")
                    {
                        let _ = key;
                        for alias in name.split(';') {
                            named_places
                                .entry(normalized(alias))
                                .or_default()
                                .push(Entry { index, envelope: envelope(&f.geometry) });
                        }
                    }
                }
            }
            if search == 30
                && f.address_tags()
                && matches!(f.geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_))
            {
                buildings.push(Entry { index, envelope: extent });
            }
            if f.road() && !f.name().is_empty() {
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
                            .map(|index| Entry { index, envelope: envelope(&input.features[index].geometry) })
                            .collect(),
                    ),
                )
            })
            .collect();
        let mut boundary_areas = boundary_areas;
        let prepared_areas = areas
            .iter()
            .filter(|e| matches!(input.features[e.index].geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_)))
            .map(|e| {
                (
                    e.index,
                    boundary_areas
                        .remove(&e.index)
                        .unwrap_or_else(|| super::areas::Area::new(&input.features[e.index].geometry)),
                )
            })
            .collect();
        let mut result = Self {
            input,
            policy,
            countries,
            centers,
            ranks,
            areas: RTree::bulk_load(areas),
            prepared_areas,
            buildings: RTree::bulk_load(buildings),
            roads: RTree::bulk_load(roads),
            named_roads,
            named_places: named_places.into_iter().map(|(name, entries)| (name, RTree::bulk_load(entries))).collect(),
            sources,
            linked,
            postcodes: super::postcodes::Postcodes::new(input, countries, policy),
            area_postcodes: vec![None; input.features.len()],
            road_contexts: BTreeMap::new(),
        };
        let mut areas: Vec<_> = result.areas.iter().map(|e| e.index).collect();
        areas.sort_by_key(|&i| result.ranks[i].1);
        for i in areas {
            result.area_postcodes[i] = policy
                .postcode(input.features[i].tag("addr:postcode"), countries[i])
                .or_else(|| result.context(i, false).remove("postcode"));
        }
        for (i, f) in
            input.features.iter().enumerate().filter(|(i, f)| f.road() || (4..26).contains(&result.ranks[*i].1))
        {
            let mut context = result.context(i, true);
            if !f.road() {
                context.insert(part(result.ranks[i].1).into(), f.name().into());
            }
            overlay(&mut context, &f.tags);
            result.road_contexts.insert(i, context);
        }
        result
    }

    pub fn center(&self, i: usize) -> Point {
        self.centers[i]
    }

    pub fn tags(&self, i: usize) -> &Tags {
        let f = &self.input.features[i];
        if !f.tags.keys().any(|k| k.starts_with("addr:") && k.as_str() != "addr:housename")
            && matches!(f.source, OsmId::Node(_))
        {
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

    fn road_distance(&self, i: usize, geometry: &Geometry) -> f64 {
        Euclidean.distance(&self.input.features[i].geometry, geometry).powi(2)
    }

    fn parent(&self, i: usize, tags: &Tags) -> Option<usize> {
        let bbox = super::geometry::bounds_geometry(&self.input.features[i].geometry);
        let extent = envelope(&bbox);
        let expand = |radius: f64| {
            AABB::from_corners(
                [extent.lower()[0] - radius, extent.lower()[1] - radius],
                [extent.upper()[0] + radius, extent.upper()[1] + radius],
            )
        };
        if let Some(streets) = self.input.associated.get(&self.input.features[i].source) {
            if let Some(parent) = streets
                .iter()
                .filter_map(|s| self.sources.get(s).copied())
                .filter(|&j| self.input.features[j].road() && self.countries[i] == self.countries[j])
                .min_by(|&a, &b| self.road_distance(a, &bbox).total_cmp(&self.road_distance(b, &bbox)).then(a.cmp(&b)))
            {
                return Some(parent);
            }
        }
        if let Some(name) = tags.get("addr:street") {
            if let Some(roads) = self.named_roads.get(&normalized(name)) {
                if let Some(parent) = roads
                    .locate_in_envelope_intersecting(&expand(0.015))
                    .filter(|e| self.countries[i] == self.countries[e.index])
                    .min_by(|a, b| {
                        self.road_distance(a.index, &bbox)
                            .total_cmp(&self.road_distance(b.index, &bbox))
                            .then(a.index.cmp(&b.index))
                    })
                {
                    return Some(parent.index);
                }
            }
        }
        if let Some(places) = tags.get("addr:place").and_then(|name| self.named_places.get(&normalized(name))) {
            if let Some(parent) = places
                .locate_in_envelope_intersecting(&expand(0.04))
                .filter(|e| self.countries[i] == self.countries[e.index])
                .min_by(|a, b| {
                    self.road_distance(a.index, &bbox)
                        .total_cmp(&self.road_distance(b.index, &bbox))
                        .then(a.index.cmp(&b.index))
                })
            {
                return Some(parent.index);
            }
        }
        if tags.contains_key("addr:place")
            || (extent.upper()[0] - extent.lower()[0]) * (extent.upper()[1] - extent.lower()[1]) >= 0.005
        {
            let p = center(&bbox);
            return self
                .areas
                .locate_in_envelope_intersecting(&expanded(p, 0.))
                .filter(|e| (5..26).contains(&self.ranks[e.index].1) && self.countries[i] == self.countries[e.index])
                .filter(|e| self.prepared_areas.get(&e.index).is_some_and(|a| a.contains(p)))
                .max_by_key(|e| self.ranks[e.index].1)
                .map(|e| e.index);
        }
        if let Geometry::LineString(line) = &self.input.features[i].geometry {
            if tags.contains_key("_interpolation_range") {
                let points = [
                    super::interpolation::point(line, 0.),
                    super::interpolation::point(line, 0.5),
                    super::interpolation::point(line, 1.),
                ];
                let mut radius = 0.0005;
                while radius < 0.01 {
                    let nearest = self
                        .roads
                        .locate_in_envelope_intersecting(&expand(radius))
                        .filter(|e| self.countries[e.index] == self.countries[i])
                        .filter(|e| Euclidean.distance(&self.input.features[e.index].geometry, line) <= radius)
                        .map(|e| {
                            (
                                e.index,
                                points
                                    .iter()
                                    .map(|p| Euclidean.distance(&self.input.features[e.index].geometry, p))
                                    .sum::<f64>(),
                            )
                        })
                        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
                    if let Some((index, _)) = nearest {
                        return Some(index);
                    }
                    radius *= 2.;
                }
                return None;
            }
        }
        let mut radius = 0.00005;
        while radius < 0.1 {
            let nearest = self
                .roads
                .locate_in_envelope_intersecting(&expand(radius))
                .filter(|e| self.countries[e.index] == self.countries[i])
                .map(|e| (e.index, self.road_distance(e.index, &bbox)))
                .filter(|(_, d)| *d <= radius * radius)
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            if let Some((index, _)) = nearest {
                return Some(index);
            }
            radius *= 2.;
        }
        None
    }

    fn context(&self, i: usize, infer_postcode: bool) -> Address {
        let f = &self.input.features[i];
        let p = self.centers[i];
        let point = Geometry::Point(p);
        let geometry = if matches!(f.geometry, Geometry::LineString(_) | Geometry::MultiLineString(_)) {
            &f.geometry
        } else {
            &point
        };
        let maxrank = self.ranks[i].1;
        let maxrank = if maxrank == 0 { 25 } else { maxrank.min(25) };
        let mut candidates: Vec<_> = self
            .areas
            .locate_in_envelope_intersecting(&envelope(geometry))
            .filter(|e| e.index != i && self.countries[e.index] == self.countries[i])
            .filter(|e| self.ranks[e.index].1 < maxrank)
            .filter(|e| {
                matches!(self.input.features[e.index].geometry, Geometry::Point(_))
                    || self
                        .prepared_areas
                        .get(&e.index)
                        .map(|a| a.intersects(geometry))
                        .unwrap_or_else(|| self.input.features[e.index].geometry.intersects(geometry))
            })
            .map(|e| {
                let item = &self.input.features[e.index];
                let (search, rank) = self.ranks[e.index];
                let guess = matches!(item.geometry, Geometry::Point(_));
                let weight: f64 = match search {
                    15 if rank == 16 => 0.2,
                    16 if rank == 16 => 0.25,
                    18 if rank == 16 => 0.5,
                    _ => 1.,
                };
                let distance = Euclidean.distance(geometry, &self.centers[e.index]);
                let distance = if guess {
                    distance
                } else {
                    self.prepared_areas.get(&e.index).map(|a| a.distance(p)).unwrap_or(0.) + 1e-5 * distance
                };
                (rank, guess, distance * weight, e.index)
            })
            .collect();
        candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)).then(a.3.cmp(&b.3)));
        let mut chosen = BTreeMap::new();
        let mut boundary: Option<usize> = None;
        let mut node_area: Option<AABB<[f64; 2]>> = None;
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
            if guess
                && node_area
                    .is_some_and(|area| !area.contains_point(&[self.centers[index].x(), self.centers[index].y()]))
            {
                continue;
            }
            chosen.insert(rank, index);
            if guess {
                node_area = Some(fuzzy_area(self.centers[index], self.ranks[index].0));
            } else {
                node_area = None;
                boundary = Some(index);
            }
        }
        let mut a = Address::new();
        for (key, value) in self.policy.country_names(self.countries[i]) {
            if key == "name" || key == "name:en" {
                a.insert(key.replacen("name", "country", 1), value.to_string());
            }
        }
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
            for (tag, name) in item.tags.iter().filter(|(k, _)| k.starts_with("name:")) {
                let lang = tag.strip_prefix("name:").unwrap();
                a.entry(format!("{key}:{lang}")).or_insert_with(|| name.to_string());
            }
            if let Some(code) = &self.area_postcodes[index] {
                a.entry("postcode".into()).or_insert_with(|| code.clone());
            }
        }
        if infer_postcode && !a.contains_key("postcode") {
            let geometry = if self.ranks[i].1 > 25 { &point } else { &f.geometry };
            if let Some(postcode) = self.postcodes.lookup(geometry, self.countries[i]) {
                a.insert("postcode".into(), postcode.into());
            }
        }
        a
    }

    pub fn address(&self, i: usize) -> Address {
        let f = &self.input.features[i];
        let tags = self.tags(i);
        let parent = if f.road() { None } else { self.parent(i, tags) };
        let mut a = self.road_contexts.get(&parent.unwrap_or(i)).cloned().unwrap_or_else(|| self.context(i, true));
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
        if let Some(code) = a.get("postcode").and_then(|s| self.policy.postcode(s, self.countries[i])) {
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

fn numbers<'a>(tags: &'a Tags, key: &str) -> Vec<&'a str> {
    static INVALID: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let invalid = INVALID.get_or_init(|| regex::Regex::new(r"(?:^|.*,)[^\d,]{3,}(?:,.*|$)").unwrap());
    tags.get(key)
        .filter(|s| !invalid.is_match(s))
        .map(|s| {
            s.split([';', ',']).map(str::trim).filter(|s| !s.is_empty() && s.encode_utf16().count() < 20).collect()
        })
        .unwrap_or_default()
}

pub fn house_numbers(tags: &Tags) -> Vec<&str> {
    let result = numbers(tags, "addr:housenumber");
    if result.is_empty() && tags.contains_key("addr:street") {
        numbers(tags, "addr:streetnumber")
    } else {
        result
    }
}

pub fn house_addresses(tags: &Tags) -> Vec<(&str, Option<&str>)> {
    let place = tags.get("addr:place").map(|s| s.as_str()).filter(|s| !s.trim().is_empty());
    let mut result: Vec<_> = if let Some(place) = place {
        numbers(tags, "addr:conscriptionnumber").into_iter().map(|n| (n, Some(place))).collect()
    } else {
        vec![]
    };
    if tags.contains_key("addr:street") {
        result.extend(house_numbers(tags).into_iter().map(|n| (n, None)));
    }
    if result.is_empty() {
        let street = tags.get("addr:block_number").map(|s| s.as_str()).filter(|s| !s.trim().is_empty()).or_else(|| {
            if tags.contains_key("addr:street") {
                None
            } else {
                place
            }
        });
        result.extend(numbers(tags, "addr:housenumber").into_iter().map(|n| (n, street)));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::super::geometry::coordinates;
    use super::super::input::Feature;
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
        let policy = super::super::policy::Policy::test_policy();
        let index = Index::new(&input, &countries, &policy);
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
        let policy = super::super::policy::Policy::test_policy();
        let index = Index::new(&input, &countries, &policy);
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
        let policy = super::super::policy::Policy::test_policy();
        let index = Index::new(&input, &countries, &policy);
        assert_eq!(index.address(0).get("postcode").unwrap(), "80481");
        assert_eq!(index.address(1).get("postcode").unwrap(), "80481");
        assert!(!index.address(2).contains_key("postcode"));
        assert!(!index.address(3).contains_key("postcode"));
    }
}
