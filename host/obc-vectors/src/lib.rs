//! Builders for the shared S0 wire-protocol test vectors (`specs/vectors/`).
//!
//! Each function constructs one fixture directly from the spec text
//! (`obc-ble-interface-spec.md` / `OBCR_Spec.md`), independently of the production
//! codecs on either side. The checked-in fixture files are these builders' output;
//! `tests/vectors.rs` asserts they haven't drifted.
//!
//! **Two documented exceptions**, both conversion *outputs* rather than wire layouts:
//! [`build_route`] runs the real `gpx_to_obcr` and [`track_export_gpx`] the real
//! `track_to_gpx`, because neither serialization has a spec to rebuild from — the converter
//! *is* the contract. Those two fixtures therefore pin **agreement**, not correctness: a bug
//! in the converter moves the fixture with it. What they catch is a second implementation
//! drifting from the first, which is exactly their job — the iOS OBCR encoder and the
//! browser's wasm bridge are both held to these bytes.
//!
//! Regenerate after a deliberate spec change with:
//!
//! ```text
//! cargo run -p obc-vectors --example regenerate --locked
//! ```

use std::path::PathBuf;

use obc_formats::io::{ByteSink, Error, SliceSource};
use obc_route::gpx_to_obcr;

pub mod landmarks;
pub mod peaks;

/// The `specs/vectors/` directory at the repo root.
pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors")
}

/// CRC-32/IEEE per spec §6: reflected, poly `0xEDB88320`, init/xorout `0xFFFFFFFF`.
/// Check value: `crc32(b"123456789") == 0xCBF43926`.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// A service record written directly from the OBCM v15 field table.
/// A Train place with an explicit node approach, independent of the production encoder.
pub fn place_record() -> Vec<u8> {
    let mut bytes = vec![0xff; 64];
    bytes[0..4].copy_from_slice(&46_561_323i32.to_le_bytes());
    bytes[4..8].copy_from_slice(&8_361_496i32.to_le_bytes());
    bytes[8] = 20;
    bytes[9] = 7;
    bytes[10..17].copy_from_slice(b"Station");
    bytes[34..36].copy_from_slice(&0xffffu16.to_le_bytes());
    bytes[36..44].copy_from_slice(&((1u64 << 62) | 123).to_le_bytes());
    bytes[44..52].copy_from_slice(&((1u64 << 62) | 456).to_le_bytes());
    bytes[52..56].copy_from_slice(&46_561_320i32.to_le_bytes());
    bytes[56..60].copy_from_slice(&8_361_490i32.to_le_bytes());
    bytes[60] = 5;
    bytes[61..64].fill(0);
    bytes
}

/// The deterministic route source: a short rolling track at 48°N with two `<wpt>`
/// waypoints listed out of ride order (as GPX carries them). One carries a `<sym>` the
/// converter maps (`Drinking Water` → Water), the other a `<type>` it doesn't (`Viewpoint` →
/// generic) — so the fixture pins both halves of the symbol mapping.
pub const ROUTE_GPX: &str = include_str!("route-source.gpx");

/// Route fixture name (also the OBCR header name field).
pub const ROUTE_NAME: &str = "Vector Loop";

/// `ROUTE_GPX` with its `<wpt>` elements removed — the same track, no waypoints.
pub fn route_gpx_plain() -> String {
    ROUTE_GPX.lines().filter(|l| !l.trim_start().starts_with("<wpt ")).fold(String::new(), |mut s, l| {
        s.push_str(l);
        s.push('\n');
        s
    })
}

