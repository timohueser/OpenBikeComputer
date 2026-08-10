//! `wx/v2/manifest.json`: the manifest of the **canonical sharded dataset** (WXR4 #1243).
//!
//! Manifest v1 exists so a client can *choose* — between products, tiers, bboxes and staleness.
//! There is nothing left to choose between: the baker publishes one dataset, on one lattice, at one
//! cadence, cut into a fixed global shard grid. So v2 carries **nothing selectable**. What is left
//! is the four things a client genuinely cannot compute:
//!
//! 1. **which generation is current** (and which two before it are still fetchable);
//! 2. **the constants of the grid** — lattice origin, pitch, extent, shard size, tile edge, paging,
//!    covered rows — stated rather than hardcoded, so the dataset can be re-cut without a client
//!    release;
//! 3. **what exists**, per frame: a shard presence bitmap plus, for every present shard, its byte
//!    length, its object CRC-32 and whether it was painted by an observation;
//! 4. **when this stops being usable**, as absolute deadlines rather than client-side constants.
//!
//! Everything else is arithmetic the client does for itself: which shards cover its bbox, and what
//! their object keys are (§10 of `OBCG_Spec.md` is the normative key scheme).
//!
//! ## Missing is not dry
//!
//! The presence bitmap is the whole of the "a 404 must not mean dry" contract, and it makes three
//! states distinguishable where a bare `GET` makes two:
//!
//! | bitmap bit | shard on the grid | meaning |
//! | --- | --- | --- |
//! | set | yes | the object **exists**; a 404 or a length/CRC mismatch is an **error** — retry, never dry |
//! | clear | yes | every cell of that shard is **dry** (intensity 0); there is no object to fetch |
//! | - | no | **out of domain**: the bbox reaches off the lattice |
//!
//! A shard that is entirely *no-data* (a floor-source outage, or the polar band) is **published**,
//! as an object full of intensity 15. Only genuinely dry shards are omitted, so a bit-clear shard
//! can never be an outage in disguise. That is the invariant [`crate::canonical::FillOutcome`]
//! measures and this document reports.
//!
//! Sentinel objects were the alternative: one tiny published object per dry shard, so absence is
//! always an error. They were rejected. A sentinel's only advantage over a bitmap is surviving a
//! manifest that is briefly behind its objects — and under immutable per-generation keys that state
//! does not exist, because the manifest swaps last and never describes a generation whose objects
//! are not already fetchable. What sentinels do cost is real: an object and a request for exactly
//! the case that should be free. The bitmap is 24 bits a frame and lives in the document the client
//! has already fetched and already trusts for `bytes` and `object_crc32`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::canonical::{self, CycleTimes, Lattice};
use crate::manifest::{key_timestamp, rfc3339};

/// The mutable document key, beside the immutable objects it names.
pub const MANIFEST_KEY: &str = "wx/v2/manifest.json";
pub const MANIFEST_VERSION: u32 = 2;

/// The one prefix every object of this dataset lives under. Stated in the manifest so the tree can
/// move without a client release; the rest of the key is arithmetic (see [`shard_key`]).
pub const KEY_PREFIX: &str = "wx/v2";

/// How long a client may reuse a fetched manifest before re-reading it. The baker's own
/// `Cache-Control` says the same thing; this states it inside the document, so the freshness rule
/// survives a proxy that rewrites headers.
pub const MANIFEST_MAX_AGE_S: u32 = 60;

/// How many superseded generations stay fetchable after a new one is published.
///
/// **This is the retention contract, and WXR8's sweep derives its delete set from exactly this
/// document**: every generation prefix under [`KEY_PREFIX`] that is neither
/// [`Manifest::generation`] nor listed in [`Manifest::previous_generations`] may be deleted, and
/// nothing else may be. Two is the smallest number that is honest: a client that fetched the
/// manifest just before a swap is mid-way through a corridor read of the generation it names, and
/// one more slot covers a client that was slow, or a cycle that published twice inside a client's
/// 60-second manifest cache.
pub const RETAINED_PREVIOUS_GENERATIONS: usize = 2;

