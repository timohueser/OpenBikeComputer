//! The bake runner's acceptance criteria (#898), all of them offline.
//!
//! Nothing here touches the network: extracts come from a [`LocalExtracts`] root
//! holding the repo's tiny fixture `.osm.pbf`, and the artifact-level tests drive an
//! injected [`Packer`] instead of libGEOS — which is what lets a test produce the
//! one artifact the design most needs to prove it rejects (a corrupt one) without
//! having to corrupt a real pack.
//!
//! One test does run the real packer end to end, because the wiring between
//! `obc_pack::pipeline::pack`, the verifier and the tree layout is exactly what a
//! fake would stop testing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use obc_bake::bake::{BakeOptions, Bakery, JobStatus, ObcPacker, Packer, RunSummary};
use obc_bake::presets::StyleDoc;
use obc_bake::regions::Region;
use obc_bake::source::LocalExtracts;
use obc_pack::catalog::CatalogOptions;
use obc_pack::progress::Progress;

const SNAPSHOT: &str = "2026-07-28";

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-bake-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Two regions, both resolving to the repo's tiny fixture extract.
fn regions_toml() -> &'static str {
    "regions = [\n  { id = \"europe/alpha\", name = \"Alpha\" },\n  { id = \"europe/beta/gamma\", name = \"Gamma\" },\n]\n"
}

/// A local extract root with the fixture placed at both regions' paths.
fn extract_root(dir: &Path) -> PathBuf {
    let root = dir.join("extracts");
    std::fs::create_dir_all(root.join("europe/beta")).unwrap();
    let fixture = repo("builder/tests/corpus/data/tiny.osm.pbf");
    std::fs::copy(&fixture, root.join("europe/alpha-latest.osm.pbf")).unwrap();
    std::fs::copy(&fixture, root.join("europe/beta/gamma-latest.osm.pbf")).unwrap();
    root
}

/// A style directory holding the real shipped schema, copied so a test can edit it.
fn presets_dir(dir: &Path) -> PathBuf {
    let out = dir.join("presets-src");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(repo("builder/presets/schema.json"), out.join(obc_bake::presets::SCHEMA_DOC)).unwrap();
    out
}

/// Packs by copying a known-good `.obcm`: a valid artifact with no libGEOS involved.
struct FixturePacker {
    source: PathBuf,
    calls: AtomicUsize,
}

impl FixturePacker {
    fn new() -> Self {
        Self { source: repo("apps/obc-sim/assets/monaco.obcm"), calls: AtomicUsize::new(0) }
    }
}

impl Packer for FixturePacker {
    fn recipe(&self) -> String {
        "fixture-copy".into()
    }

    fn pack(&self, _pbf: &Path, _preset: &StyleDoc, out: &Path, _p: &Progress) -> Result<u64, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::fs::copy(&self.source, out).map_err(|e| e.to_string())
    }
}

/// Packs a good artifact for every region except one, where it writes rubbish that
/// still starts with a plausible-looking magic — the shape a truncated or
/// interrupted pack leaves behind.
struct CorruptingPacker {
    good: FixturePacker,
    corrupt_region: &'static str,
}

impl Packer for CorruptingPacker {
    fn recipe(&self) -> String {
        "fixture-copy-with-one-corrupt".into()
    }

    fn pack(&self, pbf: &Path, preset: &StyleDoc, out: &Path, p: &Progress) -> Result<u64, String> {
        self.good.pack(pbf, preset, out, p)?;
        if out.to_string_lossy().contains(self.corrupt_region) {
            // Keep the header, wreck everything after it: the file still "is" an
            // OBCM by any sniff test, and is unreadable by a real one.
            let mut bytes = std::fs::read(out).map_err(|e| e.to_string())?;
            for b in bytes.iter_mut().skip(64) {
                *b = 0xAB;
            }
            std::fs::write(out, &bytes).map_err(|e| e.to_string())?;
        }
        Ok(std::fs::metadata(out).map_err(|e| e.to_string())?.len())
    }
}

struct Fixture {
    dir: PathBuf,
    regions: Vec<Region>,
    presets: Vec<StyleDoc>,
    source: LocalExtracts,
    tree: PathBuf,
}

