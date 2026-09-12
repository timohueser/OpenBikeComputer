//! Publication and retention contracts through the real cycle and a filesystem store.
//! The source supplies deterministic cells; provider decoding is covered by captured-source suites.

mod support;

use std::collections::BTreeSet;

use obc_formats::precip4::INTENSITY_MAX;
use obc_wx_bake::canonical::{run_cycle, CycleTimes, Lattice, CANONICAL};
use obc_wx_bake::fetch::{FixtureUpstream, Upstream};
use obc_wx_bake::geometry::GridGeometry;
use obc_wx_bake::publish::DirStore;
use obc_wx_bake::source::{gfs, Adapter, Attribution, BakedFrame, BakedSource, SourceClass};
use obc_wx_bake::{manifest_v2, timefmt};
use support::published_tree;

fn ts(text: &str) -> i64 {
    timefmt::parse_rfc3339(text).expect("test timestamp")
}

fn publication_lattice() -> Lattice {
    Lattice {
        south_lat_udeg: 45_680_000,
        west_lon_udeg: 1_460_000,
        width: 64,
        height: 48,
        shard_width: 32,
        shard_height: 24,
        tile_edge: 64,
        ..CANONICAL
    }
}

struct PublicationSource(Lattice);

impl Adapter for PublicationSource {
    fn id(&self) -> &'static str {
        gfs::ID
    }

    fn bake(&self, _upstream: &mut dyn Upstream, now: i64, _warnings: &mut Vec<String>) -> Result<BakedSource, String> {
        let lattice = self.0;
        let geometry = GridGeometry {
            south_lat_udeg: lattice.south_lat_udeg,
            west_lon_udeg: lattice.west_lon_udeg,
            cell_lat_udeg: lattice.cell_udeg,
            cell_lon_udeg: lattice.cell_udeg,
            width: lattice.width,
            height: lattice.height,
            cell_size_m: lattice.cell_size_m,
            tile_edge: lattice.tile_edge,
            entries_per_page: lattice.entries_per_page,
        };
        geometry.validate()?;
        let times = CycleTimes::anchored_at(now);
        let frames = times
            .offsets_min()
            .enumerate()
            .map(|(frame, offset_min)| BakedFrame {
                offset_min,
                valid_at: times.valid_at(offset_min),
                class: SourceClass::Forecast,
                cells: (0..geometry.cells())
                    .map(|index| {
                        let row = index / geometry.width as usize;
                        let col = index % geometry.width as usize;
                        (1 + (row + col + frame) % usize::from(INTENSITY_MAX)) as u8
                    })
                    .collect(),
            })
            .collect();
        Ok(BakedSource {
            id: self.id(),
            geometry,
            reference_time: times.reference_time,
            attribution: Attribution { text: "Publication test source", url: "https://example.invalid" },
            frames,
            motion_history: Vec::new(),
        })
    }
}