/// The generation identifier: the cycle's reference time to the minute, `YYYYMMDD'T'HHMM'Z'`.
/// It is the immutable key segment as well as the identity, so re-baking a reference time
/// overwrites its objects with identical bytes rather than creating a second generation.
pub fn generation_id(reference_time: i64) -> String {
    key_timestamp(reference_time)
}

/// The object key of one shard of one frame — **the one part of the contract a client computes
/// rather than reads**, normative in `OBCG_Spec.md` §10.
///
/// `<prefix>/<generation>/f<offset-min>/s<col>-<row>.obcg`
///
/// Shards are addressed by `(col, row)` from the lattice's south-west corner rather than by a flat
/// index, for one reason: a client derives `(col, row)` directly from its bbox by division, and a
/// flat index would make it multiply by `shard_cols` to name the object and divide back to read the
/// bitmap. Two spellings of the same identity is where the off-by-one lives.
pub fn shard_key(prefix: &str, generation: &str, offset_min: u32, col: u32, row: u32) -> String {
    format!("{prefix}/{generation}/f{offset_min}/s{col}-{row}.obcg")
}

// ---------------------------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Manifest document version; readers reject an unknown value.
    pub version: u32,
    /// The current generation: `YYYYMMDD'T'HHMM'Z'`, and the second segment of every object key.
    pub generation: String,
    /// Wall-clock UTC time this manifest was produced (RFC 3339 seconds).
    pub generated_at: String,
    /// The cycle anchor: frame `f<offset>` is valid at `reference_time + offset` minutes.
    pub reference_time: String,
    /// The prefix every object key of this dataset starts with.
    pub key_prefix: String,
    /// Superseded generations whose objects are still fetchable, **newest first**, at most
    /// [`RETAINED_PREVIOUS_GENERATIONS`]. A client holding a stale manifest may finish reading
    /// from one of these; a sweep may delete anything not named here or by `generation`.
    pub previous_generations: Vec<String>,
    pub lattice: LatticeDescriptor,
    pub cadence: Cadence,
    pub freshness: Freshness,
    /// Every source that may have painted a cell of this generation, in mosaic priority order.
    /// There is no per-cell provenance (#1242), so attribution is stated for the dataset as a
    /// whole and every one of these lines must be displayable together.
    pub attribution: Vec<AttributionEntry>,
    pub frames: Vec<Frame>,
}