fn fixture(name: &str) -> Fixture {
    let dir = scratch(name);
    let source = LocalExtracts::new(extract_root(&dir)).with_snapshot(SNAPSHOT);
    let presets = vec![obc_bake::presets::load_schema(&presets_dir(&dir)).expect("the schema loads")];
    let regions = obc_bake::regions::parse(regions_toml()).expect("region list parses");
    let tree = dir.join("tree");
    Fixture { dir, regions, presets, source, tree }
}

/// The manifest a bake tree generates, at a pinned `generated_at`.
fn manifest_of(tree: &Path) -> obc_pack::catalog::CatalogManifest {
    obc_pack::catalog::generate(
        tree,
        &CatalogOptions { base_url: "https://maps.example/obc".into(), generated_at: "2026-07-29T00:00:00Z".into() },
    )
    .expect("the bake tree generates a manifest")
    .manifest
}

/// Every artifact's `built_at`, paired with the region it belongs to.
///
/// Paired, because `built_at` is stamped per artifact at the moment its own bytes
/// land — two jobs in one run straddling a second boundary differ by design, and a
/// twenty-hour bake's stamps are hours apart. An assertion that compares one
/// region's stamp against another's is asserting a clock coincidence.
fn built_at_by_region(tree: &Path) -> Vec<(String, String)> {
    manifest_of(tree).artifacts.iter().map(|a| (a.region_id.clone(), a.built_at.clone())).collect()
}

impl Fixture {
    fn run(&self, packer: &dyn Packer, force: bool) -> RunSummary {
        Bakery {
            regions: &self.regions,
            presets: &self.presets,
            source: &self.source,
            packer,
            opts: BakeOptions { out: self.tree.clone(), force, fail_fast: false },
        }
        .run(&Progress::silent())
        .expect("the run itself completes; per-job failures are in the summary")
    }

