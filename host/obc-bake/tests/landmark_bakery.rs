//! The landmark stage's acceptance criteria, all of them offline.
//!
//! The `.poly` comes from a [`LocalExtracts`] root and the capture is a fake that writes an empty
//! but well-formed source directory, so the compiler that runs is the real one and no test ever
//! reaches Wikidata.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use obc_bake::landmarks::{LandmarkBakeOptions, LandmarkBakery, LandmarkCapture, LandmarkStatus};
use obc_bake::regions::Region;
use obc_bake::source::LocalExtracts;
use obc_pack::progress::Progress;

fn scratch(name: &str) -> PathBuf {
    let dir = obcm_testkit::scratch::scratch_dir("obc-bake-landmarks", name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn region() -> Region {
    Region { id: "europe/testland".into(), name: "Testland".into() }
}

/// A `.poly` for the region, as Geofabrik serves it beside the extract.
fn write_poly(root: &Path, w: f64, s: f64, e: f64, n: f64) {
    let path = root.join("europe_testland.poly");
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        &path,
        format!("testland\n1\n   {w} {s}\n   {e} {s}\n   {e} {n}\n   {w} {n}\n   {w} {s}\nEND\nEND\n"),
    )
    .unwrap();
}

/// A capture that writes what the tool would write for a boundary with nothing in it, and records
/// every boundary it was handed.
#[derive(Default)]
struct FakeCapture {
    calls: Mutex<Vec<String>>,
}

impl LandmarkCapture for FakeCapture {
    fn describe(&self) -> String {
        "a fake capture".into()
    }

    fn capture(&self, boundary: &Path, policy: &Path, out: &Path, _p: &Progress) -> Result<(), String> {
        let boundary_bytes = std::fs::read(boundary).unwrap();
        self.calls.lock().unwrap().push(String::from_utf8(boundary_bytes.clone()).unwrap());
        let sha = |bytes: &[u8]| -> String {
            use sha2::{Digest, Sha256};
            Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
        };
        std::fs::create_dir_all(out).unwrap();
        std::fs::write(
            out.join("recipe.json"),
            serde_json::json!({
                "schema": 1,
                "boundary_sha256": sha(&boundary_bytes),
                "policy_sha256": sha(&std::fs::read(policy).unwrap()),
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            out.join("manifest.json"),
            serde_json::json!({
                "schema": 1,
                "sources": [],
                "places": [],
                "coverage": { "country_complete": true },
            })
            .to_string(),
        )
        .unwrap();
        Ok(())
    }
}

struct Fixture {
    dir: PathBuf,
    extracts: PathBuf,
    tree: PathBuf,
    cache: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let dir = scratch(name);
        let fixture = Fixture { extracts: dir.join("extracts"), tree: dir.join("tree"), cache: dir.join("cache"), dir };
        write_poly(&fixture.extracts, 7.0, 47.0, 7.5, 47.5);
        fixture
    }

    fn run(
        &self,
        capture: &dyn LandmarkCapture,
        force: bool,
        no_capture: bool,
    ) -> obc_bake::landmarks::LandmarkRunSummary {
        let regions = [region()];
        LandmarkBakery {
            regions: &regions,
            source: &LocalExtracts::new(&self.extracts),
            capture,
            opts: LandmarkBakeOptions { out: self.tree.clone(), cache: self.cache.clone(), force, no_capture },
        }
        .run(&Progress::silent())
        .expect("the stage runs")
    }

    fn artifact(&self) -> PathBuf {
        self.tree.join("landmarks/europe/testland")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn the_regions_own_polygon_is_the_capture_boundary_and_the_artifact_lands_in_the_tree() {
    let fixture = Fixture::new("tree-layout");
    let capture = FakeCapture::default();
    let summary = fixture.run(&capture, false, false);

    let outcome = &summary.regions[0];
    assert_eq!(outcome.status, LandmarkStatus::Captured, "{}", summary.render());
    assert!(summary.ok(), "{}", summary.render());

    // The boundary is the `.poly`'s own corners, not a box someone typed.
    let calls = capture.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let boundary: serde_json::Value = serde_json::from_str(&calls[0]).unwrap();
    assert_eq!(boundary["type"], "MultiPolygon");
    let ring = boundary["coordinates"][0][0].as_array().unwrap();
    assert_eq!(ring.len(), 5, "a closed box: {ring:?}");
    let corners: Vec<(f64, f64)> = ring.iter().map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap())).collect();
    for want in [(7.0, 47.0), (7.5, 47.0), (7.5, 47.5), (7.0, 47.5)] {
        assert!(corners.contains(&want), "{want:?} missing from {corners:?}");
    }

    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture.artifact().join("landmarks.json")).unwrap()).unwrap();
    assert_eq!(doc["region_id"], "europe/testland");
    assert_eq!(doc["languages"], serde_json::json!(["en", "de", "fr", "es"]), "the shared UI language set");
    assert!(fixture.artifact().join("content.json").is_file());
    // The staging directory the compiler needed is not left beside the artifact.
    assert!(!fixture.tree.join("landmarks/europe/testland.part").exists());
}

#[test]
fn an_unchanged_capture_is_neither_re_fetched_nor_re_compiled() {
    let fixture = Fixture::new("unchanged");
    let capture = FakeCapture::default();
    fixture.run(&capture, false, false);

    let again = fixture.run(&capture, false, false);
    assert_eq!(again.regions[0].status, LandmarkStatus::Unchanged, "{}", again.render());
    assert_eq!(capture.calls.lock().unwrap().len(), 1, "a current capture is not fetched again");

    // A damaged artifact is compiled again from the same capture, still without the network.
    std::fs::write(fixture.artifact().join("content.json"), "{}").unwrap();
    let repaired = fixture.run(&capture, false, false);
    assert_eq!(repaired.regions[0].status, LandmarkStatus::Compiled, "{}", repaired.render());
    assert_eq!(capture.calls.lock().unwrap().len(), 1);
}

#[test]
fn a_capture_of_a_different_boundary_is_never_compiled() {
    let fixture = Fixture::new("moved-boundary");
    let capture = FakeCapture::default();
    fixture.run(&capture, false, false);

    // The region's border moves. The capture directory keeps its name, and its contents are now
    // about other ground.
    write_poly(&fixture.extracts, 7.0, 47.0, 8.0, 47.5);
    let refused = fixture.run(&capture, false, true);
    assert_eq!(refused.regions[0].status, LandmarkStatus::CaptureMissing, "{}", refused.render());
    assert!(!refused.ok(), "a region without a current capture is not a finished run");
    assert_eq!(capture.calls.lock().unwrap().len(), 1, "--no-capture never goes to the network");

    let recaptured = fixture.run(&capture, false, false);
    assert_eq!(recaptured.regions[0].status, LandmarkStatus::Captured, "{}", recaptured.render());
    assert_eq!(capture.calls.lock().unwrap().len(), 2);
}