/// The grid, stated so no client hardcodes it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LatticeDescriptor {
    pub south_lat_udeg: i32,
    pub west_lon_udeg: i32,
    /// Cell pitch in microdegrees, both axes; the lattice is square in degrees, not in metres.
    pub cell_udeg: u32,
    pub width: u32,
    pub height: u32,
    pub shard_width: u32,
    pub shard_height: u32,
    /// Shard columns west-to-east and rows south-to-north. The bitmap bit of shard `(col, row)`
    /// is at index `row * shard_cols + col`.
    pub shard_cols: u32,
    pub shard_rows: u32,
    pub tile_edge: u16,
    pub entries_per_page: u16,
    /// The `cell_size_m` every object of this dataset declares.
    pub cell_size_m: u16,
    /// The lattice rows at least one source reaches, `[first, end)`. Rows outside it have no
    /// source at all and are published as intensity 15 in every frame, forever — a permanent
    /// property of the dataset rather than an outage, so it is stated once here instead of being
    /// inferred from cells (#1242's polar band).
    pub covered_rows: RowRange,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RowRange {
    pub first: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cadence {
    /// Minutes between consecutive frames, and between consecutive generations.
    pub frame_step_min: u32,
    /// Frames per generation; offsets are `0, step, 2*step, ...`.
    pub frames: u32,
    /// How far a source frame may sit from a canonical frame's validity and still have painted it
    /// (`canonical::MAX_FRAME_SKEW_S`). A bake-time property of the data, stated because it bounds
    /// how much older than its `valid_at` a cell's underlying observation can be — a client that
    /// wants to caveat "radar, up to N minutes old" reads it here rather than assuming a number.
    pub max_source_skew_s: i64,
}

/// Deadlines, absolute. Every "how old is too old" decision a client used to make from its own
/// constants is one of these fields — a client compares timestamps and never counts minutes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Freshness {
    /// How long a fetched copy of *this document* may be reused before re-reading it.
    pub manifest_max_age_s: u32,
    /// When the next generation should exist. Past it with no new manifest, the service is late;
    /// the data is not yet unusable. This is the probe's alarm, not the client's.
    pub next_generation_expected_at: String,
    /// When this generation stops being usable at all: the validity of its **last** frame. Past
    /// it every frame describes the past, so there is nothing left to answer with. Derived, not
    /// chosen. Expiry never turns into a dry claim — a client past this deadline has *no* weather,
    /// which is a different thing from no rain.
    pub stale_after: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttributionEntry {
    /// The mosaic source id this line belongs to (`dwd-rv`, `gfs`, ...). Display metadata: a
    /// client MUST NOT branch on it, and it is not a selectable product.
    pub source_id: String,
    pub text: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    /// `(valid_at - reference_time)` in minutes; the `f<offset-min>` key segment.
    pub offset_min: u32,
    /// The frame's UTC validity (RFC 3339) — never a re-stamped bake time.
    pub valid_at: String,
    /// Shard presence, one bit per shard, `ceil(shard_count / 8)` bytes as lowercase hex, first
    /// byte first, least-significant bit first inside each byte. Bit `row * shard_cols + col` set
    /// means the object exists; clear means that shard is entirely dry. Bits past `shard_count` in
    /// the final byte are zero.
    pub present: String,
    /// One entry per present shard, ascending by `(row, col)`. Exactly the shards `present` names:
    /// a reader MUST reject a frame where the two disagree.
    pub shards: Vec<Shard>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Shard {
    pub col: u32,
    pub row: u32,
    /// Exact object length; a client may use it to bound Range arithmetic.
    pub bytes: u64,
    /// The OBCG whole-object CRC-32 (`0x` + 8 uppercase hex digits).
    pub object_crc32: String,
    /// Was **every** cell of this shard painted by an observation? Per shard rather than per
    /// frame, because a mosaic frame is radar over Germany and model over the Atlantic at the same
    /// instant — this mirrors the object's own `FLAG_OBSERVED`, which the baker measures.
    pub observed: bool,
}

/// Stable pretty serialization: struct field order is declaration order, so the same manifest
/// content is always the same bytes (the byte-stable-cycle contract).
pub fn to_json(manifest: &Manifest) -> String {
    let mut text = serde_json::to_string_pretty(manifest).expect("manifest serializes");
    text.push('\n');
    text
}

pub fn from_json(bytes: &[u8]) -> Result<Manifest, String> {
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|error| format!("manifest v2 parse: {error}"))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(format!("manifest version {} is not {MANIFEST_VERSION}", manifest.version));
    }
    Ok(manifest)
}

/// The generated JSON Schema for the v2 document.
pub fn schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Manifest)).expect("manifest v2 schema serializes")
}

/// Stable pretty schema text used to regenerate the checked-in file:
/// `cargo run -p obc-wx-bake --bin obc-wx-bake -- schema --v2 > host/obc-wx-bake/schema/manifest-v2.schema.json`.
pub fn schema_json() -> String {
    let mut text = serde_json::to_string_pretty(&schema()).expect("manifest v2 schema serializes");
    text.push('\n');
    text
}

pub const CHECKED_IN_SCHEMA: &str = include_str!("../schema/manifest-v2.schema.json");

// ---------------------------------------------------------------------------------------------
// Building one
// ---------------------------------------------------------------------------------------------

/// Accumulates a v2 manifest as the cycle streams frames past it.
///
/// The builder holds the *whole* frame list from the start, including frames no shard was
/// published for: a frame with an all-clear bitmap is "everywhere dry", which is data, and leaving
/// it out would make it indistinguishable from a frame the baker never got to.
#[derive(Debug)]
pub struct Builder {
    manifest: Manifest,
    shard_cols: u32,
    shard_count: u32,
}

