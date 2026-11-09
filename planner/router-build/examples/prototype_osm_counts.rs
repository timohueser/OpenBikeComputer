//! PROTOTYPE (never merges): counts the OSM data that the routing import keeps, without keeping it.
//! Usage: prototype_osm_counts PBF
use osmpbfreader::{OsmObj, OsmPbfReader};
use std::fs::File;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let (mut ways, mut refs, mut pairs, mut tag_bytes, mut relations, mut members) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut ids: Vec<i64> = Vec::new();
    let mut reader = OsmPbfReader::new(File::open(&path).unwrap());
    for item in reader.iter() {
        match item.unwrap() {
            OsmObj::Way(way) if way.nodes.len() >= 2 => {
                if !way.tags.contains_key("highway") && !way.tags.contains("route", "ferry") {
                    continue;
                }
                ways += 1;
                refs += way.nodes.len() as u64;
                pairs += way.tags.len() as u64;
                tag_bytes += way.tags.iter().map(|(k, v)| (k.len() + v.len()) as u64).sum::<u64>();
                ids.extend(way.nodes.iter().map(|n| n.0));
            }
            OsmObj::Relation(r) => {
                relations += 1;
                members += r.refs.len() as u64;
            }
            _ => {}
        }
    }
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    println!(
        "highway ways {ways}, node refs {refs} ({before}), unique nodes {}, tag pairs {pairs}, tag bytes {tag_bytes}, relations {relations}, members {members}",
        ids.len()
    );
    let (mut tagged, mut node_pairs) = (0u64, 0u64);
    let mut reader = OsmPbfReader::new(File::open(&path).unwrap());
    for item in reader.iter() {
        if let OsmObj::Node(node) = item.unwrap() {
            if !node.tags.is_empty() && ids.binary_search(&node.id.0).is_ok() {
                tagged += 1;
                node_pairs += node.tags.len() as u64;
            }
        }
    }
    println!("tagged highway nodes {tagged}, node tag pairs {node_pairs}");
}
