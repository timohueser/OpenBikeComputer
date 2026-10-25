//! Native OSM source preparation, independent of styled map packing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use obc_formats::grid::{cell_square, id_width};
use serde::{Deserialize, Serialize};

pub mod step;

/// All shipped bands divide this source leaf size exactly.
pub const SOURCE_LEAF_LOG2: u32 = 23;

/// The generator in the header of each extract.
const GENERATOR: &str = "obc-data";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LeafId {
    pub i: i64,
    pub j: i64,
}

impl LeafId {
    pub fn square(self) -> (i64, i64, i64, i64) {
        cell_square(SOURCE_LEAF_LOG2, self.i, self.j)
    }

    /// The source square and its reference-complete extraction halo.
    pub fn extract_bbox(self) -> [f64; 4] {
        extract_bbox(self.square())
    }

    /// `<i>-<j>.osm.pbf`, zero-padded.
    pub fn file_name(self) -> String {
        let width = id_width(SOURCE_LEAF_LOG2);
        format!("{:0width$}-{:0width$}.osm.pbf", self.i, self.j)
    }
}

/// Clamp the one-microdegree extraction halo to actual geography.
pub fn extract_bbox((w, s, e, n): (i64, i64, i64, i64)) -> [f64; 4] {
    [
        ((w - 1).max(-180_000_000) as f64) / 1e6,
        ((s - 1).max(-90_000_000) as f64) / 1e6,
        ((e + 1).min(180_000_000) as f64) / 1e6,
        ((n + 1).min(90_000_000) as f64) / 1e6,
    ]
}

#[derive(Debug, Clone)]
pub struct ExtractRequest {
    pub output: String,
    pub bbox: [f64; 4],
}

/// Injectable because CI must prove the hierarchy without downloading or
/// requiring Osmium. Production uses [`OsmiumRunner`].
pub trait ShardRunner: Sync {
    fn check(&self) -> Result<(), String>;
    fn split(&self, input: &Path, output_dir: &Path, requests: &[ExtractRequest]) -> Result<(), String>;
}

pub struct OsmiumRunner {
    binary: PathBuf,
}

impl Default for OsmiumRunner {
    fn default() -> Self {
        Self { binary: std::env::var_os("OBC_OSMIUM").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("osmium")) }
    }
}

impl ShardRunner for OsmiumRunner {
    fn check(&self) -> Result<(), String> {
        let out = Command::new(&self.binary)
            .arg("--version")
            .output()
            .map_err(|e| format!("{} is required for OSM preparation: {e}", self.binary.display()))?;
        if !out.status.success() {
            return Err(format!("{} --version failed with {}", self.binary.display(), out.status));
        }
        Ok(())
    }

    fn split(&self, input: &Path, output_dir: &Path, requests: &[ExtractRequest]) -> Result<(), String> {
        #[derive(Serialize)]
        struct Config<'a> {
            extracts: Vec<ConfigExtract<'a>>,
        }
        #[derive(Serialize)]
        struct ConfigExtract<'a> {
            output: &'a str,
            bbox: [f64; 4],
        }
        std::fs::create_dir_all(output_dir).map_err(|e| format!("{}: {e}", output_dir.display()))?;
        let config =
            Config { extracts: requests.iter().map(|r| ConfigExtract { output: &r.output, bbox: r.bbox }).collect() };
        let config_path = output_dir.join("extracts.json");
        let mut text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
        text.push('\n');
        obc_data::store::write_atomic(&config_path, text.as_bytes())?;
        // `-F pbf`: a planet from the store is an object without a file extension. An extract has
        // no replication header of its input and names no Osmium version, so its bytes change only
        // with its OSM data.
        let status = Command::new(&self.binary)
            .args(["extract", "-F", "pbf", "--config"])
            .arg(&config_path)
            .args(["--directory"])
            .arg(output_dir)
            .args(["--strategy", "smart", "--set-bounds", "--generator", GENERATOR, "--overwrite", "--verbose"])
            .arg(input)
            .status()
            .map_err(|e| format!("run {} extract: {e}", self.binary.display()))?;
        if !status.success() {
            return Err(format!("{} extract failed with {status}", self.binary.display()));
        }
        for request in requests {
            let path = output_dir.join(&request.output);
            if !path.is_file() {
                return Err(format!("{} did not produce {}", self.binary.display(), path.display()));
            }
        }
        Ok(())
    }
}