impl Builder {
    pub fn new(
        lattice: &Lattice,
        times: CycleTimes,
        generated_at: i64,
        attribution: Vec<AttributionEntry>,
        previous_generations: Vec<String>,
    ) -> Self {
        let generation = generation_id(times.reference_time);
        let covered = lattice.covered_rows();
        let shard_cols = lattice.shard_cols();
        let shard_count = lattice.shard_count();
        let bitmap_bytes = shard_count.div_ceil(8) as usize;
        let last_offset = times.offsets_min().last().unwrap_or(0);
        let frames = times
            .offsets_min()
            .map(|offset_min| Frame {
                offset_min,
                valid_at: rfc3339(times.valid_at(offset_min)),
                present: hex(&vec![0u8; bitmap_bytes]),
                shards: Vec::new(),
            })
            .collect();
        let previous_generations = previous_generations
            .into_iter()
            // A re-bake of the same reference time is the *same* generation, not its own
            // predecessor: listing it would make the sweep's keep-set one generation short.
            .filter(|previous| previous != &generation)
            .take(RETAINED_PREVIOUS_GENERATIONS)
            .collect();
        Self {
            manifest: Manifest {
                version: MANIFEST_VERSION,
                generation,
                generated_at: rfc3339(generated_at),
                reference_time: rfc3339(times.reference_time),
                key_prefix: KEY_PREFIX.to_string(),
                previous_generations,
                lattice: LatticeDescriptor {
                    south_lat_udeg: lattice.south_lat_udeg,
                    west_lon_udeg: lattice.west_lon_udeg,
                    cell_udeg: lattice.cell_udeg,
                    width: lattice.width,
                    height: lattice.height,
                    shard_width: lattice.shard_width,
                    shard_height: lattice.shard_height,
                    shard_cols,
                    shard_rows: lattice.shard_rows(),
                    tile_edge: lattice.tile_edge,
                    entries_per_page: lattice.entries_per_page,
                    cell_size_m: lattice.cell_size_m,
                    covered_rows: RowRange { first: covered.start, end: covered.end },
                },
                cadence: Cadence {
                    frame_step_min: canonical::FRAME_STEP_MIN,
                    frames: canonical::CYCLE_FRAMES,
                    max_source_skew_s: canonical::MAX_FRAME_SKEW_S,
                },
                freshness: Freshness {
                    manifest_max_age_s: MANIFEST_MAX_AGE_S,
                    next_generation_expected_at: rfc3339(
                        times.reference_time + i64::from(canonical::FRAME_STEP_MIN) * 60,
                    ),
                    stale_after: rfc3339(times.valid_at(last_offset)),
                },
                attribution,
                frames,
            },
            shard_cols,
            shard_count,
        }
    }

    /// Record one **published** shard. Dry shards are recorded by never being recorded: their bit
    /// stays clear and no entry appears.
    pub fn record(&mut self, offset_min: u32, col: u32, row: u32, bytes: u64, object_crc32: u32, observed: bool) {
        let index = row * self.shard_cols + col;
        debug_assert!(index < self.shard_count, "shard ({col},{row}) is not on this lattice");
        let Some(frame) = self.manifest.frames.iter_mut().find(|frame| frame.offset_min == offset_min) else {
            debug_assert!(false, "f{offset_min} is not a frame of this cycle");
            return;
        };
        let mut bitmap = unhex(&frame.present).unwrap_or_else(|| vec![0u8; self.shard_count.div_ceil(8) as usize]);
        bitmap[(index / 8) as usize] |= 1 << (index % 8);
        frame.present = hex(&bitmap);
        frame.shards.push(Shard { col, row, bytes, object_crc32: format!("0x{object_crc32:08X}"), observed });
        frame.shards.sort_by_key(|shard| (shard.row, shard.col));
    }

