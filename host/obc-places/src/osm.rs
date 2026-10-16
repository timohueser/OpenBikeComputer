//! Place metadata and topology harvested independently of drawing and graph algorithms.

use crate::hours;
use crate::metadata::{self as poi, Poi};
use crate::routing::access::{self, Barrier};
use crate::routing::{RoutableWay, NO_RIDE_BACKWARD, NO_RIDE_FORWARD};
use obc_pbf::bbox::to_deg;
use obc_pbf::scan::Keyed;
use std::collections::{HashMap, HashSet};

/// Capture a routable way's node-id sequence and µdeg coords for the nav graph. Routability is
/// tag-based ([`crate::routing::is_routable`]) and independent of styling. `coords` is snapped here to the µdeg
/// grid POIs and the serializer use, so edge lengths and later serialization agree.
pub fn push_routable_way(
    id: i64,
    w: &osmpbf::Way,
    refs: &[i64],
    coords: &[(f64, f64)],
    foot_only: &HashSet<i64>,
    out: &mut Keyed<RoutableWay>,
) {
    // This is the only place tags exist, so the kind and the riding directions are captured here
    // or never.
    let tags: Vec<_> = w.tags().collect();
    for way in routable_runs(&tags, refs, coords, foot_only) {
        out.push(id, way);
    }
}

/// The routable pieces of one way. A segment that touches a node in `foot_only` (a
/// [`Barrier::FootOnly`] barrier) stays out of the graph, so the way splits into the runs of
/// segments around it and no route passes the barrier.
fn routable_runs(
    tags: &[(&str, &str)],
    refs: &[i64],
    coords: &[(f64, f64)],
    foot_only: &HashSet<i64>,
) -> Vec<RoutableWay> {
    let Some(kind) = crate::routing::classify(tags.iter().copied()) else { return Vec::new() };
    let no_ride = crate::routing::no_ride(|key| tags.iter().find(|(k, _)| *k == key).map(|(_, v)| *v));
    if refs.len() < 2 || no_ride == NO_RIDE_FORWARD | NO_RIDE_BACKWARD {
        return Vec::new();
    }
    let open = |i: usize| !foot_only.contains(&refs[i]) && !foot_only.contains(&refs[i + 1]);
    let mut runs = Vec::new();
    let mut i = 0;
    while i + 1 < refs.len() {
        if !open(i) {
            i += 1;
            continue;
        }
        let start = i;
        while i + 1 < refs.len() && open(i) {
            i += 1;
        }
        let coords = coords[start..=i].iter().map(|&(x, y)| (crate::to_udeg(x), crate::to_udeg(y))).collect();
        runs.push(RoutableWay { node_ids: refs[start..=i].to_vec(), coords, kind, no_ride });
    }
    runs
}

/// Whether a node's tags close it to a bike: a [`Barrier::FootOnly`] barrier.
pub fn is_foot_only<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> bool {
    let value = tags.into_iter().find(|(k, _)| *k == "barrier").map(|(_, v)| v);
    access::barrier(value) == Barrier::FootOnly
}

/// Classify one node's tags against the POI table; push a candidate on match.
pub fn push_node_poi<'a, I>(id: i64, tags: I, decimicro_lon: i32, decimicro_lat: i32, out: &mut Keyed<Poi>)
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let tags: Vec<_> = tags.into_iter().collect();
    if let Some(poi::Classification { subtype, name, raw_hours, elevation_m, population }) =
        poi::classify_linked(tags.iter().copied())
    {
        out.push(
            id,
            Poi {
                metadata: obc_formats::obcm::PoiMetadata {
                    source: obc_formats::obcm::SourceId::osm(1, id as u64),
                    approach: None,
                },
                access_nodes: vec![id],
                wikidata: tags.iter().find(|(k, _)| *k == "wikidata").map(|(_, v)| (*v).into()),
                wikipedia: tags.iter().find(|(k, _)| *k == "wikipedia").map(|(_, v)| (*v).into()),
                subtype,
                lon_udeg: crate::to_udeg(to_deg(decimicro_lon)),
                lat_udeg: crate::to_udeg(to_deg(decimicro_lat)),
                name,
                from_node: true,
                hours: raw_hours.and_then(hours::parse),
                elevation_m,
                population,
            },
        );
    }
}

