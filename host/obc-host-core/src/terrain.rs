//! Where a host's terrain comes from (EL7, epic #1068) — **the** place a host answers "what is the
//! elevation source for this map?".
//!
//! Terrain is an *enhancement*, never a requirement: a map with no terrain beside it plans, renders
//! and rides exactly as before, with [`NullElevation`] in the seam. So every failure here — no such
//! file, unreadable, not an OBCT container, a container whose arithmetic doesn't close — degrades
//! to the null source with one line of explanation. None of them is a fault, deliberately outside
//! the NO MAP / MAP UNREADABLE honesty rules (#1042): those are about a rider whose *map* is
//! missing, and this file's absence takes nothing away.
//!
//! Two resolution paths, one function ([`resolve`]):
//!
//! 1. **The sidecar** — `<map>.obcd` beside a single-file `.obcm` (`OBCT_Spec.md` §4.6). What the
//!    simulator's committed fixtures use, and what a side-loaded map on a card uses.
//! 2. **The set's `terrain` role** — a published volume set names its terrain shard in the
//!    manifest. That role arrives with EL4; until it does, [`resolve`] takes the sidecar path for a
//!    set too (its manifest's own stem), so a set gains terrain the moment EL4 lands without any
//!    caller changing.

use std::path::{Path, PathBuf};

use obc_elevation::{TerrainElevation, DEFAULT_TILE_SLOTS};
use obc_formats::io::SliceSource;
use obc_route::{ElevationSource, NullElevation};

/// The terrain artifact's extension (`OBCT_Spec.md` §4.6 — `.obcd`, *not* `.obct`, which is the
/// recorded ride log).
pub const TERRAIN_EXT: &str = "obcd";

/// The sidecar path for a map file: the same path with [`TERRAIN_EXT`].
pub fn sidecar_path(map: &Path) -> PathBuf {
    map.with_extension(TERRAIN_EXT)
}

/// The elevation source for a mounted map, from its **bytes**: parse them as an OBCT container, or
/// explain on stderr and hand back the null source. `what` names the file in that line.
///
/// The bytes are **leaked** on purpose. [`TerrainElevation`] samples straight out of the container
/// (that is the point — 512 B tiles on demand, ~2.1 KB resident), so it borrows its source for as
/// long as it lives, and a host mounts terrain once for a session exactly as it mounts the map.
/// The device has the same shape with none of the awkwardness: there the bytes are the SD card and
/// the source is a `'static` extent view.
pub fn mount(bytes: Vec<u8>, what: &str) -> Box<dyn ElevationSource> {
    let src: &'static SliceSource<'static> = Box::leak(Box::new(SliceSource(Box::leak(bytes.into_boxed_slice()))));
    match TerrainElevation::<'static, DEFAULT_TILE_SLOTS>::parse(src) {
        Ok(terrain) => {
            let h = terrain.reader().header();
            let (min_lat, min_lon, max_lat, max_lon) = h.bbox_udeg();
            eprintln!(
                "terrain: {what} | posting 2^{} µdeg | {}×{} cell(s) | bbox {min_lon},{min_lat} .. {max_lon},{max_lat}",
                h.posting_log2, h.cell_rows, h.cell_cols
            );
            Box::new(terrain)
        }
        Err(e) => {
            eprintln!("terrain: ignoring {what} — not a usable OBCT container ({e:?}); routes stay flat");
            Box::new(NullElevation)
        }
    }
}

/// Resolve **the** terrain source for the map at `map_path`: read its sidecar if one is there,
/// else the null source. Reading is the caller's only I/O; everything else is policy (above).
pub fn resolve(map_path: &Path) -> Box<dyn ElevationSource> {
    let path = sidecar_path(map_path);
    match std::fs::read(&path) {
        Ok(bytes) => mount(bytes, &path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Box::new(NullElevation),
        Err(e) => {
            eprintln!("terrain: cannot read {} ({e}); routes stay flat", path.display());
            Box::new(NullElevation)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sidecar_is_the_map_path_with_the_obct_extension() {
        assert_eq!(sidecar_path(Path::new("/maps/grimsel.obcm")), PathBuf::from("/maps/grimsel.obcd"));
        assert_eq!(sidecar_path(Path::new("MS7.OBS")), PathBuf::from("MS7.obcd"));
    }

    /// The whole degrade-never-fault rule in one test: a missing file and a corrupt one both leave
    /// the caller with a working source that simply has no heights.
    #[test]
    fn a_missing_or_corrupt_terrain_file_degrades_to_the_null_source() {
        let mut missing = resolve(Path::new("/definitely/not/here.obcm"));
        assert_eq!(missing.sample(47_000_000, 8_000_000), None);
        let mut corrupt = mount(b"not an OBCT file at all, not even close".to_vec(), "corrupt.obcd");
        assert_eq!(corrupt.sample(47_000_000, 8_000_000), None);
    }
}
