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
    /// The distinct `wikidata` values that are not QIDs. A mistyped tag is an omission, never a
    /// refusal: one of them must not make a region uncapturable.
    pub rejected: Vec<String>,
}

/// Read every valid `wikidata` tag in `osm`, on a node, a way or a relation alike.
///
/// `osm_sha256` is the caller's digest of the same file, so discovery reads it once.
pub fn candidates(osm: &Path, osm_sha256: &str) -> Result<Candidates, String> {
    let (mut qids, mut rejected) = (BTreeSet::new(), BTreeSet::new());
    let mut take = |tags: &mut dyn Iterator<Item = (&str, &str)>| {
        for (key, value) in tags {
            if key != "wikidata" {
                continue;
            }
            if is_qid(value) {
                qids.insert(value.to_owned());
            } else {
                rejected.insert(value.to_owned());
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
    Ok(Candidates {
        schema: 1,
        osm_sha256: osm_sha256.to_owned(),
        qids: qids.into_iter().collect(),
        rejected: rejected.into_iter().collect(),
    })
}

/// The offline discovery entry point: `landmark-candidates --osm FILE --out FILE`.
pub fn discover(osm: &Path, output: &Path) -> Result<(), String> {
    let candidates = candidates(osm, &file_digest(osm)?)?;
    fs::write(output, serde_json::to_vec_pretty(&candidates).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The peak fixture: five summit nodes and a summit way, one node with a `wikidata` tag.
    #[test]
    fn only_an_explicit_tag_becomes_a_candidate() {
        let osm = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/peak-discovery.osm.pbf");
        let digest = file_digest(&osm).unwrap();
        let found = candidates(&osm, &digest).unwrap();
        assert_eq!(found.qids, ["Q1"], "the tagged node; the named ones are not matched by name");
        assert!(found.rejected.is_empty());
        assert_eq!(found.osm_sha256, hash(&fs::read(&osm).unwrap()));
    }

    /// The rule is the capture tool's, character for character. A tag the tool would refuse must
    /// never reach it: the tool stops before its first request, and the stage rewrites the list
    /// from the extract on every run, so an operator cannot edit one out.
    #[test]
    fn a_mistyped_tag_is_an_omission_and_not_a_candidate() {
        for value in ["Q0042", "Q+42", "Q0", "Q", "q42", "Q42 ", "wikidata:Q42", ""] {
            assert!(!is_qid(value), "{value:?} is not a QID");
        }
        for value in ["Q1", "Q42", "Q18446744073709551616"] {
            assert!(is_qid(value), "{value:?} is a QID");
        }
    }
}