impl OsmiumRunner {
    /// The first line of `osmium --version`.
    pub fn version(&self) -> Result<String, String> {
        let out = Command::new(&self.binary).arg("--version").output();
        let out = out.map_err(|e| format!("{} --version: {e}", self.binary.display()))?;
        if !out.status.success() {
            return Err(format!("{} --version failed with {}", self.binary.display(), out.status));
        }
        Ok(String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().to_string())
    }
}

/// Write the extract of each of `leaves` from `input` to `<out>/<LeafId::file_name>`. One run of
/// `runner` writes at most two extracts, so its memory stays bounded also for the planet: the
/// rectangle around the leaves is halved along its longer side, and each half is cut from the
/// extract of its parent, until each part is one leaf. A smart extract of a parent holds every
/// object that a leaf in it references, so a leaf has the bytes of a direct extract. The caller
/// creates `out`. `work` holds the halves while they are cut, and is gone at the end.
pub fn split(
    runner: &dyn ShardRunner,
    input: &Path,
    leaves: &BTreeSet<LeafId>,
    work: &Path,
    out: &Path,
) -> Result<(), String> {
    let Some(root) = Rect::around(leaves.iter()) else { return Ok(()) };
    let _ = std::fs::remove_dir_all(work);
    Split { runner, leaves, work, out }.cut(root, input, false)?;
    std::fs::remove_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))
}

struct Split<'a> {
    runner: &'a dyn ShardRunner,
    leaves: &'a BTreeSet<LeafId>,
    work: &'a Path,
    out: &'a Path,
}

impl Split<'_> {
    /// Cut `rect` from `input`, which this call deletes when it `owns` it.
    fn cut(&self, rect: Rect, input: &Path, owns: bool) -> Result<(), String> {
        let parts: Vec<Rect> = match rect.halves() {
            Some(halves) => halves
                .iter()
                .filter_map(|half| Rect::around(self.leaves.iter().filter(|leaf| half.holds(**leaf))))
                .collect(),
            None => vec![rect],
        };
        let dir = self.work.join(rect.name());
        let requests: Vec<ExtractRequest> =
            parts.iter().map(|part| ExtractRequest { output: part.file_name(), bbox: part.extract_bbox() }).collect();
        self.runner.split(input, &dir, &requests)?;
        if owns {
            std::fs::remove_file(input).map_err(|e| format!("{}: {e}", input.display()))?;
        }
        for part in parts {
            let file = dir.join(part.file_name());
            match part.leaf() {
                Some(leaf) => {
                    let path = self.out.join(leaf.file_name());
                    std::fs::rename(&file, &path).map_err(|e| format!("{}: {e}", path.display()))?;
                }
                None => self.cut(part, &file, true)?,
            }
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))
    }
}

/// A rectangle of leaves: rows `i0..i1` and columns `j0..j1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    i0: i64,
    i1: i64,
    j0: i64,
    j1: i64,
}

