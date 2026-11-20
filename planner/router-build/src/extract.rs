//! Reads the OSM objects that a caller chooses from an extract, each with its shape: a node is a
//! point, an open way a line, a closed way or a multipolygon relation an area.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::path::Path;

use osmpbfreader::{OsmId, OsmPbfReader};
use planner_router::shape::P;

use crate::osm::copy_tags;
use crate::source::{Id, Tags};

#[derive(Clone, Debug)]
pub enum Shape {
    Point(P),
    /// The points of the way that the extract holds, two or more.
    Line(Vec<P>),
    Area(Area),
}

/// The rings of an area. Each ring is closed: its last point is its first.
#[derive(Clone, Debug)]
pub struct Area {
    pub outer: Vec<Vec<P>>,
    pub inner: Vec<Vec<P>>,
}

#[derive(Clone, Debug)]
pub struct Object {
    pub id: Id,
    pub tags: Tags<'static>,
    pub shape: Shape,
}

pub struct Extract {
    /// By `id`.
    pub objects: Vec<Object>,
    /// The areas left out: a member way or a node is missing, or a ring does not close.
    pub incomplete: usize,
}

/// A multipolygon relation: its tags and the node IDs of its outer and its inner member ways.
struct Multipolygon {
    id: i64,
    tags: Tags<'static>,
    outer: Vec<i64>,
    inner: Vec<i64>,
}

fn reader(pbf: &Path) -> Result<OsmPbfReader<File>, String> {
    Ok(OsmPbfReader::new(File::open(pbf).map_err(|e| format!("{}: {e}", pbf.display()))?))
}

/// The objects whose tags `wanted` accepts, in three passes over `pbf`: the multipolygon relations,
/// then the ways with the member ways of those relations, then the nodes with the points of
/// those ways. A member with an empty role is outer.
pub fn read(pbf: &Path, wanted: &dyn Fn(&Tags) -> bool) -> Result<Extract, String> {
    let mut relations = Vec::new();
    for relation in reader(pbf)?.par_iter_relations() {
        let relation = relation.map_err(|e| e.to_string())?;
        if !relation.tags.contains("type", "multipolygon") {
            continue;
        }
        let tags = copy_tags(&relation.tags);
        if !wanted(&tags) {
            continue;
        }
        let (mut outer, mut inner) = (Vec::new(), Vec::new());
        for member in &relation.refs {
            if let OsmId::Way(way) = member.member {
                match member.role.as_str() {
                    "outer" | "" => outer.push(way.0),
                    "inner" => inner.push(way.0),
                    _ => {}
                }
            }
        }
        relations.push(Multipolygon { id: relation.id.0, tags, outer, inner });
    }
    let members: HashSet<i64> = relations.iter().flat_map(|r| r.outer.iter().chain(&r.inner)).copied().collect();
    let (mut ways, mut member_nodes) = (Vec::new(), HashMap::new());
    for way in reader(pbf)?.par_iter_ways() {
        let way = way.map_err(|e| e.to_string())?;
        let nodes: Vec<i64> = way.nodes.iter().map(|node| node.0).collect();
        if members.contains(&way.id.0) {
            member_nodes.insert(way.id.0, nodes.clone());
        }
        if !way.tags.is_empty() {
            let tags = copy_tags(&way.tags);
            if wanted(&tags) {
                ways.push((way.id.0, tags, nodes));
            }
        }
    }
    let mut needed: Vec<i64> =
        ways.iter().flat_map(|way| &way.2).chain(member_nodes.values().flatten()).copied().collect();
    needed.sort_unstable();
    needed.dedup();
    let mut points: Vec<Option<P>> = vec![None; needed.len()];
    let mut objects = Vec::new();
    for node in reader(pbf)?.par_iter_nodes() {
        let node = node.map_err(|e| e.to_string())?;
        let udeg = |decimicro: i32| (decimicro as f64 * 1e-7 * 1e6).round() as i32;
        let point = [udeg(node.decimicro_lon), udeg(node.decimicro_lat)];
        if let Ok(at) = needed.binary_search(&node.id.0) {
            points[at] = Some(point);
        }
        if !node.tags.is_empty() {
            let tags = copy_tags(&node.tags);
            if wanted(&tags) {
                objects.push(Object { id: Id::Node(node.id.0), tags, shape: Shape::Point(point) });
            }
        }
    }
    let point = |id: &i64| needed.binary_search(id).ok().and_then(|at| points[at]);
    let ring = |nodes: &[i64]| nodes.iter().map(point).collect::<Option<Vec<P>>>();
    let mut incomplete = 0;
    for (id, tags, nodes) in ways {
        let closed = nodes.len() >= 4 && nodes.first() == nodes.last();
        let shape = if closed {
            ring(&nodes).map(|ring| Shape::Area(Area { outer: vec![ring], inner: Vec::new() }))
        } else {
            let line: Vec<P> = nodes.iter().filter_map(point).collect();
            (line.len() >= 2).then_some(Shape::Line(line))
        };
        match shape {
            Some(shape) => objects.push(Object { id: Id::Way(id), tags, shape }),
            None => incomplete += usize::from(closed),
        }
    }
    for relation in relations {
        let rings = |ways: &[i64]| -> Option<Vec<Vec<P>>> {
            let parts = ways.iter().map(|way| member_nodes.get(way).cloned()).collect::<Option<Vec<_>>>()?;
            rings(parts)?.iter().map(|nodes| ring(nodes)).collect()
        };
        match (rings(&relation.outer), rings(&relation.inner)) {
            (Some(outer), Some(inner)) if !outer.is_empty() => objects.push(Object {
                id: Id::Relation(relation.id),
                tags: relation.tags,
                shape: Shape::Area(Area { outer, inner }),
            }),
            _ => incomplete += 1,
        }
    }
    objects.sort_by_key(|object| object.id);
    Ok(Extract { objects, incomplete })
}

