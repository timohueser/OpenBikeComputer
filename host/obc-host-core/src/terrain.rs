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
//! 1. **The set's `terrain` role** (EL4) — a volume set's manifest names its terrain shard, and its
//!    derived filename is `MS<id>.OBD` (`OBCA_Spec.md` §5.2). Taken first when the path is a
//!    manifest, because the manifest is the authority on what belongs to the set: a `.OBD` sitting
//!    beside a manifest that does not name it is an **orphan** of a previous assembly (§5.4), and
//!    mounting it would draw a rider a profile from the map they replaced.
//! 2. **The sidecar** — `<map>.obcd` beside a single-file `.obcm` (`OBCT_Spec.md` §4.6). What the
//!    simulator's committed fixtures use, and what a side-loaded map on a card uses.
//!
//! The two agree by construction rather than by coincidence: `MS<id>.OBD` *is* the sidecar of
//! `MS<id>.OBS`, so the role lookup and the sidecar rule name one file, and the role lookup only
//! adds the two things the sidecar cannot know — whether the set claims a raster at all, and how
//! many bytes of one.

use std::path::{Path, PathBuf};

use obc_elevation::{TerrainElevation, DEFAULT_TILE_SLOTS};
use obc_formats::io::SliceSource;
use obc_formats::obcs;
use obc_route::{ElevationSource, NullElevation};

/// The terrain artifact's extension (`OBCT_Spec.md` §4.6 — `.obcd`, *not* `.obct`, which is the
/// recorded ride log).
pub(crate) const TERRAIN_EXT: &str = "obcd";

/// The sidecar path for a map file: the same path with [`TERRAIN_EXT`].
pub(crate) fn sidecar_path(map: &Path) -> PathBuf {
    map.with_extension(TERRAIN_EXT)
}

/// The terrain shard a **set manifest** names, if it names one: `(path, recorded bytes)`.
///
/// `None` for anything that is not an `MS<id>.OBS` manifest, for a manifest that does not validate,
/// and for a set with no `terrain` record — the last of which is a complete, ordinary map whose
/// profiles are flat (`OBCC_Spec.md` §13), not a failure.
fn manifest_terrain(path: &Path) -> Option<(PathBuf, u32)> {
    let name = path.file_name()?.to_str()?;
    let id = obcs::parse_manifest_name(name.as_bytes())?;
    let raw = std::fs::read(path).ok()?;
    let manifest = obcs::parse(&raw).ok()?;
    let record = manifest.terrain()?;
    let file = obcs::terrain_name(id)?;
    Some((path.with_file_name(file.as_str()), record.bytes))
}

/// The elevation source for a mounted map, from its **bytes**: parse them as an OBCT container, or
/// explain on stderr and hand back the null source. `what` names the file in that line.
///
/// The bytes are **leaked** on purpose. [`TerrainElevation`] samples straight out of the container
/// (that is the point — 512 B tiles on demand, ~2.1 KB resident), so it borrows its source for as
/// long as it lives, and a host mounts terrain once for a session exactly as it mounts the map.
/// The device has the same shape with none of the awkwardness: there the bytes are the SD card and
/// the source is a `'static` extent view.
pub(crate) fn mount(bytes: Vec<u8>, what: &str) -> Box<dyn ElevationSource> {
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

/// Resolve **the** terrain source for the map at `map_path` — a set's manifest `terrain` role if
/// the path is a manifest, else the `.obcd` sidecar, else the null source.
pub fn resolve(map_path: &Path) -> Box<dyn ElevationSource> {
    // A manifest answers the question completely: if it names no terrain record, the set has no
    // raster and any `.OBD` beside it is an orphan (§5.4) that must not be mounted.
    if let Some(name) = map_path.file_name().and_then(|n| n.to_str()) {
        if obcs::parse_manifest_name(name.as_bytes()).is_some() {
            let Some((path, recorded)) = manifest_terrain(map_path) else { return Box::new(NullElevation) };
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    eprintln!("terrain: cannot read {} ({e}); routes stay flat", path.display());
                    return Box::new(NullElevation);
                }
            };
            // §5.3's size check, which a device also runs at mount: a shard that is not the length
            // the manifest recorded is not the shard the manifest is about.
            if bytes.len() as u64 != recorded as u64 {
                eprintln!(
                    "terrain: ignoring {} — the manifest records {recorded} bytes and the file is {}; routes stay flat",
                    path.display(),
                    bytes.len()
                );
                return Box::new(NullElevation);
            }
            return mount(bytes, &path.display().to_string());
        }
    }
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

    /// The set path: the manifest decides. A `.OBD` beside a manifest that names no terrain record
    /// is an orphan of a previous assembly, and mounting it would draw a rider a profile from the
    /// map they replaced — the one failure mode the role lookup exists to close.
    #[test]
    fn a_set_takes_its_terrain_from_the_manifest_role_and_not_from_a_stray_file() {
        let dir = std::env::temp_dir().join(format!("obc-terrain-role-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let manifest_path = dir.join("MS7.OBS");
        let terrain_path = dir.join("MS7.OBD");

        let bbox = obcs::SetBBox { min_lat: 47_000_000, min_lon: 7_000_000, max_lat: 48_000_000, max_lon: 9_000_000 };
        // Bound (§5.2): this resolver reaches the raster by the sidecar name, not by id, but a
        // manifest an assembler has not bound is a shape a card never holds.
        let core = obcs::Shard { role: obcs::Role::Core, bbox, bytes: 128, object_id: 1 };
        let write = |parts: &[obcs::Shard]| {
            let m = obcs::build(obc_formats::obcm::VERSION, 0, 1, bbox, [0; 16], [0xFF; 24], parts).expect("manifest");
            let digests = vec![[0u8; 32]; parts.len()];
            let mut out = vec![0u8; obcs::MAX_MANIFEST_LEN];
            let len = obcs::serialize(&m, &digests, &mut out).expect("serialize");
            std::fs::write(&manifest_path, &out[..len]).expect("write manifest");
        };

        // A raster on disk that the manifest does not claim: not mounted.
        std::fs::write(&terrain_path, b"whatever this used to be").expect("write orphan");
        write(&[core]);
        assert_eq!(resolve(&manifest_path).sample(47_000_000, 8_000_000), None);

        // …and one it does claim, at the wrong length: also not mounted, and never a fault.
        write(&[core, obcs::Shard { role: obcs::Role::Terrain, bbox, bytes: 999_999, object_id: 2 }]);
        assert_eq!(resolve(&manifest_path).sample(47_000_000, 8_000_000), None);

        let _ = std::fs::remove_dir_all(&dir);
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
