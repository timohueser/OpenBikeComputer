//! Volume-set mounting (`OBCA_Spec.md` §5): the reader half of §5.3's validation, §5.4's
//! "no partial mount", §5.1's role-blind dispatch, and §5.6's mount-time empty-LOD cache.
//!
//! The renderer-facing half — a hand-split set drawing pixel-identically to the monolith it was
//! split from — lives in `obc-render/tests/volume_set_diff.rs`, where the render harness is.

use std::cell::Cell;

use obc_formats::io::{ByteSource, Error as IoError};
use obc_formats::obcs::{self, ManifestError, Role};
use obc_reader::{
    BBox, FullSetShards, MapCache, MapTables, MountError, MountedSet, SetShards, ShardTables, SliceSource,
};
use obcm_testkit::set::{build_set, empty_lod, matched_pair, quadrants, ShardSpec};
use obcm_testkit::{pack_line, seal, LodSpec, Style};

/// (min_lon, min_lat, max_lon, max_lat) — 4000 µdeg square, so its quadrants and their midpoints
/// are exact and the four-way split is lossless.
const ASSEMBLY: (i32, i32, i32, i32) = (0, 0, 4000, 4000);
const COARSE_MPP: f32 = f32::INFINITY;
const FINE_MPP: f32 = 4.0;
const STYLES: &[Style] = &[(1, 0, 0x07E0, 1, 1, false, None)];

/// A [`ByteSource`] that counts `read_at` calls, so a test can assert which *files* a query
/// touched — the observable §5.6 property is the absence of I/O, not a return value.
struct Counting<'a> {
    bytes: &'a [u8],
    reads: Cell<u32>,
}

impl<'a> Counting<'a> {
    fn new(bytes: &'a [u8]) -> Counting<'a> {
        Counting { bytes, reads: Cell::new(0) }
    }
    fn take(&self) -> u32 {
        let count = self.reads.get();
        self.reads.set(0);
        count
    }
}

impl ByteSource for Counting<'_> {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), IoError> {
        self.reads.set(self.reads.get() + 1);
        SliceSource(self.bytes).read_at(offset, buf)
    }
    fn len(&self) -> u32 {
        self.bytes.len() as u32
    }
}

/// One line per quadrant. The anchor is stored **relative to the leaf bbox**, which is the same
/// quadrant on both sides of the split, so the four chunks are byte-identical in the monolith and
/// in the shards — the property that makes a hand-split set a differential and not a rebuild.
fn fine_chunks() -> [Vec<u8>; 4] {
    core::array::from_fn(|_| seal(pack_line(1, 100, 100, &[(50, 50), (50, -50)]), 4096))
}

fn coarse_chunk() -> Vec<u8> {
    seal(pack_line(1, 200, 200, &[(100, 100), (100, -100)]), 4096)
}

fn pair() -> (Vec<u8>, obcm_testkit::set::SetFixture) {
    matched_pair(ASSEMBLY, STYLES, (COARSE_MPP, coarse_chunk(), 4096), (FINE_MPP, fine_chunks(), 4096))
}

/// A manifest for `files`, keeping the fixture's roles and bboxes but re-recording each shard's
/// `Bytes`. Rebuilding a shard file changes its length, and §5.3 checks the size **first** — so
/// without this the size check would fire and mask whatever the test is actually about.
fn rebuilt_manifest(files: &[Vec<u8>], fixture: &obcm_testkit::set::SetFixture) -> obcs::SetManifest {
    let original = obcs::parse(&fixture.manifest).expect("the fixture's manifest is valid");
    let shards: Vec<obcs::Shard> = original
        .shards()
        .iter()
        .zip(files)
        .map(|(shard, bytes)| obcs::Shard { bytes: bytes.len() as u32, ..*shard })
        .collect();
    obcs::build(
        original.obcm_version,
        original.core_shard() as u8,
        original.schema_revision,
        original.bbox,
        original.set_id,
        original.name,
        &shards,
    )
    .expect("the re-recorded manifest still satisfies §5.3")
}