/// The closed rings that the ways `parts` make, joined at shared end nodes; None when a ring does
/// not close.
fn rings(mut parts: Vec<Vec<i64>>) -> Option<Vec<Vec<i64>>> {
    let mut ends: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (i, part) in parts.iter().enumerate() {
        if part.len() < 2 {
            return None;
        }
        ends.entry(part[0]).or_default().push(i);
        ends.entry(*part.last()?).or_default().push(i);
    }
    let mut used = vec![false; parts.len()];
    let mut rings = Vec::new();
    for start in 0..parts.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut ring = std::mem::take(&mut parts[start]);
        while ring.first() != ring.last() {
            let end = *ring.last()?;
            let next = *ends.get(&end)?.iter().find(|&&i| !used[i])?;
            used[next] = true;
            let mut part = std::mem::take(&mut parts[next]);
            if part[0] != end {
                part.reverse();
            }
            ring.extend(part.into_iter().skip(1));
        }
        if ring.len() < 4 {
            return None;
        }
        rings.push(ring);
    }
    Some(rings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use osmpbfreader::fileformat::{Blob, BlobHeader};
    use osmpbfreader::osmformat::{relation::MemberType, Node, PrimitiveBlock, PrimitiveGroup, Relation, Way};
    use protobuf::{EnumOrUnknown, Message};

    type TagList<'a> = &'a [(&'a str, &'a str)];
    /// A relation: its id, its member ways with their roles, and its tags.
    type RelationRow<'a> = (i64, &'a [(i64, &'a str)], TagList<'a>);

    #[derive(Default)]
    struct Strings(Vec<Vec<u8>>);

    impl Strings {
        fn index(&mut self, text: &str) -> u32 {
            let at = self.0.iter().position(|s| s == text.as_bytes()).unwrap_or_else(|| {
                self.0.push(text.as_bytes().to_vec());
                self.0.len() - 1
            });
            at as u32
        }

        fn tags(&mut self, tags: TagList) -> (Vec<u32>, Vec<u32>) {
            tags.iter().map(|(key, value)| (self.index(key), self.index(value))).unzip()
        }
    }

    fn deltas(ids: impl Iterator<Item = i64>) -> Vec<i64> {
        let mut last = 0;
        ids.map(|id| id - std::mem::replace(&mut last, id)).collect()
    }

    /// Writes a PBF of one block with the nodes, the ways and the relations, whose members are ways.
    fn pbf(path: &Path, nodes: &[(i64, P, TagList)], ways: &[(i64, &[i64], TagList)], relations: &[RelationRow]) {
        let mut strings = Strings(vec![Vec::new()]);
        let mut groups = [PrimitiveGroup::new(), PrimitiveGroup::new(), PrimitiveGroup::new()];
        for &(id, point, tags) in nodes {
            let mut node = Node::new();
            (node.id, node.lon, node.lat) = (Some(id), Some(i64::from(point[0]) * 10), Some(i64::from(point[1]) * 10));
            (node.keys, node.vals) = strings.tags(tags);
            groups[0].nodes.push(node);
        }
        for &(id, refs, tags) in ways {
            let mut way = Way::new();
            way.id = Some(id);
            (way.keys, way.vals) = strings.tags(tags);
            way.refs = deltas(refs.iter().copied());
            groups[1].ways.push(way);
        }
        for &(id, members, tags) in relations {
            let mut relation = Relation::new();
            relation.id = Some(id);
            (relation.keys, relation.vals) = strings.tags(tags);
            relation.memids = deltas(members.iter().map(|member| member.0));
            relation.roles_sid = members.iter().map(|member| strings.index(member.1) as i32).collect();
            relation.types = members.iter().map(|_| EnumOrUnknown::new(MemberType::WAY)).collect();
            groups[2].relations.push(relation);
        }
        let mut block = PrimitiveBlock::new();
        block.stringtable.mut_or_insert_default().s = strings.0;
        block.primitivegroup = groups.into();
        let mut blob = Blob::new();
        blob.raw = Some(block.write_to_bytes().unwrap());
        let blob = blob.write_to_bytes().unwrap();
        let mut header = BlobHeader::new();
        (header.type_, header.datasize) = (Some("OSMData".into()), Some(blob.len() as i32));
        let header = header.write_to_bytes().unwrap();
        let mut file = (header.len() as u32).to_be_bytes().to_vec();
        file.extend(header);
        file.extend(blob);
        std::fs::write(path, file).unwrap();
    }

    #[test]
    fn place_ids_name_the_osm_element_and_rings_join_member_ways() {
        let path = std::env::temp_dir().join(format!("obc-extract-{}.osm.pbf", std::process::id()));
        let corners =
            [(1, [0, 0]), (2, [0, 1000]), (3, [1000, 1000]), (4, [1000, 0]), (5, [400, 400]), (6, [600, 600])];
        let mut nodes: Vec<(i64, P, TagList)> = corners.iter().map(|&(id, point)| (id, point, &[][..])).collect();
        nodes.push((7, [500, 500], &[("natural", "peak")]));
        let ways: [(i64, &[i64], TagList); 6] = [
            (10, &[1, 2, 3, 1], &[("amenity", "parking")]),
            (11, &[5, 6], &[("aerialway", "gondola")]),
            (12, &[1, 2, 3], &[]),
            (13, &[3, 4, 1], &[]),
            (14, &[5, 6, 2, 5], &[]),
            (15, &[1, 4], &[("highway", "path")]),
        ];
        let water: TagList = &[("type", "multipolygon"), ("natural", "water")];
        // r21 names a way that the extract does not hold.
        let relations: [RelationRow; 2] =
            [(20, &[(12, "outer"), (13, ""), (14, "inner")], water), (21, &[(12, "outer"), (99, "outer")], water)];
        pbf(&path, &nodes, &ways, &relations);
        let wanted = |tags: &Tags| ["amenity", "aerialway", "natural"].iter().any(|key| tags.get(key).is_some());
        let extract = read(&path, &wanted).unwrap();
        std::fs::remove_file(path).unwrap();
        let ids: Vec<String> = extract.objects.iter().map(|object| object.id.to_string()).collect();
        assert_eq!((ids, extract.incomplete), (vec!["n7".into(), "w10".into(), "w11".into(), "r20".into()], 1));
        let shape = |at: usize| &extract.objects[at].shape;
        assert!(matches!(shape(0), Shape::Point([500, 500])));
        assert!(matches!(shape(1), Shape::Area(area) if area.outer == [vec![[0, 0], [0, 1000], [1000, 1000], [0, 0]]]));
        assert!(matches!(shape(2), Shape::Line(line) if line == &[[400, 400], [600, 600]]));
        let Shape::Area(area) = shape(3) else { panic!("r20 is an area") };
        let square = vec![[0, 0], [0, 1000], [1000, 1000], [1000, 0], [0, 0]];
        let hole = vec![[400, 400], [600, 600], [0, 1000], [400, 400]];
        assert_eq!((&area.outer, &area.inner), (&vec![square], &vec![hole]));
    }
}
