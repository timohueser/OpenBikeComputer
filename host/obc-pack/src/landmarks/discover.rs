//! Landmark candidates from the region's own OSM extract.
//!
//! Every candidate is an explicit `wikidata` tag on a map object. Nothing is matched by name or by
//! coordinate, for the reason a peak article is linked only by an explicit tag: a name and a
//! position cannot prove an item is about that object. So discovery is offline, it is bounded by
//! the extract instead of a bounding box, and every landmark it finds has a map object the rider
//! can be routed to.
//!
//! The list is a superset, never a selection. The compiler applies the exact polygon, the category
//! policy and the excluded classes to the captured entities afterwards.

use super::*;
use osmpbf::{Element, ElementReader};

/// `candidates.json`: the QIDs a capture is pinned to, and the extract they were read from.
#[derive(Debug, Serialize, Deserialize)]
pub struct Candidates {
    pub schema: u32,
    pub osm_sha256: String,
    pub qids: Vec<String>,
}

/// Read every valid `wikidata` tag in `osm`, on a node, a way or a relation alike.
pub fn candidates(osm: &Path) -> Result<Candidates, String> {
    let mut qids = BTreeSet::new();
    let mut take = |tags: &mut dyn Iterator<Item = (&str, &str)>| {
        for (key, value) in tags {
            if key == "wikidata" && is_qid(value) {
                qids.insert(value.to_owned());
            }
        }
    };
    ElementReader::from_path(osm)
        .map_err(|e| e.to_string())?
        .for_each(|element| match element {
            Element::Node(n) => take(&mut n.tags()),
            Element::DenseNode(n) => take(&mut n.tags()),
            Element::Way(w) => take(&mut w.tags()),
            Element::Relation(r) => take(&mut r.tags()),
        })
        .map_err(|e| e.to_string())?;
    Ok(Candidates { schema: 1, osm_sha256: file_digest(osm)?, qids: qids.into_iter().collect() })
}

/// The offline discovery entry point: `landmark-candidates --osm FILE --out FILE`.
pub fn discover(osm: &Path, output: &Path) -> Result<(), String> {
    let candidates = candidates(osm)?;
    fs::write(output, serde_json::to_vec_pretty(&candidates).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The peak fixture: five summit nodes and a summit way, one node with a `wikidata` tag.
    #[test]
    fn only_an_explicit_tag_becomes_a_candidate() {
        let osm = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/peak-discovery.osm.pbf");
        let found = candidates(&osm).unwrap();
        assert_eq!(found.qids, ["Q1"], "the tagged node; the named ones are not matched by name");
        assert_eq!(found.osm_sha256, hash(&fs::read(&osm).unwrap()));
    }
}
