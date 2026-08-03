//! `--terrain`: the packer's host-side view of baked OBCT tiles (epic #1068 EL5).
//!
//! This module owns **only** the plumbing — finding the `.obcd` files, opening them, and routing a
//! query to the one that covers it. Every byte of policy (what a malformed container is, how a
//! sample interpolates, what a hole answers) belongs to `obc-elevation`, which is the same `no_std`
//! code the device runs. That split is epic #1068's "one sampling truth": the packer must not be
//! able to sample terrain differently from the firmware, so it does not get its own sampler.
//!
//! What the packer accepts is deliberately layout-agnostic: `--terrain` may name a single container
//! (a shard covering a whole region, or one cell) **or** a directory, which is scanned recursively
//! for `*.obcd`. Nothing here parses a path into cell indices — a container states its own
//! rectangle in its header (`OBCT_Spec.md` §4.2), so the directory tree can be `<i>/<j>.obcd`, a
//! flat dump, or whatever the bakery settles on, and this code keeps working.
//!
//! **Threading.** [`TerrainSet`] is immutable and `Sync`; a [`TerrainSampler`] borrows it and is the
//! `&mut` thing an [`ElevationSource`] must be. The cell cutter builds one sampler per cell inside
//! its rayon map, which is also where the bbox filter pays for itself: a cell only opens the
//! containers its own square touches.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use obc_elevation::{ElevationSource, TerrainReader, TileCache};
use obc_formats::io::{ByteSource, Error};

/// Resident tiles per open container while packing: 64 × 512 B = 32 KB.
///
/// The device runs four (a bilinear query straddles at most four tiles). A host has no such budget
/// and a very different access pattern — the packer sweeps thousands of edges, each a run of samples
/// walking a line — so a deeper cache turns a whole edge into a handful of reads instead of one per
/// sample. Still small enough that a hundred open containers cost 3 MB.
const HOST_TILE_SLOTS: usize = 64;

/// One discovered OBCT container: its path, an open handle, and the µdeg rectangle its header
/// claims.
struct TerrainFile {
    path: PathBuf,
    src: FileSource,
    /// `(min_lat, min_lon, max_lat, max_lon)` µdeg, half-open on the max edges (`OBCT_Spec.md`
    /// §4.2). `i64` because the world box legally overhangs ±90/±180.
    bbox: (i64, i64, i64, i64),
    /// `log2` of this container's sample posting in µdeg (`OBCT_Spec.md` §4.2).
    posting_log2: u8,
}

/// The `--terrain` input: every OBCT container the operator pointed at, opened and header-validated,
/// with the rectangle each one covers.
///
/// Building one is eager and cheap (a header read and a directory validation per file) and it is
/// where a bad input is reported — a run must not get halfway through a bake before discovering that
/// the terrain it was handed is not terrain.
pub struct TerrainSet {
    files: Vec<TerrainFile>,
}

impl TerrainSet {
    /// Open `path`: a single `.obcd` container, or a directory scanned recursively for them.
    ///
    /// Files are sorted by path so the set — and therefore which container answers a query that two
    /// overlapping containers could both answer — is identical on every machine. Determinism of the
    /// packed bytes is the whole contract here.
    pub fn open(path: &Path) -> Result<TerrainSet, String> {
        let mut paths = Vec::new();
        if path.is_dir() {
            collect_obcd(path, &mut paths)?;
            if paths.is_empty() {
                return Err(format!("--terrain {}: no .obcd containers under this directory", path.display()));
            }
        } else {
            paths.push(path.to_path_buf());
        }
        paths.sort();

        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let src = FileSource::open(&path)?;
            let (bbox, posting_log2) = {
                // Parse once for validation and for the rectangle. The reader is rebuilt per sampler
                // — it borrows the source, and a sampler is what owns the tile caches.
                let reader = TerrainReader::parse(&src)
                    .map_err(|e| format!("--terrain {}: not a usable OBCT container ({e:?})", path.display()))?;
                (reader.header().bbox_udeg(), reader.header().posting_log2)
            };
            files.push(TerrainFile { path, src, bbox, posting_log2 });
        }
        Ok(TerrainSet { files })
    }

    /// How many containers the set holds — for the pack log.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The µdeg box the whole set covers as `(min_lon, min_lat, max_lon, max_lat)` — the packer's
    /// lon-first convention, converted from the OBCT header's lat-first one. `None` when empty.
    pub fn coverage(&self) -> Option<(i64, i64, i64, i64)> {
        self.files.iter().fold(None, |acc: Option<(i64, i64, i64, i64)>, f| {
            let (min_lat, min_lon, max_lat, max_lon) = f.bbox;
            Some(match acc {
                None => (min_lon, min_lat, max_lon, max_lat),
                Some(b) => (b.0.min(min_lon), b.1.min(min_lat), b.2.max(max_lon), b.3.max(max_lat)),
            })
        })
    }

    /// The **coarsest** sample posting in the set as a `log2`, or `None` when the set is empty.
    ///
    /// Read by the contour tracer ([`crate::contour`]), which walks one lattice across the whole
    /// set. Taking the coarsest is what makes that single lattice legal everywhere: postings are
    /// powers of two on the shared grid origin (`OBCT_Spec.md` §1), so every coarse lattice point is
    /// also a lattice point of any finer container — and a query that lands exactly on a sample
    /// reads that sample back rather than interpolating. Taking the *finest* would ask a coarse
    /// container for points between its samples and trace the bilinear ramp instead of the DEM.
    pub fn posting_log2(&self) -> Option<u8> {
        self.files.iter().map(|f| f.posting_log2).max()
    }

    /// A sampler over every container whose rectangle intersects `bbox` (µdeg
    /// `(min_lon, min_lat, max_lon, max_lat)` — the packer's global-bbox convention), or over all of
    /// them when `bbox` is `None`.
    ///
    /// Filtering is not an optimisation detail: the cutter builds one of these per cell, and without
    /// it every cell would re-validate every container in the set.
    pub fn sampler_for(&self, bbox: Option<(i64, i64, i64, i64)>) -> Result<TerrainSampler<'_>, String> {
        let mut open = Vec::new();
        for f in &self.files {
            if let Some((min_lon, min_lat, max_lon, max_lat)) = bbox {
                let (fmin_lat, fmin_lon, fmax_lat, fmax_lon) = f.bbox;
                if fmax_lat < min_lat || fmin_lat > max_lat || fmax_lon < min_lon || fmin_lon > max_lon {
                    continue;
                }
            }
            let reader = TerrainReader::parse(&f.src)
                .map_err(|e| format!("--terrain {}: not a usable OBCT container ({e:?})", f.path.display()))?;
            open.push(OpenTerrain { reader, cache: TileCache::new() });
        }
        Ok(TerrainSampler { open, last: 0 })
    }
}

