//! Place metadata and topology harvested independently of drawing and graph algorithms.

use crate::hours;
use crate::metadata::{self as poi, Poi};
use crate::routing::RoutableWay;
use obc_pbf::bbox::to_deg;
use obc_pbf::scan::Keyed;
use std::collections::HashMap;

/// Capture a routable way's node-id sequence and µdeg coords for the nav graph. Routability is
/// tag-based ([`crate::routing::is_routable`]) and independent of styling. `coords` is snapped here to the µdeg
/// grid POIs and the serializer use, so edge lengths and later serialization agree.
pub fn push_routable_way(id: i64, w: &osmpbf::Way, refs: &[i64], coords: &[(f64, f64)], out: &mut Keyed<RoutableWay>) {
    if refs.len() < 2 {
        return;
    }
    // Classify once (routability plus the way-kind byte). This is the only place tags exist, so
    // the kind is captured here or never.
    let Some(kind) = crate::routing::classify(w.tags()) else { return };
    let coords_udeg = coords.iter().map(|&(x, y)| (crate::to_udeg(x), crate::to_udeg(y))).collect();
    out.push(id, RoutableWay { node_ids: refs.to_vec(), coords: coords_udeg, kind });
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

/// The decimicro `(lon, lat)` of every node of a source, backed by a `Vec` sorted by id: 16 flat
/// bytes per node and a binary search, because this store is the memory floor of a large ingest.
/// A sorted PBF pushes ids in order, so [`NodeCoords::freeze`] sorts nothing there. Lookups on an
/// unfrozen store would silently miss, so freezing is the type's one rule.
#[derive(Default)]
pub struct NodeCoords(Vec<(i64, (i32, i32))>);

impl NodeCoords {
    #[inline]
    pub fn push(&mut self, id: i64, decimicro_lon: i32, decimicro_lat: i32) {
        self.0.push((id, (decimicro_lon, decimicro_lat)));
    }

    /// End the fill phase. On a duplicate id the first location wins.
    pub fn freeze(&mut self) {
        if !self.0.is_sorted_by_key(|n| n.0) {
            self.0.sort_by_key(|n| n.0);
        }
        self.0.dedup_by_key(|n| n.0);
        self.0.shrink_to_fit();
    }

    #[inline]
    pub fn get(&self, id: i64) -> Option<(i32, i32)> {
        self.0.binary_search_by_key(&id, |n| n.0).ok().map(|i| self.0[i].1)
    }

    pub fn contains(&self, id: i64) -> bool {
        self.get(id).is_some()
    }

    /// Add a later frozen source's nodes that this one does not have, so the first source wins.
    pub fn merge(&mut self, later: NodeCoords) {
        if self.0.is_empty() {
            self.0 = later.0;
            return;
        }
        let earlier = std::mem::take(&mut self.0);
        let mut merged = Vec::with_capacity(earlier.len() + later.0.len());
        let (mut a, mut b) = (earlier.into_iter().peekable(), later.0.into_iter().peekable());
        while let (Some(x), Some(y)) = (a.peek(), b.peek()) {
            match x.0.cmp(&y.0) {
                std::cmp::Ordering::Less => merged.extend(a.next()),
                std::cmp::Ordering::Equal => _ = b.next(),
                std::cmp::Ordering::Greater => merged.extend(b.next()),
            }
        }
        merged.extend(a.chain(b));
        self.0 = merged;
    }
}

/// Resolve a way's node refs to degree coordinates. `None` if any node is missing, and the caller
/// then drops the way (osmium's `InvalidLocationError`).
pub fn resolve_coords(refs: &[i64], nodes: &NodeCoords) -> Option<Vec<(f64, f64)>> {
    let mut coords = Vec::with_capacity(refs.len());
    for &r in refs {
        let (dx, dy) = nodes.get(r)?;
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
    use std::collections::HashSet;

    if paths.is_empty() {
        return Err("no .osm.pbf input given".into());
    }
    struct Relation {
        poi: Option<Poi>,
        ways: Vec<i64>,
    }
    let merging = paths.len() > 1;
    let mut nodes = NodeCoords::default();
    let mut node_pois = Keyed::new(merging);
    let mut relations = Keyed::new(merging);
    let mut links = Keyed::new(merging);
    let (mut seen_relations, mut seen_links) = (HashSet::new(), HashSet::new());
    for (index, path) in paths.iter().enumerate() {
        let mut source_nodes = NodeCoords::default();
        let mut source_pois = Keyed::new(merging);
        scan_blobs(path, None, progress, |el| {
            match el {
                Element::Node(n) => {
                    source_nodes.push(n.id(), n.decimicro_lon(), n.decimicro_lat());
                    push_node_poi(n.id(), n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut source_pois);
                }
                Element::DenseNode(n) => {
                    source_nodes.push(n.id(), n.decimicro_lon(), n.decimicro_lat());
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
            source_pois.retain_keys(|id| !nodes.contains(id));
        }
        node_pois.append(source_pois);
        source_nodes.freeze();
        nodes.merge(source_nodes);
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
                        push_routable_way(w.id(), &w, &refs, &coords, &mut routable);
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
    use super::NodeCoords;

    /// An unsorted source still resolves, and on a duplicate id the first location wins, within a
    /// source and across a merge.
    #[test]
    fn node_coords_first_location_wins() {
        let mut first = NodeCoords::default();
        for (id, lon) in [(5, 50), (1, 10), (5, 51), (3, 30)] {
            first.push(id, lon, 0);
        }
        first.freeze();
        let mut later = NodeCoords::default();
        for (id, lon) in [(2, 20), (3, 31), (9, 90)] {
            later.push(id, lon, 0);
        }
        later.freeze();
        first.merge(later);
        let lons: Vec<_> = (0..10).map(|id| first.get(id).map(|c| c.0)).collect();
        assert_eq!(lons, [None, Some(10), Some(20), Some(30), None, Some(50), None, None, None, Some(90)]);
    }
}