/// Mount a fixture's shards, running `body` with the mounted set. Keeps the source borrow chain in
/// one place and deliberately drops the manifest first: a successful mount retains compact
/// metadata, not the parsed manifest.
fn with_set<T>(fixture: &obcm_testkit::set::SetFixture, body: impl FnOnce(&MountedSet<'_>) -> T) -> T {
    let sources: Vec<SliceSource> = fixture.sources().into_iter().map(SliceSource).collect();
    let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    let core_index = obcs::parse(&fixture.manifest).expect("hand-built manifest is valid").core_shard();
    let core = MapTables::parse(&sources[core_index]).expect("core parses");
    let cache = MapCache::new();
    let mut store = FullSetShards::new();
    let set = {
        let manifest = obcs::parse(&fixture.manifest).expect("hand-built manifest is valid");
        MountedSet::mount(&mut store, &manifest, &refs, &core, &cache).expect("a complete set mounts")
    };
    body(&set)
}

// ---------------------------------------------------------------------------------------------
// Mounting
// ---------------------------------------------------------------------------------------------

#[test]
fn a_complete_hand_split_set_mounts_as_one_map() {
    let (_, fixture) = pair();
    with_set(&fixture, |set| {
        assert_eq!(set.shard_count(), 6, "core + coarse + four geometry quadrants");
        assert_eq!(
            set.bbox(),
            BBox { min_lon: ASSEMBLY.0, min_lat: ASSEMBLY.1, max_lon: ASSEMBLY.2, max_lat: ASSEMBLY.3 },
            "the mounted bbox is the assembly bbox (§4.2)"
        );
        // §5.4: the only size figure a UI may show is the total.
        let expected: u64 = fixture.shards.iter().map(|s| s.len() as u64).sum();
        assert_eq!(set.total_bytes(), expected);
        assert_eq!(set.role_of(0), Some(Role::Core));
        assert_eq!(set.role_of(1), Some(Role::Coarse));
        assert_eq!(set.role_of(2), Some(Role::Geometry));
        assert_eq!(set.role_of(6), None);
    });
}

/// §5.5: when the whole assembly fits one file the assembler writes a set of **one** — the core,
/// carrying every LOD. The dispatch loop simply runs over one shard.
#[test]
fn the_single_file_fast_path_mounts() {
    let (monolith, _) = pair();
    let fixture = build_set(
        ASSEMBLY,
        STYLES,
        0,
        &[ShardSpec {
            role: Role::Core,
            bbox: ASSEMBLY,
            lods: vec![
                LodSpec { max_mpp: COARSE_MPP, index: vec![0], chunks: vec![coarse_chunk()], chunk_size: 4096 },
                LodSpec {
                    max_mpp: FINE_MPP,
                    index: vec![obcm_testkit::BRANCH_BIT | 1, 0, 1, 2, 3],
                    chunks: fine_chunks().to_vec(),
                    chunk_size: 4096,
                },
            ],
        }],
    );
    // The single shard is byte-identical to the monolith: a set of one costs one extra small file
    // and changes nothing about the map.
    assert_eq!(fixture.shards[0], monolith, "the §5.5 fast path's one shard IS the monolithic map");
    with_set(&fixture, |set| {
        assert_eq!(set.shard_count(), 1);
    });
}

// ---------------------------------------------------------------------------------------------
// §5.4 — incomplete sets never mount
// ---------------------------------------------------------------------------------------------

/// A shard file that is simply not there yet — the ordinary shape of a mid-copy set. The caller
/// hands over the sources it could open, and the count no longer matches the manifest.
#[test]
fn a_missing_shard_refuses_the_whole_set() {
    let (_, fixture) = pair();
    let all: Vec<SliceSource> = fixture.sources().into_iter().map(SliceSource).collect();
    let manifest = obcs::parse(&fixture.manifest).unwrap();
    let core = MapTables::parse(&all[0]).unwrap();
    let cache = MapCache::new();

    // Everything but the last geometry shard.
    let short: Vec<&dyn ByteSource> = all[..5].iter().map(|s| s as &dyn ByteSource).collect();
    assert_eq!(
        MountedSet::mount(&mut FullSetShards::new(), &manifest, &short, &core, &cache).err(),
        Some(MountError::ShardCount),
        "a set with a shard missing must read as incomplete, never as a smaller map"
    );
}

/// No manifest at all ⇒ no set (§5.4). A shard is a valid OBCM file, so the load-bearing property
/// is that its bytes can never be *mistaken* for a manifest: the magic is `OBCM`, not `OBCS`.
#[test]
fn no_manifest_means_no_set() {
    let (_, fixture) = pair();
    assert_eq!(obcs::parse(&[]).err(), Some(ManifestError::Layout), "an absent manifest is not a set");
    assert_eq!(
        obcs::parse(&fixture.shards[0][..obcs::HEADER_LEN]).err(),
        Some(ManifestError::Layout),
        "a shard's own header is not a manifest"
    );
    // …and a manifest truncated mid-copy (the header landed, the records did not).
    assert_eq!(
        obcs::parse(&fixture.manifest[..obcs::HEADER_LEN]).err(),
        Some(ManifestError::Length),
        "a manifest whose shard records have not landed is not a set"
    );
}

/// A shard still growing, or one left from a previous set. §5.3 requires the exact recorded
/// `Bytes` at mount — the check a device MUST do even though it MAY defer the SHA-256.
#[test]
fn a_size_mismatch_refuses_the_whole_set() {
    let (_, fixture) = pair();
    let manifest = obcs::parse(&fixture.manifest).unwrap();
    let cache = MapCache::new();

    // Both directions matter. Short is the mid-copy shape: the file is still being written.
    // Long is the stale-leftover shape: a previous, larger assembly's file under the same derived
    // name. Either way the recorded `Bytes` is the contract, and the *whole set* is refused —
    // deliberately including the case where the offending file still parses as a valid OBCM.
    type Mutation = (usize, fn(&mut Vec<u8>));
    let mutate: [Mutation; 2] = [
        (3, |bytes| {
            bytes.truncate(bytes.len() - 1);
        }),
        (0, |bytes| bytes.push(0)),
    ];
    for (index, apply) in mutate {
        let mut files = fixture.shards.clone();
        apply(&mut files[index]);
        let sources: Vec<SliceSource> = files.iter().map(|f| SliceSource(f.as_slice())).collect();
        let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
        let core = MapTables::parse(&sources[0]).expect("the core still parses");
        assert_eq!(
            MountedSet::mount(&mut FullSetShards::new(), &manifest, &refs, &core, &cache).err(),
            Some(MountError::Size(index as u8)),
            "shard {index} is not the recorded Bytes"
        );
    }
}

/// A shard whose OBCM header bbox is not the bbox the manifest records for it — files from two
/// different assemblies sharing a prefix.
#[test]
fn a_header_bbox_mismatch_refuses_the_whole_set() {
    let (_, fixture) = pair();
    let quads = quadrants(ASSEMBLY);
    // Rebuild the NW geometry shard over the NE quadrant instead, padded back to the same length
    // so the size check cannot fire first and mask the bbox check.
    let mut swapped = fixture.shards.clone();
    swapped[2] = obcm_testkit::build_file(
        quads[1],
        STYLES,
        &[
            empty_lod(COARSE_MPP),
            LodSpec { max_mpp: FINE_MPP, index: vec![0], chunks: vec![fine_chunks()[0].clone()], chunk_size: 4096 },
        ],
    );
    assert_eq!(swapped[2].len(), fixture.shards[2].len(), "the swap keeps the recorded Bytes");

    let manifest = obcs::parse(&fixture.manifest).unwrap();
    let sources: Vec<SliceSource> = swapped.iter().map(|f| SliceSource(f.as_slice())).collect();
    let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    let core = MapTables::parse(&sources[0]).unwrap();
    let cache = MapCache::new();
    assert_eq!(
        MountedSet::mount(&mut FullSetShards::new(), &manifest, &refs, &core, &cache).err(),
        Some(MountError::Bbox(2))
    );
}

/// §4.7 stamps one skin into every shard of a set, so a differing style table means these files
/// are not one map — the shape a half-replaced set takes when the new assembly used a new skin.
#[test]
fn a_differing_style_table_refuses_the_whole_set() {
    let (_, fixture) = pair();
    let other: &[Style] = &[(1, 0, 0xF800, 1, 1, false, None)];
    let quads = quadrants(ASSEMBLY);
    let mut mixed = fixture.shards.clone();
    mixed[2] = obcm_testkit::build_file(
        quads[0],
        other,
        &[
            empty_lod(COARSE_MPP),
            LodSpec { max_mpp: FINE_MPP, index: vec![0], chunks: vec![fine_chunks()[0].clone()], chunk_size: 4096 },
        ],
    );
    assert_eq!(mixed[2].len(), fixture.shards[2].len(), "same-length skin, so only the styles differ");

    let manifest = obcs::parse(&fixture.manifest).unwrap();
    let sources: Vec<SliceSource> = mixed.iter().map(|f| SliceSource(f.as_slice())).collect();
    let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    let core = MapTables::parse(&sources[0]).unwrap();
    let cache = MapCache::new();
    assert_eq!(
        MountedSet::mount(&mut FullSetShards::new(), &manifest, &refs, &core, &cache).err(),
        Some(MountError::Styles(2))
    );
}

/// Shard files no manifest references are **orphans** (§5.4): the mount opens exactly the
/// `Shard Count` files the manifest names, in index order, and a stray `MS<id>S<kk>.OBM` beyond
/// that is neither opened nor allowed to enlarge the set.
#[test]
fn dangling_shards_are_ignored() {
    let (_, fixture) = pair();
    let manifest = obcs::parse(&fixture.manifest).unwrap();
    let cache = MapCache::new();

    // A leftover seventh file sitting on the card next to the set.
    let mut with_orphan = fixture.shards.clone();
    with_orphan.push(fixture.shards[2].clone());
    let sources: Vec<Counting> = with_orphan.iter().map(|f| Counting::new(f.as_slice())).collect();
    let core = MapTables::parse(&sources[0]).unwrap();

    // Handing the mount the orphan too is a count mismatch, not a bigger set.
    let all: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    assert_eq!(
        MountedSet::mount(&mut FullSetShards::new(), &manifest, &all, &core, &cache).err(),
        Some(MountError::ShardCount)
    );

    // Mounting the manifest's own shards succeeds and never touches the orphan's bytes.
    for source in &sources {
        let _ = source.take();
    }
    let named: Vec<&dyn ByteSource> = sources[..6].iter().map(|s| s as &dyn ByteSource).collect();
    let mut store = FullSetShards::new();
    let set =
        MountedSet::mount(&mut store, &manifest, &named, &core, &cache).expect("the named shards are a complete set");
    assert_eq!(set.shard_count(), 6);
    assert_eq!(sources[6].take(), 0, "the orphan is never read");

    // The names themselves say which index a file claims — index 6 is past this set's count.
    assert_eq!(obcs::parse_shard_name(b"MS7S06.OBM"), Some((7, 6)));
    assert!(obcs::parse_shard_name(b"MS7S06.OBM").unwrap().1 >= set.shard_count(), "index 6 is dangling here");
    // …and the neighbouring single-map convention is not a shard at all (§5.2).
    assert_eq!(obcs::parse_shard_name(b"MP7.OBM"), None);
}

/// §5.1 requires every shard to list the **full ladder**, with the rungs it does not carry written
/// empty — and dispatch indexes the *core*'s chosen LOD into each shard's own table. A shard whose
/// ladder disagrees is therefore not "missing detail": it answers a different question at every
/// rung, with reads that look perfectly valid.
///
/// Two shapes, both of which mounted cleanly before this check existed: a **reversed** ladder
/// (rung 1 means the coarse scale) and a **shorter** one (the shard silently contributes nothing).
#[test]
fn a_shard_whose_ladder_is_not_the_cores_refuses_the_whole_set() {
    let (_, fixture) = pair();
    let quads = quadrants(ASSEMBLY);
    let chunk = || fine_chunks()[0].clone();

    // Reversed: the same two rungs, swapped, so LOD 1 is the coarse scale.
    let reversed = vec![
        LodSpec { max_mpp: FINE_MPP, index: vec![0], chunks: vec![chunk()], chunk_size: 4096 },
        empty_lod(COARSE_MPP),
    ];
    // Shorter: one rung where the core lists two.
    let shorter = vec![LodSpec { max_mpp: FINE_MPP, index: vec![0], chunks: vec![chunk()], chunk_size: 4096 }];

    for (what, lods) in [("a reversed ladder", reversed), ("a shorter ladder", shorter)] {
        let mut files = fixture.shards.clone();
        files[2] = obcm_testkit::build_file(quads[0], STYLES, &lods);
        let manifest = rebuilt_manifest(&files, &fixture);
        let sources: Vec<SliceSource> = files.iter().map(|f| SliceSource(f.as_slice())).collect();
        let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
        let core = MapTables::parse(&sources[0]).expect("the core still parses");
        let cache = MapCache::new();
        assert_eq!(
            MountedSet::mount(&mut FullSetShards::new(), &manifest, &refs, &core, &cache).err(),
            Some(MountError::Ladder(2)),
            "{what} must refuse the set"
        );
    }
}

/// A caller can only mount as many shards as its [`SetShards`] holds, and a device's store is
/// smaller than the format's 32 — a mount holds every shard's file handle open. The refusal names
/// **the caller's** cap, because "this device mounts 4" is the sentence a rider needs; "the format
/// allows 32" is not.
#[test]
fn a_set_larger_than_the_callers_store_is_refused_with_the_cap() {
    let (_, fixture) = pair();
    let sources: Vec<SliceSource> = fixture.sources().into_iter().map(SliceSource).collect();
    let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    let manifest = obcs::parse(&fixture.manifest).unwrap();
    let core = MapTables::parse(&sources[0]).unwrap();
    let cache = MapCache::new();

    // A store for four shards, handed a set of six.
    let mut small: SetShards<4> = SetShards::new();
    assert_eq!(small.capacity(), 4);
    assert_eq!(
        MountedSet::mount(&mut small, &manifest, &refs, &core, &cache).err(),
        Some(MountError::Handles(4)),
        "the cap in the error is the caller's, not the format's"
    );
    // Exactly enough is enough.
    let mut exact: SetShards<6> = SetShards::new();
    assert!(MountedSet::mount(&mut exact, &manifest, &refs, &core, &cache).is_ok());
}

/// `MountError::Manifest` is reachable, and this is how: `SetManifest`'s remaining public fields
/// can be moved after it parsed, so the mount re-runs §5.3 rather than trusting its argument.
/// (`core_shard` is *not* among them — it is private precisely so an index cannot go out of range.)
#[test]
fn a_manifest_mutated_after_parsing_refuses_at_mount() {
    let (_, fixture) = pair();
    let sources: Vec<SliceSource> = fixture.sources().into_iter().map(SliceSource).collect();
    let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    let core = MapTables::parse(&sources[0]).unwrap();
    let cache = MapCache::new();

    let mut manifest = obcs::parse(&fixture.manifest).unwrap();
    manifest.bbox = obcs::SetBBox { min_lat: 1, min_lon: 1, max_lat: 0, max_lon: 0 };
    assert_eq!(
        MountedSet::mount(&mut FullSetShards::new(), &manifest, &refs, &core, &cache).err(),
        Some(MountError::Manifest(ManifestError::Geometry))
    );
}

/// §5.3 pins one `OBCM Version` across the whole set, and the mount checks it **per shard** — the
/// same subset the board's scan checks, so the two cannot disagree about what a valid set is.
/// Today this build parses exactly one OBCM version, so the property is also transitively true;
/// that is a reason to state it, not a reason to leave it implicit.
#[test]
fn the_manifests_obcm_version_is_checked_against_every_shard() {
    let (_, fixture) = pair();
    let sources: Vec<SliceSource> = fixture.sources().into_iter().map(SliceSource).collect();
    let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    let core = MapTables::parse(&sources[0]).unwrap();
    let cache = MapCache::new();

    // Every shard's own header carries the version the reader parses…
    for bytes in &fixture.shards {
        assert_eq!(ShardTables::parse(&SliceSource(bytes)).unwrap().version(), obc_formats::obcm::VERSION);
    }
    // …so a manifest claiming another one is refused at the *core* (index 0), the first shard the
    // mount reaches.
    let mut manifest = obcs::parse(&fixture.manifest).unwrap();
    manifest.obcm_version = obc_formats::obcm::VERSION - 1;
    assert_eq!(
        MountedSet::mount(&mut FullSetShards::new(), &manifest, &refs, &core, &cache).err(),
        Some(MountError::Header(0))
    );
}

/// A mixed-depth antichain is §5.1-legal — the shards of a role are quadtree nodes, not a grid — and
/// it must mount. Seven geometry shards at two depths, tiling the assembly exactly.
#[test]
fn a_mixed_depth_antichain_mounts() {
    let deep = obcm_testkit::set::deep_matched_pair(
        ASSEMBLY,
        STYLES,
        (COARSE_MPP, coarse_chunk(), 4096),
        (FINE_MPP, core::array::from_fn(|_| fine_chunks()[0].clone()), 4096),
    );
    with_set(&deep.antichain, |set| {
        assert_eq!(set.shard_count(), 9, "core + coarse + NW's four + three siblings");
        assert_eq!(set.role_of(2), Some(Role::Geometry));
        assert_eq!(set.role_of(8), Some(Role::Geometry));
    });
    with_set(&deep.subdivided, |set| assert_eq!(set.shard_count(), 6));
}

// ---------------------------------------------------------------------------------------------
// §5.1 / §5.6 — role-blind dispatch, and why it is free
// ---------------------------------------------------------------------------------------------

/// §5.6: the per-LOD `Index Node Count == 0` predicate is derived at mount from the LOD table the
/// reader already holds resident — 7 bits per file at the v1 ladder.
#[test]
fn mount_time_empty_lod_cache_reads_the_lod_table_not_the_bands() {
    let (_, fixture) = pair();
    let coarse = ShardTables::parse(&SliceSource(&fixture.shards[1])).expect("the coarse shard parses");
    assert!(!coarse.lod_is_empty(0), "the coarse shard carries LOD 0");
    assert!(coarse.lod_is_empty(1), "and writes LOD 1 empty");
    assert!(coarse.lod_is_empty(2), "a LOD past the ladder is empty, not a panic");

    let geometry = ShardTables::parse(&SliceSource(&fixture.shards[2])).expect("a geometry shard parses");
    assert!(geometry.lod_is_empty(0));
    assert!(!geometry.lod_is_empty(1));
    assert_eq!(geometry.bbox(), {
        let (min_lon, min_lat, max_lon, max_lat) = quadrants(ASSEMBLY)[0];
        BBox { min_lon, min_lat, max_lon, max_lat }
    });
    assert_eq!(geometry.lods().len(), 2, "a shard lists the full ladder (§5.1)");
}

/// The property §5.6 exists for: a zoomed-out viewport reads **exactly one** file (the coarse
/// shard), and a zoomed-in one reads only the geometry shards its box straddles. The core and the
/// unsplit coarse shard intersect every viewport, so without the cache both would be walked into
/// at every zoom level.
#[test]
fn dispatch_touches_only_the_files_a_viewport_needs() {
    use obc_map_scene::MapScene;

    let (_, fixture) = pair();
    let sources: Vec<Counting> = fixture.shards.iter().map(|f| Counting::new(f.as_slice())).collect();
    let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    let manifest = obcs::parse(&fixture.manifest).unwrap();
    let core = MapTables::parse(&sources[0]).unwrap();
    let cache = MapCache::new();
    let mut store = FullSetShards::new();
    let set = MountedSet::mount(&mut store, &manifest, &refs, &core, &cache).unwrap();

    let mut points: heapless::Vec<(i32, i32), 512> = heapless::Vec::new();
    let mut rings: heapless::Vec<usize, 16> = heapless::Vec::new();
    let whole = BBox { min_lon: 0, min_lat: 0, max_lon: 4000, max_lat: 4000 };

    // LOD 0 over the whole assembly: only the coarse shard has anything to say.
    for source in &sources {
        let _ = source.take();
    }
    let mut seen = 0usize;
    set.visit_candidates(0, &whole, &mut points, &mut rings, |_| true, |_| seen += 1);
    assert!(seen > 0, "the coarse shard contributed geometry");
    assert_eq!(sources[0].take(), 0, "the core is never opened for a viewport query (§5.1)");
    assert!(sources[1].take() > 0, "the coarse shard is read");
    for (index, source) in sources.iter().enumerate().skip(2) {
        assert_eq!(source.take(), 0, "geometry shard {index} writes LOD 0 empty — no I/O to discover it");
    }

    // LOD 1 over the NW quadrant only: one geometry shard, and nothing else.
    let nw = quadrants(ASSEMBLY)[0];
    let view = BBox { min_lon: nw.0 + 1, min_lat: nw.1 + 1, max_lon: nw.2 - 1, max_lat: nw.3 - 1 };
    cache.clear().unwrap();
    for source in &sources {
        let _ = source.take();
    }
    seen = 0;
    set.visit_candidates(1, &view, &mut points, &mut rings, |_| true, |_| seen += 1);
    assert!(seen > 0, "the NW geometry shard contributed geometry");
    assert_eq!(sources[0].take(), 0, "the core carries no ladder LOD");
    assert_eq!(sources[1].take(), 0, "the coarse shard writes LOD 1 empty");
    assert!(sources[2].take() > 0, "the NW shard is read");
    for (index, source) in sources.iter().enumerate().skip(3) {
        assert_eq!(source.take(), 0, "shard {index} does not intersect the viewport");
    }
}

/// §5.1: nav and POI queries always go to the core, which is why routing never crosses a file.
#[test]
fn nav_and_poi_always_go_to_the_core() {
    let (_, fixture) = pair();
    let sources: Vec<Counting> = fixture.shards.iter().map(|f| Counting::new(f.as_slice())).collect();
    let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    let manifest = obcs::parse(&fixture.manifest).unwrap();
    let core = MapTables::parse(&sources[0]).unwrap();
    let cache = MapCache::new();
    let mut store = FullSetShards::new();
    let set = MountedSet::mount(&mut store, &manifest, &refs, &core, &cache).unwrap();

    let reader = set.core_reader();
    assert_eq!(reader.file(), 0, "the core reader reads the core file");
    assert!(!reader.is_set_shard(), "the core is not a shard reader");
    assert_eq!(reader.bbox, set.bbox(), "and spans the whole assembly");
    // The fixture's core carries an empty nav graph + empty POI directory, which is what a
    // testkit-built file writes; the contract under test is *which file* answers.
    assert_eq!(reader.poi_directory().entries.len(), obc_formats::obcm::POI_CATEGORY_COUNT as usize);
    assert!(reader.nav_directory().is_empty());
}

/// The other half of §5.1, and the one a doc comment used to be the only guard for: a **shard**
/// reader borrows the core's `MapTables`, so its POI and nav directories are the *core file's*
/// offsets pointed at a *shard's* bytes. That is not a degraded answer, it is a read at an
/// unrelated offset — so every one of those accessors answers empty instead.
///
/// Every accessor is checked against the **core** reader over the same tables, so "empty" is a
/// decision this code made and not the fixture being empty anyway.
#[test]
fn nav_and_poi_accessors_are_empty_on_a_shard_reader() {
    use obc_formats::obcm::PoiCategory;
    use obc_reader::corridor::PoiCategorySet;

    let (_, fixture) = pair();
    with_set(&fixture, |set| {
        let core = set.core_reader();
        let shard = set.shard_reader(2).expect("shard 2 is a geometry shard");
        assert!(!core.is_set_shard() && shard.is_set_shard());

        // The three table accessors: the core's are the map's, the shard's are empty.
        assert_eq!(core.poi_directory().entries.len(), obc_formats::obcm::POI_CATEGORY_COUNT as usize);
        assert!(shard.poi_directory().entries.is_empty(), "poi_directory");
        assert!(core.poi_directory().chunk_size > 0 && shard.poi_directory().chunk_size == 0);
        assert!(core.nav_directory().chunk_size > 0 && shard.nav_directory().chunk_size == 0, "nav_directory");
        assert!(!core.nav_profiles().is_empty() && shard.nav_profiles().is_empty(), "nav_profiles");

        // …and the seven query paths, which are the ones that would otherwise read the core's
        // offsets against a shard's bytes.
        assert_eq!(shard.poi_hours(0), None, "poi_hours");
        let mut found: heapless::Vec<obc_reader::Poi, { obc_reader::MAX_POI_RESULTS }> = heapless::Vec::new();
        shard.nearest_pois(PoiCategory::Water, (2000, 2000), &mut found).unwrap();
        assert!(found.is_empty(), "nearest_pois");
        let mut corridor: heapless::Vec<obc_reader::CorridorPoi, { obc_reader::MAX_CORRIDOR_RESULTS }> =
            heapless::Vec::new();
        shard.corridor_pois(PoiCategorySet::ALL, &NoPath, 0, &mut corridor).unwrap();
        assert!(corridor.is_empty(), "corridor_pois");

        let view = BBox { min_lon: ASSEMBLY.0, min_lat: ASSEMBLY.1, max_lon: ASSEMBLY.2, max_lat: ASSEMBLY.3 };
        let mut scratch = [0u8; obc_reader::NAV_MAX_CHUNK_BYTES];
        let mut visited = 0usize;
        shard.for_each_nav_node(&view, &mut scratch, |_| visited += 1).unwrap();
        assert_eq!(visited, 0, "for_each_nav_node");
        let mut tiles = obc_reader::NavTileCache::new();
        shard.for_each_nav_node_cached(&view, &mut tiles, |_| visited += 1).unwrap();
        assert_eq!(visited, 0, "for_each_nav_node_cached");
        let mut points: heapless::Vec<(i32, i32), 8> = heapless::Vec::new();
        assert_eq!(shard.nav_edge(0, &mut points), None, "nav_edge");
        assert_eq!(shard.nav_edge_oriented(&mut tiles, 0, (0, 0), |_| {}), None, "nav_edge_oriented");
    });
}

/// A zero-chunk route path — `corridor_pois` needs one and this test is not about routing.
struct NoPath;

impl obc_reader::corridor::RoutePath for NoPath {
    fn chunk_count(&self) -> usize {
        0
    }
    fn chunk_start_m(&self, _k: usize) -> u32 {
        0
    }
    fn chunk_bbox(&self, _k: usize) -> BBox {
        BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 }
    }
    fn visit_chunk_points(&self, _k: usize, _visit: &mut dyn FnMut(&[(i32, i32)])) {}
}