/// One parsed container plus the tiles it has resident.
struct OpenTerrain<'a> {
    reader: TerrainReader<'a>,
    cache: TileCache<HOST_TILE_SLOTS>,
}

/// An [`ElevationSource`] over a [`TerrainSet`], for one thread.
///
/// Dispatch is a linear scan with a **last-hit memo**, because that is what the access pattern
/// deserves: a sweep along one edge stays inside one container for hundreds of samples, and even a
/// region-wide bake sees only a handful of containers per cell after [`TerrainSet::sampler_for`]'s
/// filter. An index would be a lot of machinery for a scan that is almost always length one.
pub struct TerrainSampler<'a> {
    open: Vec<OpenTerrain<'a>>,
    last: usize,
}

impl TerrainSampler<'_> {
    /// Whether this sampler has any coverage at all. One with none answers `None` everywhere and is
    /// therefore indistinguishable from `NullElevation` — the honest answer for a cell outside the
    /// terrain the operator supplied.
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

impl ElevationSource for TerrainSampler<'_> {
    fn sample(&mut self, lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        // The memo first, then everything else. A container that does not cover the point answers
        // `None` from its own bounds check, so "try it and see" is also the coverage test.
        let n = self.open.len();
        for k in 0..n {
            let i = (self.last + k) % n;
            let slot = &mut self.open[i];
            if let Some(h) = slot.reader.sample(&mut slot.cache, lat_udeg, lon_udeg) {
                self.last = i;
                return Some(h);
            }
        }
        None
    }
}

/// Every `*.obcd` under `dir`, recursively.
fn collect_obcd(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("--terrain {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("--terrain {}: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_obcd(&path, out)?;
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("obcd")) {
            out.push(path);
        }
    }
    Ok(())
}

/// A [`ByteSource`] over a file on disk, read at absolute offsets.
///
/// `Mutex<File>` rather than a `RefCell`: a [`TerrainSet`] is shared across the cutter's rayon
/// workers and every one of their samplers borrows the *same* handle, so the seek/read pair has to
/// be atomic. The lock is held for one ≤ 512-byte read and each sampler's 32 KB tile cache keeps
/// those rare, so the contention is nothing next to the GEOS work the same threads are doing.
struct FileSource {
    file: Mutex<File>,
    len: u32,
}

impl FileSource {
    fn open(path: &Path) -> Result<FileSource, String> {
        let file = File::open(path).map_err(|e| format!("--terrain {}: {e}", path.display()))?;
        let bytes = file.metadata().map_err(|e| format!("--terrain {}: {e}", path.display()))?.len();
        let len = u32::try_from(bytes)
            .map_err(|_| format!("--terrain {}: {bytes} bytes exceeds the 4 GiB OBCT offset space", path.display()))?;
        Ok(FileSource { file: Mutex::new(file), len })
    }
}

impl ByteSource for FileSource {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error> {
        use std::io::{Read, Seek, SeekFrom};
        let end = (offset as u64).checked_add(buf.len() as u64).ok_or(Error::BadOffset)?;
        if end > self.len as u64 {
            return Err(Error::BadOffset);
        }
        let mut file = self.file.lock().map_err(|_| Error::BadOffset)?;
        file.seek(SeekFrom::Start(offset as u64)).map_err(|_| Error::BadOffset)?;
        file.read_exact(buf).map_err(|_| Error::BadOffset)
    }

    fn len(&self) -> u32 {
        self.len
    }
}