/// Resolve a way's node refs to degree coordinates. `None` if any node is missing, and the caller
/// then drops the way (osmium's `InvalidLocationError`).
pub fn resolve_coords(refs: &[i64], nodes: &HashMap<i64, (i32, i32)>) -> Option<Vec<(f64, f64)>> {
    let mut coords = Vec::with_capacity(refs.len());
    for r in refs {
        let &(dx, dy) = nodes.get(r)?;
        coords.push((to_deg(dx), to_deg(dy)));
    }
    Some(coords)
}

/// Classify a way's place metadata independently of drawing styles.
pub fn push_way_poi(w: &osmpbf::Way, refs: &[i64], coords: &[(f64, f64)], pois: &mut Keyed<Poi>) {
    let tags: HashMap<&str, &str> = w.tags().collect();
    let is_closed = refs.len() >= 2 && refs.first() == refs.last();
    if is_closed || tags.contains_key("wikidata") || tags.contains_key("wikipedia") {
        if let Some(poi::Classification { subtype, name, raw_hours, elevation_m, population }) =
            poi::classify_linked(tags.iter().map(|(&k, &v)| (k, v)))
                .filter(|p| p.subtype != obc_formats::obcm::SUMMIT_SUBTYPE_ID)
        {
            let (cx, cy) = if is_closed { crate::ring_centroid(coords) } else { coords[0] };
            let subtype = if is_closed { subtype } else { 0 };
            pois.push(
                w.id(),
                Poi {
                    metadata: obc_formats::obcm::PoiMetadata {
                        source: obc_formats::obcm::SourceId::osm(2, w.id() as u64),
                        approach: None,
                    },
                    access_nodes: refs.to_vec(),
                    wikidata: tags.get("wikidata").map(|v| (*v).into()),
                    wikipedia: tags.get("wikipedia").map(|v| (*v).into()),
                    subtype,
                    lon_udeg: crate::to_udeg(cx),
                    lat_udeg: crate::to_udeg(cy),
                    name,
                    from_node: false,
                    hours: raw_hours.and_then(hours::parse),
                    elevation_m,
                    population,
                },
            );
        }
    }
}

/// Place metadata on an area relation; summit identity belongs to nodes.
pub fn relation_poi(id: i64, tags: &HashMap<&str, &str>) -> Option<Poi> {
    poi::classify(tags.iter().map(|(&k, &v)| (k, v))).filter(|p| p.subtype != obc_formats::obcm::SUMMIT_SUBTYPE_ID).map(
        |p| Poi {
            metadata: obc_formats::obcm::PoiMetadata {
                source: obc_formats::obcm::SourceId::osm(3, id as u64),
                approach: None,
            },
            access_nodes: Vec::new(),
            wikidata: tags.get("wikidata").map(|v| (*v).into()),
            wikipedia: tags.get("wikipedia").map(|v| (*v).into()),
            subtype: p.subtype,
            lon_udeg: 0,
            lat_udeg: 0,
            name: p.name,
            from_node: false,
            hours: p.raw_hours.and_then(hours::parse),
            elevation_m: p.elevation_m,
            population: p.population,
        },
    )
}

/// An explicit relation link remains usable even when its place type has no table row.
pub fn relation_link(id: i64, tags: &HashMap<&str, &str>) -> poi::LandmarkLink {
    poi::LandmarkLink {
        metadata: obc_formats::obcm::PoiMetadata {
            source: obc_formats::obcm::SourceId::osm(3, id as u64),
            approach: None,
        },
        position: None,
        wikidata: tags.get("wikidata").map(|value| (*value).into()),
        wikipedia: tags.get("wikipedia").map(|value| (*value).into()),
        hours: tags.get("opening_hours").and_then(|value| hours::parse(value)),
    }
}