    fn manifest(&self) -> obc_pack::catalog::GeneratedCatalog {
        obc_pack::catalog::generate(
            &self.tree,
            &CatalogOptions {
                base_url: "https://maps.example/obc".into(),
                generated_at: "2026-07-29T00:00:00Z".into(),
            },
        )
        .expect("the bake tree generates a manifest")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_real_pack_lands_in_a_tree_the_catalog_generator_accepts() {
    let f = fixture("real");
    // The real pipeline, exactly as the CLI runs it. `no_land` because land
    // generation is a ~950 MB download, and nothing here is about land.
    let summary = f.run(&ObcPacker { no_land: true, chunk_size: None }, false);
    assert!(summary.ok(), "{}", summary.render());
    assert_eq!(summary.jobs.len(), 2);
    for job in &summary.jobs {
        assert!(matches!(job.status, JobStatus::Baked { .. }), "{job:?}");
    }

    let generated = f.manifest();
    let ids: Vec<&str> = generated.manifest.artifacts.iter().map(|a| a.region_id.as_str()).collect();
    assert_eq!(ids, vec!["europe/alpha", "europe/beta/gamma"], "region ids mirror the curated ids");

    let alpha = &generated.manifest.artifacts[0];
    assert_eq!(alpha.region_name, "Alpha", "the sidecar's name, recorded at bake time");
    assert_eq!(alpha.preset_id, "bikepacking");
    assert_eq!(alpha.preset_version, f.presets[0].version, "recorded, not re-derived");
    assert_eq!(alpha.obcm_version, obc_formats::obcm::VERSION, "read out of the artifact's own header");
    assert_eq!(alpha.source_snapshot, SNAPSHOT);
    assert_eq!(alpha.url, "https://maps.example/obc/regions/europe/alpha/bikepacking.obcm");
    assert!(alpha.bytes > 0);

    // The bake state is local bookkeeping and stays out of the published tree — the
    // generator would refuse a stray file, so this is also what keeps it walkable.
    let state = f.tree.join("regions/europe/alpha").join(obc_bake::bake::state_file_name("bikepacking"));
    assert!(state.is_file(), "the state dotfile is written beside the artifact");
    assert!(generated.warnings.is_empty(), "{:?}", generated.warnings);
}

#[test]
fn a_corrupted_artifact_fails_verification_and_never_reaches_the_manifest() {
    let f = fixture("corrupt");
    let packer = CorruptingPacker { good: FixturePacker::new(), corrupt_region: "gamma" };
    let summary = f.run(&packer, false);

    // Loud: a non-ok summary, a named failure, and the region listed as uncovered.
    assert!(!summary.ok());
    let failures = summary.failures();
    assert_eq!(failures.len(), 1, "{}", summary.render());
    assert_eq!(failures[0].region_id, "europe/beta/gamma");
    assert_eq!(summary.uncovered_regions, vec!["europe/beta/gamma".to_string()]);
    let text = summary.render();
    assert!(text.contains("FAILED JOBS"), "{text}");
    assert!(text.contains("REGIONS WITH NO ARTIFACT"), "{text}");

    // The artifact was never installed — not a bad file in the tree, no file at all.
    let bad_dir = f.tree.join("regions/europe/beta/gamma");
    assert!(!bad_dir.join("bikepacking.obcm").exists(), "a corrupt artifact must not exist under its real name");
    assert!(!bad_dir.join("bikepacking.obcm.json").exists(), "and no sidecar advertising it");
    let leftovers: Vec<_> = std::fs::read_dir(&bad_dir)
        .map(|d| d.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "the temp file is cleaned up too: {leftovers:?}");

    // …and the manifest describes only what survived.
    let generated = f.manifest();
    let ids: Vec<&str> = generated.manifest.artifacts.iter().map(|a| a.region_id.as_str()).collect();
    assert_eq!(ids, vec!["europe/alpha"]);
}

#[test]
fn an_unchanged_rerun_skips_and_a_changed_preset_does_not() {
    let f = fixture("idempotent");
    let packer = FixturePacker::new();

    let first = f.run(&packer, false);
    assert!(first.ok(), "{}", first.render());
    assert_eq!(packer.calls.load(Ordering::SeqCst), 2);
    let artifact = f.tree.join("regions/europe/alpha/bikepacking.obcm");
    let (bytes, digest) = obc_bake::hash::file(&artifact).unwrap();

    // Same extract bytes, same preset bytes, same format version ⇒ no work, and the
    // artifact on disk is untouched.
    let second = f.run(&packer, false);
    assert!(second.ok(), "{}", second.render());
    assert_eq!(packer.calls.load(Ordering::SeqCst), 2, "nothing was re-packed");
    assert!(second.jobs.iter().all(|j| matches!(j.status, JobStatus::Unchanged { .. })), "{}", second.render());
    assert_eq!(obc_bake::hash::file(&artifact).unwrap(), (bytes, digest.clone()));
    assert_eq!(second.total_bytes(), first.total_bytes(), "an unchanged run still reports the published size");

    // --force re-bakes regardless.
    let forced = f.run(&packer, true);
    assert_eq!(packer.calls.load(Ordering::SeqCst), 4);
    assert!(forced.jobs.iter().all(|j| matches!(j.status, JobStatus::Baked { .. })));

    // A restyle changes the preset bytes, so the key changes and everything re-bakes
    // — which is the case a mtime-keyed cache would get wrong, since the artifact is
    // newer than the config it is now out of date with.
    let mut restyled = fixture("idempotent-restyle");
    restyled.run(&packer, false);
    let before = packer.calls.load(Ordering::SeqCst);
    let config_path = restyled.presets[0].path.clone();
    // Bump `_meta.version` from wherever it currently stands. Pinning the literal
    // number couples the test to whichever schema ships today — the edit silently
    // no-opped (and the test failed) when the fixture moved from a v2 preset to the
    // v4 default.
    let mut cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    let bumped = u32::try_from(cfg["_meta"]["version"].as_u64().unwrap() + 1).unwrap();
    cfg["_meta"]["version"] = bumped.into();
    std::fs::write(&config_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    restyled.presets = vec![obc_bake::presets::load_schema(config_path.parent().unwrap()).unwrap()];
    let after_restyle = restyled.run(&packer, false);
    assert_eq!(packer.calls.load(Ordering::SeqCst), before + 2, "a restyle re-bakes its own artifacts");
    assert!(after_restyle.jobs.iter().all(|j| matches!(j.status, JobStatus::Baked { .. })));
    // And the sidecar now records the new preset version, while the tree's preset
    // config carries it as the current one — §3's two numbers, from the same run.
    let generated = restyled.manifest();
    assert_eq!(generated.manifest.presets[0].version, bumped);
    assert!(generated.manifest.artifacts.iter().all(|a| a.preset_version == bumped));
}

#[test]
fn an_artifact_that_rotted_on_disk_is_rebaked_even_though_its_inputs_did_not_change() {
    let f = fixture("rot");
    let packer = FixturePacker::new();
    f.run(&packer, false);
    assert_eq!(packer.calls.load(Ordering::SeqCst), 2);

    // Truncate an artifact behind the runner's back: inputs unchanged, key unchanged,
    // recorded digest no longer matches what is there.
    let artifact = f.tree.join("regions/europe/alpha/bikepacking.obcm");
    let mut bytes = std::fs::read(&artifact).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&artifact, &bytes).unwrap();

    let summary = f.run(&packer, false);
    assert_eq!(packer.calls.load(Ordering::SeqCst), 3, "the damaged artifact re-bakes; the intact one does not");
    assert!(summary.ok(), "{}", summary.render());
}

#[test]
fn renaming_a_region_rewrites_the_sidecar_without_repacking_the_map() {
    let mut f = fixture("rename");
    let packer = FixturePacker::new();
    f.run(&packer, false);
    assert_eq!(f.manifest().manifest.artifacts[0].region_name, "Alpha");
    let packed = packer.calls.load(Ordering::SeqCst);

    // A display name cannot move a byte of the map — but it *is* what the manifest
    // publishes, so a plain skip would leave the catalog advertising the old name
    // forever, and a full re-bake would spend hours writing a string into a sidecar.
    f.regions[0].name = "Alpha (renamed)".into();
    let summary = f.run(&packer, false);
    assert!(summary.ok(), "{}", summary.render());
    assert_eq!(packer.calls.load(Ordering::SeqCst), packed, "nothing was re-packed");
    assert_eq!(f.manifest().manifest.artifacts[0].region_name, "Alpha (renamed)");
    assert!(
        summary.jobs.iter().any(|j| matches!(j.status, JobStatus::SidecarRefreshed { .. })),
        "and the run says so rather than reporting a silent skip: {}",
        summary.render()
    );
}

#[test]
fn a_redated_but_identical_extract_refreshes_the_sidecar_and_packs_nothing() {
    let dir = scratch("redate");
    let root = extract_root(&dir);
    let presets = vec![obc_bake::presets::load_schema(&presets_dir(&dir)).expect("the schema loads")];
    let regions = obc_bake::regions::parse(regions_toml()).expect("region list parses");
    let tree = dir.join("tree");
    let packer = FixturePacker::new();

    let run = |snapshot: &str| {
        Bakery {
            regions: &regions,
            presets: &presets,
            source: &LocalExtracts::new(&root).with_snapshot(snapshot),
            packer: &packer,
            opts: BakeOptions { out: tree.clone(), force: false, fail_fast: false },
        }
        .run(&Progress::silent())
        .expect("run completes")
    };

    run("2026-07-28");
    let packed = packer.calls.load(Ordering::SeqCst);
    let built_at_before = built_at_by_region(&tree);

    // Geofabrik re-publishes the same bytes under a new date — the scenario the
    // module doc promises will not cost a re-pack. The manifest must still tell the
    // truth about the extract's date.
    let summary = run("2026-07-29");
    assert!(summary.ok(), "{}", summary.render());
    assert_eq!(packer.calls.load(Ordering::SeqCst), packed, "a re-dated identical extract must not re-pack");
    assert!(
        summary.jobs.iter().all(|j| matches!(j.status, JobStatus::SidecarRefreshed { .. })),
        "{}",
        summary.render()
    );

    for artifact in &manifest_of(&tree).artifacts {
        assert_eq!(artifact.source_snapshot, "2026-07-29", "the published date follows the extract");
    }
    assert_eq!(
        built_at_by_region(&tree),
        built_at_before,
        "built_at describes when each region's bytes were packed, and they were not re-packed"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_extract_fails_the_whole_region_loudly() {
    let f = fixture("missing-extract");
    std::fs::remove_file(f.dir.join("extracts/europe/alpha-latest.osm.pbf")).unwrap();
    let summary = f.run(&FixturePacker::new(), false);

    assert!(!summary.ok());
    assert_eq!(summary.uncovered_regions, vec!["europe/alpha".to_string()]);
    let text = summary.render();
    assert!(text.contains("extract:"), "the failure says the extract was the problem: {text}");
    assert!(text.contains("REGIONS WITH NO ARTIFACT"), "{text}");
}