/// The objects on disk match the manifest's current generation and two previous generations.
#[test]
fn the_tree_holds_exactly_the_generations_the_published_manifest_names() {
    let lattice = publication_lattice();
    let source = PublicationSource(lattice);
    let adapters: [&dyn Adapter; 1] = [&source];
    let dir = std::env::temp_dir().join(format!("obc-wx-sweep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = DirStore::new(&dir);

    // The generations present in the tree, read from the object keys themselves.
    let generations_on_disk = |dir: &std::path::Path| -> BTreeSet<String> {
        published_tree(dir)
            .keys()
            .filter(|key| key.ends_with(".obcg"))
            .filter_map(|key| key.split('/').nth(2).map(str::to_string))
            .collect()
    };

    let mut swept = Vec::new();
    let mut chains = Vec::new();
    for step in 0..5 {
        let now = ts("2026-08-09T14:30:00Z") + step * 900;
        let mut upstream = FixtureUpstream::default();
        let report = run_cycle(&lattice, &adapters, &mut upstream, &mut store, now, 2, false).expect("publishes");
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        let raw = std::fs::read(dir.join(manifest_v2::MANIFEST_KEY)).expect("the manifest");
        let document = manifest_v2::from_json(&raw).expect("v2");
        let named: BTreeSet<String> =
            std::iter::once(document.generation.clone()).chain(document.previous_generations.iter().cloned()).collect();
        assert_eq!(
            generations_on_disk(&dir),
            named,
            "step {step}: the tree and the manifest disagree about which generations exist"
        );
        swept.push((report.swept.generations.clone(), report.swept.deleted_objects > 0));
        chains.push((document.generation, document.previous_generations));
    }

    // The chain the sweep reads its delete set from: current plus exactly two, newest first.
    assert_eq!(
        chains,
        vec![
            ("20260809T1430Z".to_string(), vec![]),
            ("20260809T1445Z".to_string(), vec!["20260809T1430Z".to_string()]),
            ("20260809T1500Z".to_string(), vec!["20260809T1445Z".to_string(), "20260809T1430Z".to_string()]),
            ("20260809T1515Z".to_string(), vec!["20260809T1500Z".to_string(), "20260809T1445Z".to_string()]),
            ("20260809T1530Z".to_string(), vec!["20260809T1515Z".to_string(), "20260809T1500Z".to_string()]),
        ],
        "current plus exactly two, newest first"
    );

    // Nothing to retire until a fourth generation exists; from then on, exactly one per cycle, and
    // it is the one that just fell off the chain.
    assert_eq!(
        swept,
        vec![
            (vec![], false),
            (vec![], false),
            (vec![], false),
            (vec!["20260809T1430Z".to_string()], true),
            (vec!["20260809T1445Z".to_string()], true),
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A sweep that cannot delete must not turn a good publish into a failed cycle: the manifest is
/// already in place, the objects it no longer names are unreferenced, and the bucket's 1-day
/// lifecycle rule is what collects the leak. The cycle reports a warning and succeeds.
#[test]
fn a_store_that_refuses_to_delete_still_publishes_a_good_cycle() {
    use obc_wx_bake::publish::{Deleted, ObjectStore, PlannedObject};

    /// A directory store with its delete wired to fail — everything else is the real one.
    struct NoDelete(DirStore);
    impl ObjectStore for NoDelete {
        fn describe(&self) -> String {
            self.0.describe()
        }
        fn put(&mut self, object: &PlannedObject) -> Result<(), String> {
            self.0.put(object)
        }
        fn head(&mut self, key: &str) -> Result<Option<u64>, String> {
            self.0.head(key)
        }
        fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
            self.0.get(key)
        }
        fn delete(&mut self, _key: &str) -> Result<Deleted, String> {
            Err("503 SlowDown".to_string())
        }
    }

    let lattice = publication_lattice();
    let source = PublicationSource(lattice);
    let adapters: [&dyn Adapter; 1] = [&source];
    let dir = std::env::temp_dir().join(format!("obc-wx-sweep-fails-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = NoDelete(DirStore::new(&dir));

    let mut last = None;
    for step in 0..4 {
        let now = ts("2026-08-09T14:30:00Z") + step * 900;
        let mut upstream = FixtureUpstream::default();
        last = Some(
            run_cycle(&lattice, &adapters, &mut upstream, &mut store, now, 2, false)
                .expect("a sweep failure is not a cycle failure"),
        );
    }
    let report = last.expect("four cycles");
    assert_eq!(report.swept.generations, vec!["20260809T1430Z"], "it still reports what it tried to retire");
    assert_eq!(report.swept.deleted_objects, 0);
    assert_eq!(report.warnings.len(), 1, "one warning for the generation, not one per key");
    assert!(report.warnings[0].contains("retention sweep"), "{}", report.warnings[0]);
    // Both public warning fields retain the sweep warning.
    assert_eq!(report.swept.warnings, report.warnings);
    // …and the manifest is the one this cycle published, byte for byte the good outcome.
    let raw = std::fs::read(dir.join(manifest_v2::MANIFEST_KEY)).expect("the manifest");
    assert_eq!(manifest_v2::from_json(&raw).expect("v2").generation, "20260809T1515Z");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An older cycle cannot overwrite a newer manifest or change its object tree.
#[test]
fn a_cycle_older_than_the_published_manifest_refuses_to_publish() {
    let lattice = publication_lattice();
    let source = PublicationSource(lattice);
    let adapters: [&dyn Adapter; 1] = [&source];
    let dir = std::env::temp_dir().join(format!("obc-wx-backwards-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = DirStore::new(&dir);

    let now = ts("2026-08-09T15:00:00Z");
    run_cycle(&lattice, &adapters, &mut FixtureUpstream::default(), &mut store, now, 2, false).expect("the good cycle");
    let before = published_tree(&dir);

    // A stalled racer, or a clock that stepped back one cadence step.
    let error = run_cycle(&lattice, &adapters, &mut FixtureUpstream::default(), &mut store, now - 900, 2, false)
        .expect_err("a manifest that goes backwards must not be published");
    assert!(error.contains("refusing to publish a manifest that goes backwards"), "{error}");
    assert!(error.contains("20260809T1500Z") && error.contains("20260809T1445Z"), "names both: {error}");
    assert_eq!(published_tree(&dir), before, "the refused cycle wrote nothing at all");

    // Re-baking the *same* reference time is the idempotent republish the design rests on, and
    // stays allowed — equality is not going backwards.
    run_cycle(&lattice, &adapters, &mut FixtureUpstream::default(), &mut store, now, 2, false)
        .expect("a re-bake is fine");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Tear the fourth manifest, when a generation is due for retirement. A failed readback must
/// prevent every delete and leave all four generations on disk.
#[test]
fn a_manifest_that_does_not_read_back_stops_the_sweep() {
    use obc_wx_bake::publish::{Deleted, ObjectStore, PlannedObject};

    /// A directory store that can be told to corrupt the manifest the instant it is written — the
    /// shape of a torn body, applied to the one key that matters.
    struct TearsTheManifest {
        inner: DirStore,
        tear: bool,
    }
    impl ObjectStore for TearsTheManifest {
        fn describe(&self) -> String {
            self.inner.describe()
        }
        fn put(&mut self, object: &PlannedObject) -> Result<(), String> {
            self.inner.put(object)?;
            if self.tear && object.key == manifest_v2::MANIFEST_KEY {
                let half = object.bytes.len() / 2;
                self.inner.put(&PlannedObject { bytes: object.bytes[..half].to_vec(), ..object.clone() })?;
            }
            Ok(())
        }
        fn head(&mut self, key: &str) -> Result<Option<u64>, String> {
            self.inner.head(key)
        }
        fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
            self.inner.get(key)
        }
        fn delete(&mut self, _key: &str) -> Result<Deleted, String> {
            panic!("the sweep must not run against a manifest that did not read back");
        }
    }

    let lattice = publication_lattice();
    let source = PublicationSource(lattice);
    let adapters: [&dyn Adapter; 1] = [&source];
    let dir = std::env::temp_dir().join(format!("obc-wx-torn-put-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = TearsTheManifest { inner: DirStore::new(&dir), tear: false };

    // Three clean cycles fill the chain up. Nothing has fallen off it yet, so `delete` is never
    // reached and the tripwire above stays quiet on its own merits rather than on the tear's.
    for step in 0..3 {
        let now = ts("2026-08-09T14:30:00Z") + step * 900;
        let report = run_cycle(&lattice, &adapters, &mut FixtureUpstream::default(), &mut store, now, 2, false)
            .expect("a clean cycle publishes");
        assert!(report.swept.generations.is_empty(), "nothing is off the chain until the fourth cycle");
    }

    // The fourth would retire 14:30 — if it ever got that far.
    store.tear = true;
    let error = run_cycle(
        &lattice,
        &adapters,
        &mut FixtureUpstream::default(),
        &mut store,
        ts("2026-08-09T15:15:00Z"),
        2,
        false,
    )
    .expect_err("a manifest that does not read back fails the cycle");
    assert!(error.contains("refusing to sweep"), "{error}");

    // …and the generation it was about to retire is still there, along with the other three.
    let generations: BTreeSet<String> = published_tree(&dir)
        .keys()
        .filter(|key| key.ends_with(".obcg"))
        .filter_map(|key| key.split('/').nth(2).map(str::to_string))
        .collect();
    assert_eq!(
        generations,
        BTreeSet::from([
            "20260809T1430Z".to_string(),
            "20260809T1445Z".to_string(),
            "20260809T1500Z".to_string(),
            "20260809T1515Z".to_string(),
        ]),
        "a failed readback must leave every generation standing, including the one due for retirement"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