    pub fn finish(self) -> Manifest {
        self.manifest
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

/// The previous manifest's generation chain, newest first: what the one about to be published must
/// promise to keep. An absent or unreadable document is a first publish, not an error — the
/// retention contract can only shrink the keep-set to the current generation, which is safe.
pub fn carried_generations(previous: Option<&[u8]>) -> Vec<String> {
    let Some(bytes) = previous else { return Vec::new() };
    let Ok(manifest) = from_json(bytes) else { return Vec::new() };
    let mut chain = vec![manifest.generation];
    chain.extend(manifest.previous_generations);
    chain.truncate(RETAINED_PREVIOUS_GENERATIONS);
    chain
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len() / 2).map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::CANONICAL;

    /// The obc-pack schema discipline: the checked-in schema must equal the generated one, or the
    /// document every client parses against lies about what this binary writes.
    #[test]
    fn checked_in_schema_is_current() {
        let checked_in: serde_json::Value =
            serde_json::from_str(CHECKED_IN_SCHEMA).expect("schema/manifest-v2.schema.json parses");
        assert_eq!(
            checked_in,
            schema(),
            "schema/manifest-v2.schema.json is stale; regenerate with `cargo run -p obc-wx-bake --bin obc-wx-bake -- schema --v2 > host/obc-wx-bake/schema/manifest-v2.schema.json`"
        );
    }

    #[test]
    fn the_key_scheme_is_prefix_generation_frame_shard() {
        let generation = generation_id(1_800_000_000);
        assert_eq!(generation, "20270115T0800Z");
        assert_eq!(shard_key(KEY_PREFIX, &generation, 45, 3, 2), "wx/v2/20270115T0800Z/f45/s3-2.obcg");
    }

    /// The bitmap is the presence contract; `shards[]` is its detail. They are written from one
    /// call, and this pins that they stay one fact.
    #[test]
    fn the_bitmap_and_the_shard_list_are_the_same_statement() {
        let times = CycleTimes { reference_time: 1_800_000_000 };
        let mut builder = Builder::new(&CANONICAL, times, 1_800_000_060, Vec::new(), Vec::new());
        // Shards 0 (0,0), 7 (1,1) and 23 (5,3) of the 6 x 4 grid.
        builder.record(0, 0, 0, 100, 0xDEAD_BEEF, true);
        builder.record(0, 5, 3, 300, 0x0000_0001, false);
        builder.record(0, 1, 1, 200, 0x0000_0002, false);
        let manifest = builder.finish();
        let frame = &manifest.frames[0];
        // bits 0, 7 and 23 -> bytes 0x81, 0x00, 0x80.
        assert_eq!(frame.present, "810080");
        assert_eq!(
            frame.shards.iter().map(|shard| (shard.col, shard.row)).collect::<Vec<_>>(),
            vec![(0, 0), (1, 1), (5, 3)],
            "ascending by (row, col)"
        );
        assert_eq!(frame.shards[0].object_crc32, "0xDEADBEEF");
        // Every other frame of the cycle is all-dry, and says so rather than being absent.
        assert_eq!(manifest.frames.len(), canonical::CYCLE_FRAMES as usize);
        assert!(manifest.frames[1..].iter().all(|frame| frame.present == "000000" && frame.shards.is_empty()));
    }

    /// The deadlines are derived from the cycle, not chosen: the last frame's validity is the
    /// moment the generation can answer nothing, and one frame step is when its successor is due.
    #[test]
    fn the_freshness_deadlines_are_derived_from_the_time_axis() {
        let times = CycleTimes::anchored_at(crate::manifest::parse_rfc3339("2026-08-10T14:37:00Z").expect("ts"));
        let manifest = Builder::new(&CANONICAL, times, times.reference_time, Vec::new(), Vec::new()).finish();
        assert_eq!(manifest.reference_time, "2026-08-10T14:30:00Z");
        assert_eq!(manifest.generation, "20260810T1430Z");
        assert_eq!(manifest.freshness.next_generation_expected_at, "2026-08-10T14:45:00Z");
        assert_eq!(manifest.freshness.stale_after, "2026-08-10T16:30:00Z");
        assert_eq!(manifest.cadence.max_source_skew_s, canonical::MAX_FRAME_SKEW_S);
    }

    /// The retention contract: current plus two, newest first, and a re-bake of the same reference
    /// time is not its own predecessor.
    #[test]
    fn the_generation_chain_keeps_exactly_the_two_before_it() {
        let times = CycleTimes::anchored_at(crate::manifest::parse_rfc3339("2026-08-10T15:00:00Z").expect("ts"));
        let previous = Builder::new(
            &CANONICAL,
            CycleTimes::anchored_at(crate::manifest::parse_rfc3339("2026-08-10T14:45:00Z").expect("ts")),
            0,
            Vec::new(),
            vec!["20260810T1430Z".into(), "20260810T1415Z".into()],
        )
        .finish();
        let carried = carried_generations(Some(to_json(&previous).as_bytes()));
        assert_eq!(carried, vec!["20260810T1445Z", "20260810T1430Z"]);
        let current = Builder::new(&CANONICAL, times, 0, Vec::new(), carried).finish();
        assert_eq!(current.previous_generations, vec!["20260810T1445Z", "20260810T1430Z"]);

        // Re-baking 15:00 must not carry 15:00 as its own predecessor.
        let repeat =
            Builder::new(&CANONICAL, times, 0, Vec::new(), carried_generations(Some(to_json(&current).as_bytes())))
                .finish();
        assert_eq!(repeat.previous_generations, vec!["20260810T1445Z"]);
        assert_eq!(carried_generations(None), Vec::<String>::new());
        assert_eq!(carried_generations(Some(b"not json")), Vec::<String>::new());
    }

    /// The shared cross-language fixture is a document **this binary would write**: parsed back
    /// through the writer model, which is `deny_unknown_fields`, so a field the fixture invents or
    /// misspells fails here rather than being quietly ignored by the two lenient clients that also
    /// read it (`host/obc-wx-client/tests/manifest_v2.rs`, and its Swift twin in WXR5).
    #[test]
    fn the_shared_cross_language_fixture_is_a_document_this_baker_would_write() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/wx-manifest-v2.json");
        let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let manifest = from_json(&bytes).expect("the shared fixture is a valid v2 document");
        assert_eq!(manifest.generation, "20260810T1430Z");
        assert_eq!(manifest.key_prefix, KEY_PREFIX);
        assert_eq!(manifest.previous_generations.len(), RETAINED_PREVIOUS_GENERATIONS);
        assert_eq!(manifest.frames.len(), canonical::CYCLE_FRAMES as usize);
        // The fixture pins the production lattice, so a re-cut of the grid must update it.
        let live = Builder::new(&CANONICAL, CycleTimes { reference_time: 0 }, 0, Vec::new(), Vec::new()).finish();
        assert_eq!(serde_json::to_value(manifest.lattice).unwrap(), serde_json::to_value(live.lattice).unwrap());
        assert_eq!(serde_json::to_value(manifest.cadence).unwrap(), serde_json::to_value(live.cadence).unwrap());
        assert_eq!(manifest.freshness.manifest_max_age_s, MANIFEST_MAX_AGE_S);
    }

    /// The document states the grid so no client hardcodes it — including the polar band, which is
    /// permanent and therefore worth saying once instead of leaving a reader to infer it.
    #[test]
    fn the_lattice_and_its_covered_rows_are_stated() {
        let times = CycleTimes { reference_time: 1_800_000_000 };
        let manifest = Builder::new(&CANONICAL, times, 0, Vec::new(), Vec::new()).finish();
        let lattice = manifest.lattice;
        assert_eq!((lattice.width, lattice.height), (36_000, 18_000));
        assert_eq!((lattice.shard_cols, lattice.shard_rows), (6, 4));
        assert_eq!((lattice.shard_width, lattice.shard_height), (6_144, 4_608));
        assert_eq!(lattice.cell_size_m, canonical::LATTICE_CELL_SIZE_M);
        let covered = CANONICAL.covered_rows();
        assert_eq!((lattice.covered_rows.first, lattice.covered_rows.end), (covered.start, covered.end));
        assert!(lattice.covered_rows.first > 0 && lattice.covered_rows.end < lattice.height);
    }
}