impl Rect {
    fn around<'a>(leaves: impl Iterator<Item = &'a LeafId>) -> Option<Rect> {
        leaves.fold(None, |rect, leaf| {
            let one = Rect { i0: leaf.i, i1: leaf.i + 1, j0: leaf.j, j1: leaf.j + 1 };
            Some(rect.map_or(one, |rect: Rect| Rect {
                i0: rect.i0.min(one.i0),
                i1: rect.i1.max(one.i1),
                j0: rect.j0.min(one.j0),
                j1: rect.j1.max(one.j1),
            }))
        })
    }

    fn holds(self, leaf: LeafId) -> bool {
        (self.i0..self.i1).contains(&leaf.i) && (self.j0..self.j1).contains(&leaf.j)
    }

    fn leaf(self) -> Option<LeafId> {
        (self.i1 - self.i0 == 1 && self.j1 - self.j0 == 1).then_some(LeafId { i: self.i0, j: self.j0 })
    }

    /// The two halves across the longer side; `None` for one leaf.
    fn halves(self) -> Option<[Rect; 2]> {
        let (rows, cols) = (self.i1 - self.i0, self.j1 - self.j0);
        if cols >= rows && cols > 1 {
            let mid = self.j0 + cols / 2;
            Some([Rect { j1: mid, ..self }, Rect { j0: mid, ..self }])
        } else if rows > 1 {
            let mid = self.i0 + rows / 2;
            Some([Rect { i1: mid, ..self }, Rect { i0: mid, ..self }])
        } else {
            None
        }
    }

    fn extract_bbox(self) -> [f64; 4] {
        let (west, south, _, _) = cell_square(SOURCE_LEAF_LOG2, self.i0, self.j0);
        let (_, _, east, north) = cell_square(SOURCE_LEAF_LOG2, self.i1 - 1, self.j1 - 1);
        extract_bbox((west, south, east, north))
    }

    fn name(self) -> String {
        format!("{}-{}_{}-{}", self.i0, self.i1, self.j0, self.j1)
    }

    fn file_name(self) -> String {
        format!("{}.osm.pbf", self.name())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("obc-osm-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Writes the box of each extract as its bytes, and counts the extracts of each run.
    #[derive(Default)]
    struct FakeRunner {
        runs: Mutex<Vec<usize>>,
    }

    impl ShardRunner for FakeRunner {
        fn check(&self) -> Result<(), String> {
            Ok(())
        }

        fn split(&self, input: &Path, output_dir: &Path, requests: &[ExtractRequest]) -> Result<(), String> {
            assert!(input.is_file(), "{} is read before it is deleted", input.display());
            self.runs.lock().unwrap().push(requests.len());
            std::fs::create_dir_all(output_dir).unwrap();
            for request in requests {
                std::fs::write(output_dir.join(&request.output), format!("{:?}", request.bbox)).unwrap();
            }
            Ok(())
        }
    }

    fn files(dir: &Path) -> BTreeMap<String, String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (path.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(path).unwrap())
            })
            .collect()
    }

    #[test]
    fn a_split_writes_exactly_its_leaves_with_at_most_two_extracts_per_run() {
        let (south, west) = obc_formats::grid::containing_indices(SOURCE_LEAF_LOG2, -90_000_000, -180_000_000);
        let (north, east) = obc_formats::grid::containing_indices(SOURCE_LEAF_LOG2, 90_000_000, 180_000_000);
        let planet: BTreeSet<LeafId> =
            (south..=north).flat_map(|i| (west..=east).map(move |j| LeafId { i, j })).collect();
        let sparse = [LeafId { i: 30, j: 10 }, LeafId { i: 37, j: 32 }, LeafId { i: 38, j: 33 }].into();
        let one = [LeafId { i: 37, j: 32 }].into();
        for leaves in [planet, sparse, one] {
            let temp = Temp::new("split");
            let (input, work, out) = (temp.0.join("input.osm.pbf"), temp.0.join("work"), temp.0.join("out"));
            std::fs::write(&input, "input").unwrap();
            std::fs::create_dir(&out).unwrap();
            let runner = FakeRunner::default();
            split(&runner, &input, &leaves, &work, &out).unwrap();
            let runs = runner.runs.into_inner().unwrap();
            assert!(runs.iter().all(|&extracts| (1..=2).contains(&extracts)), "{runs:?}");
            let cut: BTreeMap<String, String> =
                leaves.iter().map(|leaf| (leaf.file_name(), format!("{:?}", leaf.extract_bbox()))).collect();
            assert_eq!(files(&out), cut, "each leaf is cut with its own box, and no other file is left");
            assert!(!work.exists() && input.is_file(), "the halves are gone; the input stays");
        }
    }

    /// Osmium writes each leaf of a split as a direct extract would, without the replication
    /// header or the Osmium version, and an edit in one leaf changes only that leaf.
    #[test]
    fn osmium_leaves_change_only_with_their_own_osm_data() {
        let runner = OsmiumRunner::default();
        if runner.check().is_err() {
            return;
        }
        let temp = Temp::new("leaves");
        let path = |name: &str| temp.0.join(name);
        let osmium = |args: &[&str]| {
            let status = Command::new(&runner.binary).args(args).current_dir(&temp.0).status().unwrap();
            assert!(status.success(), "osmium {args:?}");
        };
        // One node, way or relation per line around the corner of four leaves at 0°, 0°; leaf
        // (33, 33) is empty. Way 105 crosses from leaf (32, 31) into (32, 32). The forest of
        // relation 201 lies in (31, 31) and (31, 32), with a second ring, way 107, in leaf (30, 33)
        // outside every half. The bicycle route 202 has members in (31, 31), (32, 32) and (30, 33).
        let node = |id: u32, lat: f64, lon: f64| format!(r#"<node id="{id}" version="1" lat="{lat}" lon="{lon}"/>"#);
        let way = |id: u32, nodes: &[u32], tag: &str| {
            let nodes: String = nodes.iter().map(|n| format!(r#"<nd ref="{n}"/>"#)).collect();
            format!(r#"<way id="{id}" version="1">{nodes}{tag}</way>"#)
        };
        let track = r#"<tag k="highway" v="track"/>"#;
        let at =
            [(-1.5, -1.5), (-0.5, -0.5), (-1.5, 0.5), (-0.5, 1.5), (0.5, -1.5), (1.5, -0.5), (0.5, 0.5), (1.5, 1.5)];
        let far = [(-9.5, 9.5), (-9.5, 9.7), (-9.3, 9.6)];
        let mut osm = vec![r#"<?xml version="1.0"?><osm version="0.6">"#.to_string()];
        osm.extend(at.iter().chain(&far).zip(1..).map(|(&(lat, lon), id)| node(id, lat, lon)));
        osm.extend([(101, [1, 2]), (102, [3, 4]), (103, [5, 6]), (104, [7, 8])].map(|(id, n)| way(id, &n, track)));
        osm.push(way(105, &[6, 7], r#"<tag k="highway" v="primary"/>"#));
        osm.extend([way(106, &[1, 2, 3, 1], ""), way(107, &[9, 10, 11, 9], "")]);
        let relation = |id: u32, members: &[(u32, &str)], tags: &str| {
            let members: String = members
                .iter()
                .map(|(way, role)| format!(r#"<member type="way" ref="{way}" role="{role}"/>"#))
                .collect();
            format!(r#"<relation id="{id}" version="1">{members}{tags}</relation>"#)
        };
        let forest = r#"<tag k="type" v="multipolygon"/><tag k="landuse" v="forest"/>"#;
        osm.push(relation(201, &[(106, "outer"), (107, "outer")], forest));
        let route = r#"<tag k="type" v="route"/><tag k="route" v="bicycle"/>"#;
        osm.extend([relation(202, &[(101, ""), (104, ""), (107, "")], route), "</osm>".into()]);
        std::fs::write(path("base.osm"), osm.join("\n")).unwrap();
        let replication = |sequence: &str| format!("--output-header=osmosis_replication_sequence_number={sequence}");
        osmium(&["cat", "base.osm", "-o", "base.osm.pbf", &replication("10")]);
        // Node 8 is only in way 104 of leaf (32, 32).
        let moved =
            r#"<osmChange version="0.6"><modify><node id="8" version="2" lat="1.5" lon="1.6"/></modify></osmChange>"#;
        std::fs::write(path("day.osc"), moved).unwrap();
        osmium(&["apply-changes", "base.osm.pbf", "day.osc", "-o", "updated.osm.pbf", &replication("11")]);

        let leaves: BTreeSet<LeafId> =
            [(31, 31), (31, 32), (32, 31), (32, 32), (33, 33)].map(|(i, j)| LeafId { i, j }).into();
        let cut = |input: &str, name: &str| {
            let out = path(name);
            std::fs::create_dir(&out).unwrap();
            split(&runner, &path(input), &leaves, &path("work"), &out).unwrap();
            out
        };
        let (before, after) = (cut("base.osm.pbf", "before"), cut("updated.osm.pbf", "after"));
        let requests: Vec<ExtractRequest> =
            leaves.iter().map(|leaf| ExtractRequest { output: leaf.file_name(), bbox: leaf.extract_bbox() }).collect();
        runner.split(&path("base.osm.pbf"), &path("direct"), &requests).unwrap();
        let hash = |dir: &Path, leaf: &LeafId| obc_data::store::hash_file(&dir.join(leaf.file_name())).unwrap();
        for leaf in &leaves {
            assert_eq!(hash(&before, leaf), hash(&path("direct"), leaf), "{leaf:?}: split and direct extract");
            assert_eq!(hash(&before, leaf) != hash(&after, leaf), *leaf == LeafId { i: 32, j: 32 }, "{leaf:?}");
            let info = Command::new(&runner.binary).args(["fileinfo", "-j"]).arg(after.join(leaf.file_name())).output();
            let info: serde_json::Value = serde_json::from_slice(&info.unwrap().stdout).unwrap();
            let header = info["header"]["option"].as_object().unwrap();
            assert_eq!(header["generator"], GENERATOR);
            assert!(!header.keys().any(|key| key.starts_with("osmosis_replication")), "{header:?}");
        }
    }

    /// The operator page draws a leaf from its id: builder/web/src/lib/data/leaf.test.ts has these numbers.
    #[test]
    fn a_leaf_square_is_the_one_that_the_operator_page_draws() {
        assert_eq!(LeafId { i: 37, j: 32 }.square(), (0, 41_943_040, 8_388_608, 50_331_648));
    }
}
