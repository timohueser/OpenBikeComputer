# WIP — #1026 firmware volume sets

**Delete this file before the PR leaves draft.**

Branch `feat/1026-fw-volume-sets`, base `develop`. Two commits:

1. `feat(formats): OBCS volume-set manifest codec (#1026)` — **DONE, green.**
2. `wip(fw): checkpoint — volume-set map source, not yet compiling` — in flight.

## DONE

### `firmware/obc-formats/src/obcs.rs` (new, 27 tests passing)

The whole `OBCA_Spec.md` §5.2 / §5.3 codec: `parse`, `serialize`, `build`,
`validate`, `shard_digest`, `Role`, `SetBBox`, `Shard`, `SetManifest`, and the
derived-filename pair `manifest_name` / `shard_name` + their strict parsers
`parse_manifest_name` / `parse_shard_name`.

`cargo test -p obc-formats` — passes. `cargo fmt -p obc-formats` and
`cargo clippy -p obc-formats --all-targets -- -D warnings` — clean.

Two decisions worth keeping:

- SHA-256 digests are **not** resident in `SetManifest` (§5.3 lets a device defer
  hashing; 32 shards × 32 B = 1 KiB of RAM nothing reads). `shard_digest(bytes, i)`
  reaches them in the caller's buffer. Resident cost is 24 B/shard, ~840 B at cap.
- The "shards of a role tile the assembly" rule is checked as pairwise
  **interior**-disjointness (abutting boxes share edges, not interiors) plus an
  exact `i64` area sum. That is the §5.1 antichain property without polygon
  arithmetic.

## IN PROGRESS — does not compile

`cargo build -p obc-reader` currently fails. Everything below is written but
unverified.

### `firmware/obc-reader/src/reader.rs` (edited)

- `ChunkSlot` and `IndexBlock` gained `file: u8`, threaded through
  `load_chunk(src, file, lod, cid, …)`, `index_read(src, file, off, out)` and
  `index_block(src, file, block_off)`. Both fields land in existing padding, so
  the RAM delta is **zero**. Call sites and the in-file cache unit tests updated.
- `Reader` gained `file: u8` and `shard_lods: Option<&'a [Lod]>`;
  `Reader::lods()` prefers the override. `Reader::new` sets `file: 0,
  shard_lods: None` (single-map path byte-for-byte unchanged).
- `Reader::new_in_set(src, tables, cache, file, shard: Option<&ShardTables>)` —
  new. Also `Reader::file()`.
- `MapTables::parse_member(src, of)` — parses a shard adopting `of`'s parse
  generation. This is load-bearing: `MapCache::adopt` clears everything resident
  when the generation changes, so per-shard generations would make a set thrash
  its own cache on every dispatch hop. Safe only because `new_in_set` also tags
  every cache key with the shard index.
- `MapTables` accessors added: `lod_is_empty(lod)`, `lods()`, `styles()`.
- `parse_header` and `parse_lod_table` are now `pub(crate)` so `volume.rs` can
  parse a shard's header + LOD table **without** `parse_styles`' 2 KiB stack
  scratch (the ~36 KB-stack rule: nothing large in the mount path).

### `firmware/obc-reader/src/volume.rs` (new)

`ShardTables` (bbox + LOD table + `empty: u16` §5.6 bitmask) and `MountedSet`,
which implements `MapScene` so the renderer needs no changes. Dispatch is
role-blind: `dispatches(shard, lod, view)` = bbox intersect **and** not
§5.6-empty, both from resident bytes. `core_reader()` is where nav/POI/hours go.
`MountedSet::mount` does the reader half of §5.3 (source count, exact `Bytes`,
OBCM version, header bbox == recorded bbox) and validates the style tables are
byte-identical across shards by streaming a 64-byte window — validate, don't
re-load (§4.7). No partial mount: a mid-copy set errors, never mounts.

Pass-A/pass-B identity across shards: `tag_token` / `untag_token` steal the top
5 bits of the `FeatureToken`'s chunk-id high word for the shard index, leaving
2^27 chunk ids per LOD (three orders of magnitude past the 4 GiB file ceiling).

### Known compile errors to fix first

- `firmware/obc-reader/src/lib.rs` re-exports `MountError, MountedSet,
  ShardTables` — check the `pub use` ordering/duplication against `reader::*`.
- `volume.rs` `dispatches()` has a `== false` clippy-bait line; rewrite as `!`.
- `volume.rs` uses `crate::MapReadError`, `crate::CacheError`,
  `crate::FeatureReadError`, `crate::FeatureDecodeError`, `crate::CapacityError`
  — confirm each is re-exported at the crate root (they are `pub use reader::…`).
- `ShardTables::parse` calls `obc_formats::io::rd_u32`; confirm the import path.
- `MountedSet::mount` borrows `sources[core_index]` inside a loop that also
  holds `sources` — may need a `let core_src = sources[core_index];` hoist.
- `Lod` is `pub` but check `parse_lod_table`'s return type `Vec<Lod, 16>` is
  nameable from `volume.rs`.

## REMAINS (nothing started)