/// An in-memory [`ByteSink`] for the streaming converters below.
struct VecSink(Vec<u8>);

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.0[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

/// Convert a GPX string to OBCR v3 bytes via the reference converter.
pub fn build_route(gpx: &str) -> Vec<u8> {
    let mut sink = VecSink(Vec::new());
    gpx_to_obcr(&SliceSource(gpx.as_bytes()), ROUTE_NAME, &mut sink).unwrap();
    sink.0
}

/// Recorded-track fixture name — carries an `&` so the GPX export's XML escaping is pinned too.
pub const TRACK_NAME: &str = "Schauinsland & back";

/// A recorded `.obct` ride log: a flat array of 20-byte records, **no header**
/// (`obc-formats/src/track.rs`, the byte authority). Built field-by-field from that layout rather
/// than through `encode_record`, so the fixture pins the record independently of the production
/// codec — the same rule the rest of this module follows.
///
/// Shaped for **codec coverage, not plausibility** (it teleports between hemispheres): five points
/// spanning signed coordinates, segment flags, and sensor sentinel combinations —
///
/// | # | why it is here |
/// | :-- | :-- |
/// | 0 | first point (always opens a `<trkseg>`), all three sensor fields present |
/// | 1 | cadence absent — the `TrackPointExtension` wrapper still appears, one element short |
/// | 2 | every sensor absent — no `<extensions>` block at all |
/// | 3 | `segment_start` after a pause (a second `<trkseg>`), negative lat/lon/elevation, and **power only** (no wrapper) |
/// | 4 | zeroes everywhere: `0.000000` coordinate formatting, and `hr`/`cad`/`pwr` = 0 as real values, distinct from the `0xFF`/`0xFFFF` absent sentinels |
pub fn track_log() -> Vec<u8> {
    /// One record's fields, named after the layout they serialize into. `0xFF` / `0xFFFF` in the
    /// sensor fields are the "absent" sentinels.
    struct Rec {
        lon: i32,
        lat: i32,
        ele: i16,
        flags: u16,
        t_ms: u32,
        hr: u8,
        cad: u8,
        pwr: u16,
    }
    let rec = |lon, lat, ele, flags, t_ms, hr, cad, pwr| Rec { lon, lat, ele, flags, t_ms, hr, cad, pwr };
    let points = [
        rec(7_842_000, 47_995_000, 300, 1, 0, 132, 78, 185),
        rec(7_843_500, 47_996_000, 305, 0, 1_000, 138, 0xFF, 190),
        rec(7_845_000, 47_997_200, 318, 0, 2_000, 0xFF, 0xFF, 0xFFFF),
        rec(-122_419_400, -37_774_900, -12, 1, 63_000, 0xFF, 0xFF, 240),
        rec(0, 0, 0, 0, 64_000, 0, 0, 0),
    ];
    let mut v = Vec::with_capacity(points.len() * 20);
    for p in points {
        v.extend_from_slice(&p.lon.to_le_bytes()); // 0..4
        v.extend_from_slice(&p.lat.to_le_bytes()); // 4..8
        v.extend_from_slice(&p.ele.to_le_bytes()); // 8..10
        v.extend_from_slice(&le16(p.flags)); // 10..12 — bit 0 = segment_start
        v.extend_from_slice(&le32(p.t_ms)); // 12..16
        v.push(p.hr); // 16
        v.push(p.cad); // 17
        v.extend_from_slice(&le16(p.pwr)); // 18..20
    }
    v
}

/// The GPX 1.1 export of [`ride_v3`], through the production converter (`track_to_gpx`).
///
/// Unlike the binary fixtures there is no independent spec to rebuild this from — the exporter's
/// serialization *is* the contract — so this goes through the real code, exactly like
/// [`build_route`] does for OBCR. Its value is cross-implementation: the browser bridge
/// (`obc-web-convert`, compiled to wasm) must reproduce these bytes character-for-character.
pub fn track_export_gpx() -> Vec<u8> {
    let mut sink = VecSink(Vec::new());
    obc_route::track_to_gpx(&SliceSource(&ride_v3()), TRACK_NAME, &mut sink).unwrap();
    sink.0
}

fn le16(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}
fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// Ride object v3: three exact 20-byte recorded samples followed by the fixed 84-byte footer.
/// Built field-by-field from the specification rather than through the production codec.
pub fn ride_v3() -> Vec<u8> {
    let mut v = Vec::new();
    // lon µdeg, lat µdeg, ele m, flags, t_ms, hr, cadence, power.
    for (lon, lat, ele, flags, t_ms, hr, cad, pwr) in [
        (7_800_000i32, 48_000_000i32, 214i16, 1u16, 0u32, 140u8, 84u8, 205u16),
        (7_801_200, 48_001_000, 219, 0, 60_000, 0xFF, 0xFF, 0xFFFF),
        (7_803_000, 48_002_000, 225, 1, 120_000, 150, 0xFF, 215),
    ] {
        v.extend_from_slice(&lon.to_le_bytes());
        v.extend_from_slice(&lat.to_le_bytes());
        v.extend_from_slice(&ele.to_le_bytes());
        v.extend_from_slice(&flags.to_le_bytes());
        v.extend_from_slice(&t_ms.to_le_bytes());
        v.push(hr);
        v.push(cad);
        v.extend_from_slice(&pwr.to_le_bytes());
    }

    let name = b"Sensor Ride";
    v.extend_from_slice(b"OBRF");
    v.push(3); // version
    v.push(name.len() as u8);
    v.extend_from_slice(&le16(84)); // fixed footer length
    v.extend_from_slice(&le32(1_751_460_000)); // start_time
    v.extend_from_slice(&le32(12_345)); // distance m
    v.extend_from_slice(&le32(3_600)); // moving_time s
    v.extend_from_slice(&le16(343)); // avg_speed cm/s
    v.extend_from_slice(&le16(120)); // climb m
    v.extend_from_slice(&le32(3)); // point_count
    v.push(142); // avg_hr
    v.push(176); // max_hr
    v.push(85); // avg_cad
    v.push(0); // reserved
    v.extend_from_slice(&le16(210)); // avg_pwr
    v.extend_from_slice(&le16(480)); // max_pwr
    v.extend_from_slice(name);
    v.resize(3 * 20 + 84, 0); // fixed 48-byte name slot
    v
}

// OBCT terrain (OBCT_Spec.md)

/// The terrain fixture's posting and cell size as `log2(µdeg)`. The posting is the v1 value
/// (`2^9`); the **cell is deliberately not** — a v1 `2^19` cell is 2 MiB of raster, and the whole
/// point of both being header data (spec §1.3) is that a small one is equally legal. At `2^14` a
/// cell is 32 × 32 samples = 2 × 2 tiles, so the fixture still exercises tile addressing inside a
/// cell, cross-cell fetch, and a hole — in 6 KB.
pub const TERRAIN_POSTING_LOG2: u8 = 9;
pub const TERRAIN_CELL_LOG2: u8 = 14;
/// The fixture's cell rectangle: 2 × 2 cells with their minimum corner at ≈ 46.972928°N,
/// 7.979008°E (the Bernese Oberland, so a transposed lat/lon lands in the sea and is noticed).
pub const TERRAIN_CELL_MIN_I: u32 = 19_251;
pub const TERRAIN_CELL_MIN_J: u32 = 16_871;
pub const TERRAIN_ROWS: u16 = 2;
pub const TERRAIN_COLS: u16 = 2;
/// The cell missing from the rectangle, as an offset from its minimum corner — the fixture's hole,
/// pinning the `0` directory sentinel and the "a query in a hole is uncovered" rule (§5.3).
pub const TERRAIN_ABSENT_CELL: (u32, u32) = (1, 1);
/// The one `NODATA` sample, as a lattice offset from the rectangle's base sample (§5.4).
pub const TERRAIN_NODATA_AT: (u32, u32) = (40, 5);

/// The fixture's surface: a **plane** `100 + 3·di + 5·dj` metres over the lattice offsets from the
/// rectangle's base sample, with one [`TERRAIN_NODATA_AT`] void.
///
/// A plane because it is the one surface whose bilinear interpolation has a closed form
/// independent of the interpolator — a second implementation can check itself against arithmetic
/// rather than against a reference table. The coefficients differ (3 vs 5) so a transposed
/// latitude/longitude cannot pass.
pub fn terrain_height(di: u32, dj: u32) -> i16 {
    if (di, dj) == TERRAIN_NODATA_AT {
        return i16::MIN; // obc_formats::obct::NODATA, written out per this crate's rules
    }
    (100 + 3 * di as i32 + 5 * dj as i32) as i16
}

/// The µdeg coordinate of lattice offset `d` on either axis, from the rectangle's base sample.
pub fn terrain_coord(base_cell: u32, d: u32) -> i32 {
    let base_sample = (base_cell as i64) << (TERRAIN_CELL_LOG2 - TERRAIN_POSTING_LOG2);
    (-(1i64 << 28) + ((base_sample + d as i64) << TERRAIN_POSTING_LOG2)) as i32
}

/// A full **OBCT terrain shard** (`OBCT_Spec.md` §4): the 32-byte header, the row-major `uint32`
/// offset directory over the 2 × 2 cell rectangle (`0` = the absent cell), then the three present
/// cell blocks — each 2 × 2 tiles of 16 × 16 `int16` metres, row-major with rows advancing latitude.
///
/// Built straight from the spec's field tables, independent of the `obc-elevation` reader that
/// parses it from the other side. Length = `32 + 16 + 3 × 2048` = 6192 bytes.
pub fn terrain_shard() -> Vec<u8> {
    terrain_container(
        TERRAIN_POSTING_LOG2,
        TERRAIN_CELL_LOG2,
        TERRAIN_CELL_MIN_I,
        TERRAIN_CELL_MIN_J,
        TERRAIN_ROWS,
        TERRAIN_COLS,
        &|ci, cj| (ci, cj) != TERRAIN_ABSENT_CELL,
        &terrain_height,
    )
}

/// The general OBCT §4 container writer behind [`terrain_shard`], for a caller that needs terrain
/// somewhere *else* — the packer's `--terrain` tests bake ascent over a synthetic surface at their
/// own coordinates, and moving the pinned spec fixture to suit them would defeat its purpose.
///
/// `present(ci, cj)` decides which cells of the rectangle get a block (a `false` leaves the
/// directory's absent sentinel); `height(di, dj)` is the surface, indexed by **lattice offsets from
/// the rectangle's base sample** exactly like [`terrain_height`]. Still written straight from the
/// spec's field tables — this is one implementation with a parameterised surface, not a second one.
#[allow(clippy::too_many_arguments)]
pub fn terrain_container(
    posting_log2: u8,
    cell_log2: u8,
    cell_min_i: u32,
    cell_min_j: u32,
    rows: u16,
    cols: u16,
    present: &dyn Fn(u32, u32) -> bool,
    height: &dyn Fn(u32, u32) -> i16,
) -> Vec<u8> {
    let samples = 1u32 << (cell_log2 - posting_log2); // samples per cell edge
    let tiles = samples / 16; // tiles per cell edge
    let dir_len = rows as usize * cols as usize * 4;

    let mut header = [0u8; 32];
    header[0..4].copy_from_slice(b"OBCT");
    header[4] = 1; // version
    header[5] = posting_log2;
    header[6] = cell_log2;
    header[7] = 0; // flags — v1 defines none
    header[8..12].copy_from_slice(&cell_min_i.to_le_bytes());
    header[12..16].copy_from_slice(&cell_min_j.to_le_bytes());
    header[16..18].copy_from_slice(&rows.to_le_bytes());
    header[18..20].copy_from_slice(&cols.to_le_bytes());
    header[20..24].copy_from_slice(&le32(32)); // the directory follows the header
                                               // 24..32 reserved (0)

    let mut directory = vec![0u8; dir_len];
    let mut blocks: Vec<u8> = Vec::new();
    for ci in 0..rows as u32 {
        for cj in 0..cols as u32 {
            if !present(ci, cj) {
                continue; // leave the slot at the absent sentinel
            }
            let slot = (ci as usize * cols as usize + cj as usize) * 4;
            let offset = (32 + dir_len + blocks.len()) as u32;
            directory[slot..slot + 4].copy_from_slice(&le32(offset));
            for ti in 0..tiles {
                for tj in 0..tiles {
                    for r in 0..16u32 {
                        for c in 0..16u32 {
                            let di = ci * samples + ti * 16 + r;
                            let dj = cj * samples + tj * 16 + c;
                            blocks.extend_from_slice(&height(di, dj).to_le_bytes());
                        }
                    }
                }
            }
        }
    }

    let mut v = Vec::with_capacity(32 + dir_len + blocks.len());
    v.extend_from_slice(&header);
    v.extend_from_slice(&directory);
    v.extend_from_slice(&blocks);
    v
}

/// Config object v1 (spec §7.3): name "OBC Tourer", metric.
pub fn config_v1() -> Vec<u8> {
    let name = b"OBC Tourer";
    let mut v = Vec::new();
    v.extend_from_slice(&le16(name.len() as u16));
    v.extend_from_slice(name);
    v.push(0); // units: metric
    v
}

/// The `fw_version` string carried in the OBCU update-container fixture — a
/// realistic `git describe` value the iOS picker + device DIS both display.
pub const UPDATE_FW_VERSION: &str = "1.2.0+abc1234";

/// The deterministic raw application image inside the OBCU container fixture: a
/// 128-byte body whose first 32-bit word is a plausible Cortex-M initial stack
/// pointer (`0x2002_0000`, inside the nRF54L15 DK RAM — see
/// `obc_dfu::looks_like_vector_table`), then a byte ramp. Content is opaque to
/// the transfer layer; it exists so the fixture exercises the image CRC too.
pub fn update_raw_image() -> Vec<u8> {
    let mut v = Vec::with_capacity(128);
    v.extend_from_slice(&le32(0x2002_0000)); // plausible initial SP (vector-table-first)
    for i in 4u32..128 {
        v.push((i & 0xFF) as u8);
    }
    v
}

/// A full **OBCU update container** (`OBCU_Spec.md` §1, `UPDATE.BIN`): the fixed
/// 64-byte header (magic `OBCU`, version 1, raw-image length + CRC-32, NUL-padded
/// `fw_version`, header CRC-32 over bytes `0..60`) followed by [`update_raw_image`].
/// Built straight from the spec's field table — independent of the `obc-dfu`
/// production codec, which pins the same bytes from the other side. The iOS
/// companion's `OBCUHeader` decoder validates this file identically.
pub fn update_container_v1() -> Vec<u8> {
    let image = update_raw_image();
    let mut header = [0u8; 64];
    header[0..4].copy_from_slice(b"OBCU");
    header[4..6].copy_from_slice(&le16(1)); // header_version
                                            // 6..8 reserved (0)
    header[8..12].copy_from_slice(&le32(image.len() as u32));
    header[12..16].copy_from_slice(&le32(crc32(&image)));
    let vbytes = UPDATE_FW_VERSION.as_bytes();
    header[16..16 + vbytes.len()].copy_from_slice(vbytes); // NUL-padded to 32
                                                           // 48..60 reserved (0) — future signature-scheme marker
    let hcrc = crc32(&header[..60]);
    header[60..64].copy_from_slice(&le32(hcrc));

    let mut v = Vec::with_capacity(64 + image.len());
    v.extend_from_slice(&header);
    v.extend_from_slice(&image);
    v
}

/// A full signed OBCU v2 update container (`OBCU_Spec.md` §1): the same
/// 64-byte header table as [`update_container_v1`] — *including* `header_version` still `1`, the
/// flash-once-bootloader guarantee — with v1's reserved bytes `48..52` now carrying the
/// signature-scheme marker (`sig_scheme` = 1 · `sig_len` = 64), followed by [`update_raw_image`]
/// and a **64-byte Ed25519 signature trailer**.
///
/// The header is hand-built from the spec's field table, like every other fixture here. The
/// signature is the one part that cannot be: it comes from `obc_dfu::sign_image` over the
/// domain-separated message of §1.3 (`"OBCUv2-sig\0" ‖ fw_version[32] ‖ image_len ‖ image`), signed
/// with the **committed test key** (`firmware/obc-dfu/keys/test/`). That signing is deterministic
/// (fixed zero noise), so this fixture is a stable file — a nondeterministic signer would re-cut it
/// on every regeneration, which is exactly why `obc_dfu::sig` pins determinism with its own test.
pub fn update_container_v2() -> Vec<u8> {
    let image = update_raw_image();
    let mut header = [0u8; 64];
    header[0..4].copy_from_slice(b"OBCU");
    header[4..6].copy_from_slice(&le16(1)); // header_version — still 1 in a v2 container
                                            // 6..8 reserved (0)
    header[8..12].copy_from_slice(&le32(image.len() as u32));
    header[12..16].copy_from_slice(&le32(crc32(&image)));
    let vbytes = UPDATE_FW_VERSION.as_bytes();
    header[16..16 + vbytes.len()].copy_from_slice(vbytes); // NUL-padded to 32
    header[48..50].copy_from_slice(&le16(1)); // sig_scheme = Ed25519
    header[50..52].copy_from_slice(&le16(64)); // sig_len
                                               // 52..60 still reserved (0)
    let hcrc = crc32(&header[..60]);
    header[60..64].copy_from_slice(&le32(hcrc));

    // The signed message's own layout is asserted byte-for-byte in `obc-dfu`'s
    // `signed_message_is_the_spec_bytes`; here we only need the resulting 64 bytes.
    let decoded = obc_dfu::ImageHeader::decode(&header).expect("the hand-built header decodes");
    let signature = obc_dfu::sign_image(&obc_dfu::sig::test_key::SEED, &decoded, &image);

    let mut v = Vec::with_capacity(64 + image.len() + signature.len());
    v.extend_from_slice(&header);
    v.extend_from_slice(&image);
    v.extend_from_slice(&signature);
    v
}

/// `status` message `commandResult` (spec §4.3): 4 bytes.
pub fn status_command_result(cmd: u8, status: u8, detail: u8) -> Vec<u8> {
    vec![3u8, cmd, status, detail]
}

/// The `setClock` command write (spec §4.4, cmd 5): `cmd u8 = 5 · utc u32 LE ·
/// offset_min i16 LE`. 7 bytes.
pub fn command_set_clock(utc: u32, offset_min: i16) -> Vec<u8> {
    let mut v = vec![5u8];
    v.extend_from_slice(&le32(utc));
    v.extend_from_slice(&offset_min.to_le_bytes());
    v
}

/// Trip fixture name (also the trip object header name field).
pub const TRIP_NAME: &str = "Alpen Traverse";

/// The full-width trip key in `trip-v3.bin`; every byte differs, so a swapped byte order shows.
pub const TRIP_KEY: u64 = 0x0123_4567_89AB_CDEF;

/// The start date in `trip-v3.bin`: 2025-09-29, a Monday, in days since 1970-01-01.
pub const TRIP_START_DATE: u16 = 20_360;

/// The days in `trip-v3.bin` as `(route, join_m, leave_m)`: a day that ends on the line at its
/// route end, a day that leaves the line 400 m before its stop, and a day that joins the line
/// after 400 m and ends on it (`leave_m` past the route end). The third route id is full width.
pub const TRIP_DAYS: [(u64, u32, u32); 3] = [(7, 0, 82_000), (8, 0, 73_600), (0x1_0000_0063, 400, u32::MAX)];

/// Trip object v3 (spec §7.7): a 64-byte header (`version 3 · day_count u16 · name ≤ 48 ·
/// start_date u16 · trip_key u64`) followed by `day_count` 16-byte day records
/// (`route u64 · join_m u32 · leave_m u32`). Length = `64 + 16·day_count`.
pub fn trip_v3(key: u64, name: &str, start_date: u16, days: &[(u64, u32, u32)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(3); // version
    v.push(0); // reserved
    v.extend_from_slice(&le16(days.len() as u16)); // day_count
    v.push(name.len() as u8); // name_len
    let mut padded = [0u8; 48];
    padded[..name.len()].copy_from_slice(name.as_bytes());
    v.extend_from_slice(&padded); // name, zero-padded to 48
    v.push(0); // reserved
    v.extend_from_slice(&le16(start_date));
    v.extend_from_slice(&key.to_le_bytes());
    assert_eq!(v.len(), 64, "trip object header is 64 bytes");
    for &(route, join_m, leave_m) in days {
        v.extend_from_slice(&route.to_le_bytes()); // full-width flat-store ObjectId, ride order
        v.extend_from_slice(&le32(join_m));
        v.extend_from_slice(&le32(leave_m));
    }
    v
}

/// Valid visit envelope and two overlaps whose shared bytes are otherwise valid.
fn visit_envelopes(plain: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    use obc_formats::io::{put_u16, put_u32, rd_u32};
    let offset = plain.len() as u32;
    let mut valid = plain.to_vec();
    let mut descriptor = [0; 80];
    descriptor[..16].fill(1);
    descriptor[16..24].copy_from_slice(&2u64.to_le_bytes());
    descriptor[24..32].copy_from_slice(&3u64.to_le_bytes());
    for base in [32, 44] {
        put_u32(&mut descriptor, base + 4, 1);
        put_u32(&mut descriptor, base + 8, 2);
    }
    descriptor[56..64].copy_from_slice(&123u64.to_le_bytes());
    descriptor[72] = 1;
    valid.extend_from_slice(&descriptor);
    valid[118] = 1;
    put_u32(&mut valid, 120, offset);
    put_u32(&mut valid, 124, 80);

    // The last four reserved zeros also encode a waypoint's zero distance.
    let mut waypoint = valid.clone();
    let mut record = [0; 80];
    record[15] = 1;
    record[20] = b'W';
    waypoint.extend_from_slice(&record[4..]);
    put_u32(&mut waypoint, 112, offset + 76);
    put_u16(&mut waypoint, 116, 1);

    // The same zeros also encode a valid index bbox minimum longitude.
    let mut index = valid.clone();
    let old_index = rd_u32(plain, 56) as usize;
    let index_len = rd_u32(plain, 52) as usize * obc_formats::obcr::CHUNK_META_LEN;
    index.extend_from_slice(&plain[old_index + 4..old_index + index_len]);
    put_u32(&mut index, 56, offset + 76);
    vec![
        ("route-visit.obcr", valid),
        ("route-visit-waypoint-overlap.obcr", waypoint),
        ("route-visit-index-overlap.obcr", index),
    ]
}

/// Every fixture as `(file name, bytes)`.
pub fn all() -> Vec<(&'static str, Vec<u8>)> {
    let route_wp = build_route(ROUTE_GPX);
    let route_plain = build_route(&route_gpx_plain());
    let trip = trip_v3(TRIP_KEY, TRIP_NAME, TRIP_START_DATE, &TRIP_DAYS);
    let terrain = terrain_shard();
    let envelopes = visit_envelopes(&route_plain);
    let mut fixtures = vec![
        ("route-waypoints.obcr", route_wp),
        ("route-plain.obcr", route_plain),
        // The sample-codec fixture remains a codec vector only. GPX export is pinned from the
        // finished ride-v3 object; headerless sample arrays are not accepted as rides.
        ("track-log.obct", track_log()),
        ("track-export.gpx", track_export_gpx()),
        // The OBCT terrain shard (`OBCT_Spec.md`): a 2 × 2 cell rectangle with a hole
        // and a NODATA sample, over a plane. Not a wire layout — a *storage* one, like
        // `track-log.obct` beside it — and it is here because three implementations will sample it
        // (the device, the `obc-dem` baker's cross-check, and eventually the browser), and the
        // spec's guarantee is that they agree bit-for-bit on the same coordinate.
        ("terrain-shard.obcd", terrain.clone()),
        ("ride-v3.bin", ride_v3()),
        ("config-v1.bin", config_v1()),
        ("place-train-v15.bin", place_record()),
        ("landmark-section-v16.bin", landmarks::section()),
        ("peak-section-v17.bin", peaks::section()),
        // The answer to an accepted installFw: ok, no detail. Pins the four-byte layout.
        ("status-command-result.bin", status_command_result(3, 0, 0)),
        // The phone's clock stamp (cmd 5): 2026-07-09T12:00:00Z (unix 1783598400),
        // +02:00 (offset 120 min). 7 bytes.
        ("command-set-clock.bin", command_set_clock(1_783_598_400, 120)),
        ("update-container-v1.bin", update_container_v1()),
        // The signed OBCU v2 container (spec §1): the same header table and the
        // same 128-byte image, plus the scheme marker in v1's reserved bytes and a 64-byte Ed25519
        // trailer under the committed test key. Kept alongside v1 rather than replacing it: v1 is
        // still what a fielded bootloader and the device's own rollback snapshot look like, and the pair
        // is what pins the offset-compatibility guarantee across implementations.
        ("update-container-v2.bin", update_container_v2()),
        // A trip (§7.7): three days that pin every header and day field.
        ("trip-v3.bin", trip),
    ];
    fixtures.extend(envelopes);
    fixtures
}