pub struct Places {
    pub pois: Vec<Poi>,
    pub links: Vec<poi::LandmarkLink>,
    pub ways: Vec<RoutableWay>,
}

/// Read source metadata and approaches without constructing drawing features or a graph.
/// Duplicate objects belong to the first supplied source, and merged outputs return in ID order.
pub fn harvest(
    paths: &[String],
    config: &obc_map_core::config::Config,
    progress: &obc_map_core::progress::Progress,
) -> Result<Places, String> {
    use obc_pbf::area::assemble_multipolygon;
    use obc_pbf::scan::{scan_blobs, Scan};
    use osmpbf::{Element, RelMemberType};

    if paths.is_empty() {
        return Err("no .osm.pbf input given".into());
    }
    struct Relation {
        poi: Option<Poi>,
        ways: Vec<i64>,
    }
    let merging = paths.len() > 1;
    let mut nodes = HashMap::new();
    let mut foot_only = HashSet::new();
    let mut node_pois = Keyed::new(merging);
    let mut relations = Keyed::new(merging);
    let mut links = Keyed::new(merging);
    let (mut seen_relations, mut seen_links) = (HashSet::new(), HashSet::new());
    for (index, path) in paths.iter().enumerate() {
        let mut source_nodes = HashMap::new();
        let mut source_foot_only = HashSet::new();
        let mut source_pois = Keyed::new(merging);
        scan_blobs(path, None, progress, |el| {
            match el {
                Element::Node(n) => {
                    source_nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
                    if is_foot_only(n.tags()) {
                        source_foot_only.insert(n.id());
                    }
                    push_node_poi(n.id(), n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut source_pois);
                }
                Element::DenseNode(n) => {
                    source_nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
                    if is_foot_only(n.tags()) {
                        source_foot_only.insert(n.id());
                    }
                    push_node_poi(n.id(), n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut source_pois);
                }
                Element::Relation(r) => {
                    let tags: HashMap<_, _> = r.tags().collect();
                    if (tags.contains_key("wikidata") || tags.contains_key("wikipedia"))
                        && (!merging || index == 0 || seen_links.insert(r.id()))
                    {
                        links.push(r.id(), relation_link(r.id(), &tags));
                    }
                    if matches!(tags.get("type").copied(), Some("multipolygon" | "boundary")) {
                        let poi = relation_poi(r.id(), &tags);
                        let styled = !tags.contains_key("admin_level") && config.get_style(&tags).is_some();
                        let ways: Vec<_> =
                            r.members().filter(|m| m.member_type == RelMemberType::Way).map(|m| m.member_id).collect();
                        if (styled || poi.is_some())
                            && !ways.is_empty()
                            && (!merging || index == 0 || seen_relations.insert(r.id()))
                        {
                            relations.push(r.id(), Relation { poi, ways });
                        }
                    }
                }
                _ => {}
            }
            Scan::Continue
        })
        .map_err(|e| format!("pass 1: {e}"))?;
        if merging && index == 0 {
            seen_relations.extend(relations.keys().iter().copied());
            seen_links.extend(links.keys().iter().copied());
        }
        if merging {
            source_pois.retain_keys(|id| !nodes.contains_key(&id));
        }
        node_pois.append(source_pois);
        foot_only.extend(source_foot_only.into_iter().filter(|id| !nodes.contains_key(id)));
        for (id, coordinates) in source_nodes {
            nodes.entry(id).or_insert(coordinates);
        }
    }
    if merging {
        node_pois.sort();
        relations.sort();
        links.sort();
    }
    let relations = relations.into_items();
    let needed: HashSet<_> = relations.iter().flat_map(|r| r.ways.iter().copied()).collect();
    let mut member_geom = HashMap::new();
    let mut way_pois = Keyed::new(merging);
    let mut routable = Keyed::new(merging);
    let mut claimed = HashSet::new();
    for path in paths {
        let mut source_claimed = Vec::new();
        scan_blobs(path, None, progress, |el| {
            if let Element::Way(w) = el {
                if !merging || !claimed.contains(&w.id()) {
                    source_claimed.push(w.id());
                    let refs: Vec<_> = w.refs().collect();
                    if let Some(coords) = resolve_coords(&refs, &nodes) {
                        push_routable_way(w.id(), &w, &refs, &coords, &foot_only, &mut routable);
                        push_way_poi(&w, &refs, &coords, &mut way_pois);
                        if needed.contains(&w.id()) {
                            member_geom.insert(w.id(), coords);
                        }
                    }
                }
            }
            Scan::Continue
        })
        .map_err(|e| format!("pass 2: {e}"))?;
        claimed.extend(source_claimed);
    }
    if merging {
        way_pois.sort();
        routable.sort();
    }
    let ways = routable.into_items();
    let mut candidates = node_pois.into_items();
    candidates.extend(way_pois.into_items());
    for relation in relations {
        let Some(mut poi) = relation.poi else { continue };
        let Some(members) = relation.ways.iter().map(|id| member_geom.get(id).cloned()).collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let polygons = assemble_multipolygon(&members);
        if let Some((lon, lat)) = crate::area_center(polygons.iter().map(|p| p.exterior.as_slice())) {
            poi.lon_udeg = crate::to_udeg(lon);
            poi.lat_udeg = crate::to_udeg(lat);
            candidates.push(poi);
        }
    }
    let (mut pois, dropped) = poi::dedupe(candidates);
    poi::resolve_approaches(&mut pois, &ways, &config.routing.profiles);
    let mut landmark_links: Vec<_> =
        pois.iter().filter(|p| p.wikidata.is_some() || p.wikipedia.is_some()).map(poi::LandmarkLink::from).collect();
    landmark_links.extend(links.into_items());
    pois.retain(|p| p.subtype != 0);
    progress.log(poi::format_counts(&pois, dropped));
    Ok(Places { pois, links: landmark_links, ways })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(tags: &[(&str, &str)], foot_only: &[i64]) -> Vec<(Vec<i64>, u8)> {
        let refs: Vec<i64> = (0..5).collect();
        let coords: Vec<_> = refs.iter().map(|&i| (8.0 + i as f64 * 1e-3, 46.0)).collect();
        routable_runs(tags, &refs, &coords, &foot_only.iter().copied().collect())
            .into_iter()
            .map(|way| (way.node_ids, way.no_ride))
            .collect()
    }

    #[test]
    fn one_way_rules_close_a_direction_and_an_unknown_rule_drops_the_way() {
        let road = [("highway", "residential")];
        assert_eq!(runs(&road, &[]), [((0..5).collect(), 0)]);
        assert_eq!(runs(&[road[0], ("oneway", "yes")], &[]), [((0..5).collect(), NO_RIDE_BACKWARD)]);
        assert_eq!(runs(&[road[0], ("oneway", "-1")], &[]), [((0..5).collect(), NO_RIDE_FORWARD)]);
        assert_eq!(runs(&[road[0], ("oneway", "yes"), ("oneway:bicycle", "no")], &[]), [((0..5).collect(), 0)]);
        assert!(runs(&[road[0], ("oneway", "sometimes")], &[]).is_empty());
    }

    #[test]
    fn a_foot_only_barrier_removes_the_segments_that_touch_it() {
        let road = [("highway", "track")];
        assert_eq!(runs(&road, &[2]), [(vec![0, 1], 0), (vec![3, 4], 0)]);
        assert_eq!(runs(&road, &[0]), [(vec![1, 2, 3, 4], 0)]);
        assert_eq!(runs(&road, &[1, 3]), []);
        assert!(is_foot_only([("barrier", "stile")]));
        assert!(!is_foot_only([("barrier", "gate")]));
        assert!(!is_foot_only([("barrier", "something_new")]));
    }
}