1. **Tests for `volume.rs`.** The plan: build shard sets *at test time* with
   `host/obcm-testkit` (`build_file(bbox, styles, lods)` — bbox is a per-call
   argument, so several files with different bboxes is trivial) rather than
   committing fixtures (fixture rule: `.obcm` fixtures regenerate only via
   `assets/repack.sh`).
   - **Differential test**: render a monolithic map and the same data as a
     hand-split set, assert pixel-identical frames, including viewports that
     straddle a shard boundary. Reusable helper:
     `firmware/obc-render/tests/fill_edges.rs:38` `render_into(&mut Buf, bytes,
     &Viewport) -> RenderStats` (bytes in, pixels out — closest fit); `Buf` is
     copy-pasted in `firmware/obc-render/tests/common/mod.rs:14` and
     `firmware/obc-app/tests/common/mod.rs:22`, so it needs a local copy or a
     shared export. `host/obc-bench/src/main.rs:70` has an FNV-1a `frame_hash`.
     `MapRenderer::render` is already generic over `S: MapScene`, so a
     `MountedSet` drops straight in.
   - **Refusal tests**: missing shard, missing manifest, size mismatch, dangling
     shard ignored. The codec-level half is done; these are the mount-level half.
2. **Registry / UI (scope item 3) — not started, and it is the big one.**
   Everything lives in `firmware/obc-fw-nrf54l/src/sd.rs` (workspace-EXCLUDED —
   build it separately). Concretely:
   - `is_map_entry` (sd.rs:2980) accepts **any** `*.OBM`, so today a bare shard
     would list as a standalone map — a direct §5.4 violation and the
     safety-critical fix. `MS<id>S<kk>.OBM` must be excluded from the standalone
     catalog and `.OBS` recognised. `id_in_name` (sd.rs:3055) parses
     `{prefix}{u16}.{ext}` and cannot express `MS<id>S<kk>` — use
     `obc_formats::obcs::parse_shard_name` / `parse_manifest_name`.
   - `MapSummary` (sd.rs:273) holds one `file` / `byte_len` / `entry_block` /
     `entry_offset`; a set needs N of each, and one summed size (§5.4).
   - `scan_maps_into` (sd.rs:1375), `open_map` (sd.rs:1286), `map_source`
     (sd.rs:1604), `MapSource` enum (sd.rs:300), `open_map: Option<(RawFile,
     u32)>` and `map_extents` / `static mut MAP_EXTENTS` are all single-slot.
   - `obc_app::map_catalog::{MapChoice, choose_map, is_superseded_upload}`
     (`firmware/obc-app/src/map_catalog.rs`) reason in single-file terms;
     `is_superseded_upload` would **delete a set's siblings** today.
   - One delete removing the whole prefix: the three delete paths are
     `retire_superseded_maps` (sd.rs:1337), `map_upload_abort` (sd.rs:2359),
     `sweep_aborted_maps` (sd.rs:2385) — all single-file `delete_file_in_dir`.
   - ⚠ **Magic collision**: `firmware/obc-app/src/store_meta.rs:230` already uses
     `b"OBCS"` as the `MAP.SEL` magic (and `ride.rs:102` for `SYNCED.SET`), which
     is also the OBCA §5.2 manifest magic. Different files, so not a bug, but it
     will confuse a grep — worth a comment at both sites.
   - ⚠ **Blast radius**: `obc-app` re-concretizes the render seam —
     `Ctx.reader: Option<&'a Reader<'d>>` (`screen/mod.rs:216`), `Render.reader`
     (`mod.rs:387`), `App::render_map_timed(…, reader: Option<&Reader>, …)`
     (`app.rs:2163`) all name `obc_reader::Reader` by type. Making the app hold a
     `MountedSet` means genericising those over `S: MapScene`.
3. **FAT handles (scope item 4) — not started.**
   `firmware/obc-fw-nrf54l/src/sd.rs:237-240`: `SD_MAX_DIRS = 4`,
   **`SD_MAX_FILES = 6`**, `SD_MAX_VOLUMES = 1`. The 6 is already committed:
   3 held mid-ride (map + route geometry + track log), 5 at the upload-commit
   peak. A DACH set is core + coarse + ~6 geometry = 8 handles, so this MUST be
   raised (16 is the suggested value: 8 shards + the 5-peak + margin). Measure
   and report the RAM delta per open-file slot in the PR body — embedded-sdmmc
   0.9's `FileInfo` is small, but the number belongs in the body.
   The fork is `timohueser/embedded-sdmmc-rs`, branch `cmd25-multiblock-write`,
   declared in **two** places: root `Cargo.toml:44` and
   `firmware/obc-fw-nrf54l/Cargo.toml:296`.
4. **obc-sim mounting a set (scope item 5).** `apps/obc-sim/src/main.rs` takes
   the map path as the first positional arg and `std::fs::read`s it into a
   `Vec<u8>` behind a `SliceSource`; `MapTables::parse` is at main.rs:987 /
   gui.rs:252. A `--set MS7.OBS` mode would read the manifest, `fs::read` each
   derived shard name beside it, and build a `MountedSet`.
5. **Verification not yet run**: workspace `cargo build --release` + `cargo test`;
   the excluded board crate (`cd firmware/obc-fw-nrf54l && cargo build --release`);
   four-step fmt; `cargo clippy --workspace --all-targets --all-features --locked
   -- -D warnings`.
6. **Doc sync**: `docs/content/` describes map storage and the registry — check
   `data-formats` and the architecture page once the registry work lands, and
   run `python3 docs/build_docs.py --check-links`.

## Gotcha for the next agent

Earlier in this session, edits intended for the worktree were applied to the
**shared checkout** at `/Users/timo/Documents/OSM` because a `cd` prefix moved
cargo/python out of the worktree. `firmware/obc-reader/src/{reader.rs,lib.rs}`
there were restored from `HEAD`, and `firmware/obc-reader/src/volume.rs` may
still exist as an untracked stray in the shared checkout — **delete it** if so.
Nothing was committed there. Run every command from the worktree root with no
`cd` prefix.

Do not touch `host/obcm-assemble` (a concurrent agent owns it), `host/obc-bake`,
or the cutter/catalog code. No upload/transfer-protocol changes — that is P3b-2
and the #889 branch owns it.
