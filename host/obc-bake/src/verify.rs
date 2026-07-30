//! "Verify the output actually opens with `obc-reader`" — meant literally.
//!
//! A header sniff would pass on a file that is a valid 40-byte header followed by
//! garbage, and that is exactly the artifact a killed packer, a full disk, or a
//! truncated copy leaves behind. So verification runs the **real reader**, the same
//! crate the device runs, over the **whole** artifact: parse the tables, then walk
//! every LOD's quadtree and decode every feature in every chunk it reaches.
//!
//! Two things make that a real gate rather than a smoke test:
//!
//! - [`DecodeStatus`] is checked, not just the `Result`. The reader is written to
//!   survive a corrupt map on a rider's SD card — it consumes an undecodable
//!   feature whole and keeps going, counting it. So a walk that "succeeded" can
//!   still have skipped a thousand malformed features, and only the counters say
//!   so. Any `malformed` or `capacity_dropped` fails the artifact: the packer
//!   validates its chunk size against the reader's cap
//!   ([`obc_pack::serialize::validate_chunk_size`]), so neither can be a legitimate
//!   outcome of a good bake.
//! - The read goes through a **file-backed** [`ByteSource`], not a slice. A country
//!   artifact is hundreds of megabytes; verifying it must not need it resident, and
//!   reading it through `read_at` is also closer to how the device sees it (small
//!   positioned reads through a cache) than a `Vec<u8>` would be.
//!
//! Verification happens on the temporary file, **before** it is renamed into the
//! bake tree, so a failed artifact never exists at a path the catalog generator
//! walks. That is the mechanism behind "a corrupted artifact never reaches the
//! manifest": not a check the publisher performs, but a file that was never there.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use obc_formats::io::{ByteSource, Error as IoError};
use obc_reader::{BBox, MapCache, MapTables, Reader, MAX_FEAT_PTS, MAX_FEAT_RINGS};

/// What a verified artifact turned out to contain. Logged per artifact so a bake
/// that produces a *readable but empty* map is still visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verified {
    pub obcm_version: u8,
    pub bbox: BBox,
    pub lods: usize,
    pub chunks: u64,
    pub features: u64,
    /// POI categories with content (spec §7.4 ids 1..=6).
    pub poi_categories: usize,
    pub has_nav_graph: bool,
}

/// Open `path` with the real reader and walk all of it.
pub fn verify(path: &Path) -> Result<Verified, String> {
    walk(path, true)
}

/// The same full walk for one **cell** artifact, plus the one check that makes it a
/// cell: its header bbox must be *exactly* its grid square
/// ([`OBCA_Spec.md` §3.1](../../../specs/OBCA_Spec.md)).
///
/// Two differences from [`verify`], and both are about what a cell legitimately is.
/// A cell may be **empty**: open sea, or a `network`-band cell in a square with no
/// roads. A whole-region artifact with no features is a failed bake; a cell with no
/// features is a fact about the ground. And the bbox is checked against the id rather
/// than merely for sanity, because that identity is what lets an assembler graft the
/// cell's chunk bytes in without decoding them — a cell whose header disagrees with
/// its id would land its geometry somewhere else, silently.
pub fn verify_cell(path: &Path, square: (i64, i64, i64, i64)) -> Result<Verified, String> {
    let verified = walk(path, false)?;
    let (min_lon, min_lat, max_lon, max_lat) = square;
    let got = (
        verified.bbox.min_lon as i64,
        verified.bbox.min_lat as i64,
        verified.bbox.max_lon as i64,
        verified.bbox.max_lat as i64,
    );
    if got != (min_lon, min_lat, max_lon, max_lat) {
        return Err(format!(
            "{}: header bbox {got:?} is not the cell's grid square {square:?} (lon/lat µdeg). A cell's bbox MUST be \
             exactly its square (OBCA_Spec.md §3.1) — that is what lets an assembler copy its chunk bytes verbatim.",
            path.display()
        ));
    }
    Ok(verified)
}

fn walk(path: &Path, require_features: bool) -> Result<Verified, String> {
    let src = FileSource::open(path)?;
    let tables = MapTables::parse(&src).map_err(|e| format!("{}: not a readable OBCM map: {e:?}", path.display()))?;
    // The cache is ~277 KB — heap, never the stack (`alloc` feature).
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);

    if reader.version != obc_formats::obcm::VERSION {
        return Err(format!(
            "{}: OBCM v{} but this build writes v{} — the artifact is stale",
            path.display(),
            reader.version,
            obc_formats::obcm::VERSION
        ));
    }
    let bbox = reader.bbox;
    if bbox.min_lon > bbox.max_lon || bbox.min_lat > bbox.max_lat {
        return Err(format!("{}: inside-out header bbox {bbox:?}", path.display()));
    }

    let mut chunks_total = 0u64;
    let mut features_total = 0u64;
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    for lod in 0..reader.lods().len() {
        // Collect first: `for_each_feature` borrows the same cache the walk does.
        let mut chunks: Vec<(u32, BBox)> = Vec::new();
        reader
            .for_each_chunk(lod, &bbox, |id, node| chunks.push((id, node)))
            .map_err(|e| format!("{}: LOD{lod} index walk failed: {e:?}", path.display()))?;
        chunks_total += chunks.len() as u64;
        for (chunk_id, node) in chunks {
            let status = reader
                .for_each_feature(lod, chunk_id, &node, &mut points, &mut ring_lens, |_| {})
                .map_err(|e| format!("{}: LOD{lod} chunk {chunk_id} unreadable: {e:?}", path.display()))?;
            if status.malformed > 0 || status.capacity_dropped > 0 {
                return Err(format!(
                    "{}: LOD{lod} chunk {chunk_id} decoded {} features but dropped {} malformed and {} oversized — \
                     the artifact is corrupt",
                    path.display(),
                    status.complete,
                    status.malformed,
                    status.capacity_dropped
                ));
            }
            features_total += u64::from(status.complete);
        }
    }
    if require_features && features_total == 0 {
        return Err(format!(
            "{}: reads cleanly but contains no features — refusing to publish an empty map",
            path.display()
        ));
    }

    Ok(Verified {
        obcm_version: reader.version,
        bbox,
        lods: reader.lods().len(),
        chunks: chunks_total,
        features: features_total,
        poi_categories: reader.poi_directory().entries.iter().filter(|e| !e.is_empty()).count(),
        has_nav_graph: tables.has_nav_graph(),
    })
}

/// The header a cell states about itself: its OBCM version and its bbox, and nothing
/// else read. Forty bytes, so a whole cell store can be checked against its ids
/// without decoding a single chunk.
pub fn header_of(path: &Path) -> Result<(u8, BBox), String> {
    let src = FileSource::open(path)?;
    let tables = MapTables::parse(&src).map_err(|e| format!("{}: not a readable OBCM map: {e:?}", path.display()))?;
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);
    Ok((reader.version, reader.bbox))
}

// --- verifying a whole cell tree (OBCA §3, OBCC §11) ------------------------------

/// How much of a cell store to open.
#[derive(Debug, Clone, Copy)]
pub struct CellTreeVerifyOptions {
    /// Full reader round-trip + digest on one cell in every `sample` — the *spot*
    /// check. `1` opens every cell; `0` opens none. Every cell is still checked for
    /// size and for header-bbox-equals-its-id, which needs 40 bytes.
    pub sample: usize,
}

impl Default for CellTreeVerifyOptions {
    fn default() -> Self {
        Self { sample: 50 }
    }
}

/// What a cell-tree verify found.
#[derive(Debug, Clone, Default)]
pub struct CellTreeReport {
    pub bands: usize,
    pub cells: usize,
    pub partial_cells: usize,
    pub regions: usize,
    /// Cells opened with the real reader and re-hashed.
    pub sampled: usize,
    pub bytes: u64,
    /// Every failed check, in the order they were made. Empty means the tree is good.
    pub problems: Vec<String>,
}

impl CellTreeReport {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "cell tree: {} cells ({} partial) across {} band(s), {} region(s), {} bytes — {} opened with the reader",
            self.cells, self.partial_cells, self.bands, self.regions, self.bytes, self.sampled
        );
        if self.problems.is_empty() {
            let _ = writeln!(s, "verify: OK");
        } else {
            let _ = writeln!(s, "\n!!! {} PROBLEM(S) !!!", self.problems.len());
            for p in self.problems.iter().take(40) {
                let _ = writeln!(s, "  {p}");
            }
            if self.problems.len() > 40 {
                let _ = writeln!(s, "  … and {} more", self.problems.len() - 40);
            }
        }
        s
    }
}

/// Verify a published cell tree against its own `schema_version 2` catalog.
///
/// The catalog is the thing a consumer trusts, so it is the thing this checks
/// *against* — every claim in it, back to the bytes:
///
/// 1. **The satellites are the ones the root pinned.** `bytes` and `sha256` per cell
///    index and per region cell list (`OBCC_Spec.md` §11.1). A satellite that does not
///    match is the failure the pinning exists to make impossible to miss.
/// 2. **Every cell's header bbox is its id.** Cheap and total, because it is the check
///    the catalog deliberately has no field for (§11.6): the identifier states the
///    coverage and the bytes are made to agree with the identifier.
/// 3. **Spot reader round-trips.** A sampled cell is opened with the real reader and
///    walked whole — every chunk, every feature — and re-hashed against the manifest.
/// 4. **The region lists resolve.** Every cell a region names is in its band's index,
///    and the root's `bytes_by_band` adds up to its `bytes` (which is what
///    `OBCA_Spec.md` §5.7's pre-download projection is arithmetic over).
///
/// Problems are collected rather than thrown, so one run names everything wrong with
/// a store instead of the first thing.
pub fn verify_cell_tree(tree: &Path, opts: CellTreeVerifyOptions) -> Result<CellTreeReport, String> {
    use obc_pack::catalog::v2::{CatalogV2, CellId, CellIndexDocument, RegionCellsDocument};
    use std::collections::{BTreeMap, BTreeSet};

    let root_path = tree.join(obc_pack::catalog::DEFAULT_MANIFEST_NAME);
    let text = std::fs::read_to_string(&root_path).map_err(|e| {
        format!(
            "{}: {e} — verify reads a tree's generated catalog; run the bake or `obc-pack catalog --v2` first",
            root_path.display()
        )
    })?;
    let root: CatalogV2 = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", root_path.display()))?;
    let mut report = CellTreeReport::default();
    let problem = |s: String, into: &mut Vec<String>| into.push(s);

    if root.schema_version != obc_pack::catalog::v2::CATALOG_SCHEMA_VERSION {
        return Err(format!(
            "{}: schema_version {} — this is not a v2 cell catalog",
            root_path.display(),
            root.schema_version
        ));
    }
    if root.schema.obcm_version != obc_formats::obcm::VERSION {
        problem(
            format!(
                "the catalog publishes OBCM v{} but this build reads v{} — every cell in the store is unreadable to \
                 it (OBCC_Spec.md §11.9)",
                root.schema.obcm_version,
                obc_formats::obcm::VERSION
            ),
            &mut report.problems,
        );
    }

    // 1 + 2 + 3, per band.
    let mut published: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for band in &root.cell_index {
        report.bands += 1;
        let rel = format!("{}/{}/index.json", "cells", band.band);
        let doc: CellIndexDocument = match satellite(tree, &rel, band.bytes, &band.sha256, &mut report.problems) {
            Some(d) => d,
            None => continue,
        };
        if doc.schema_revision != root.schema.revision || doc.band != band.band {
            problem(
                format!(
                    "{rel}: says band `{}` revision {} but the root says `{}` revision {}",
                    doc.band, doc.schema_revision, band.band, root.schema.revision
                ),
                &mut report.problems,
            );
        }
        if doc.cells.len() as u32 != band.cell_count {
            problem(
                format!("{rel}: holds {} cells but the root pinned {}", doc.cells.len(), band.cell_count),
                &mut report.problems,
            );
        }
        let ids: BTreeSet<String> = doc.cells.iter().map(|c| c.id.clone()).collect();
        published.insert(band.band.clone(), ids);

        for (n, entry) in doc.cells.iter().enumerate() {
            report.cells += 1;
            report.bytes += entry.bytes;
            if entry.partial {
                report.partial_cells += 1;
            }
            let id = match CellId::parse(&entry.id) {
                Ok(id) => id,
                Err(e) => {
                    problem(format!("{rel}: {e}"), &mut report.problems);
                    continue;
                }
            };
            if id.log2 != band.cell_log2 {
                problem(
                    format!("{rel}: cell `{}` is 2^{} but the band is 2^{}", entry.id, id.log2, band.cell_log2),
                    &mut report.problems,
                );
            }
            let mut parts = entry.id.split('/');
            let (_, i, j) = (parts.next(), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let path = tree.join("cells").join(&band.band).join(i).join(format!("{j}.obcm"));
            let bytes = match std::fs::metadata(&path) {
                Ok(m) => m.len(),
                Err(e) => {
                    problem(format!("{}: {e} — the catalog publishes it", path.display()), &mut report.problems);
                    continue;
                }
            };
            if bytes != entry.bytes {
                problem(
                    format!("{}: {bytes} bytes on disk, {} in the catalog", path.display(), entry.bytes),
                    &mut report.problems,
                );
                continue;
            }
            // Every cell: the header must be exactly the square its id names.
            let square = id.square();
            match header_of(&path) {
                Ok((version, bbox)) => {
                    if version != root.schema.obcm_version {
                        problem(
                            format!(
                                "{}: OBCM v{version}, but the catalog says v{}",
                                path.display(),
                                root.schema.obcm_version
                            ),
                            &mut report.problems,
                        );
                    }
                    let got = (bbox.min_lat, bbox.min_lon, bbox.max_lat, bbox.max_lon);
                    let want = (square.min_lat, square.min_lon, square.max_lat, square.max_lon);
                    if got != want {
                        problem(
                            format!(
                                "{}: header bbox {got:?} is not cell `{}`'s square {want:?} (OBCA_Spec.md §3.1)",
                                path.display(),
                                entry.id
                            ),
                            &mut report.problems,
                        );
                    }
                }
                Err(e) => problem(e, &mut report.problems),
            }
            // Spot check: the full walk and the digest.
            if opts.sample > 0 && n % opts.sample == 0 {
                report.sampled += 1;
                let sq = (
                    i64::from(square.min_lon),
                    i64::from(square.min_lat),
                    i64::from(square.max_lon),
                    i64::from(square.max_lat),
                );
                if let Err(e) = verify_cell(&path, sq) {
                    problem(e, &mut report.problems);
                }
                match crate::hash::file(&path) {
                    Ok((_, sha)) if sha != entry.sha256 => problem(
                        format!("{}: sha256 {sha} but the catalog pinned {}", path.display(), entry.sha256),
                        &mut report.problems,
                    ),
                    Ok(_) => {}
                    Err(e) => problem(e, &mut report.problems),
                }
            }
        }
    }

    // 4, per region.
    for region in &root.regions {
        report.regions += 1;
        let rel = format!("regions/{}/cells.json", region.id);
        let Some(doc): Option<RegionCellsDocument> =
            satellite(tree, &rel, region.cells_bytes, &region.cells_sha256, &mut report.problems)
        else {
            continue;
        };
        if doc.region_id != region.id || doc.schema_revision != root.schema.revision {
            problem(
                format!(
                    "{rel}: says `{}` revision {}, the root says `{}` revision {}",
                    doc.region_id, doc.schema_revision, region.id, root.schema.revision
                ),
                &mut report.problems,
            );
        }
        let summed: u64 = region.bytes_by_band.values().sum();
        if summed != region.bytes {
            problem(
                format!("region `{}`: bytes_by_band sums to {summed} but bytes is {} — the per-file projection of OBCA_Spec.md §5.7 is arithmetic over exactly these numbers", region.id, region.bytes),
                &mut report.problems,
            );
        }
        for (band, ids) in &doc.cells {
            let Some(index) = published.get(band) else {
                problem(format!("{rel}: band `{band}` is not in the catalog"), &mut report.problems);
                continue;
            };
            for id in ids {
                if !index.contains(id) {
                    problem(
                        format!(
                            "{rel}: names cell `{id}` in band `{band}`, which is not published (OBCC_Spec.md §11.7)"
                        ),
                        &mut report.problems,
                    );
                }
            }
        }
    }

    Ok(report)
}

/// Read a pinned satellite and check it is byte-for-byte the one the root named.
fn satellite<T: serde::de::DeserializeOwned>(
    tree: &Path,
    rel: &str,
    bytes: u64,
    sha256: &str,
    problems: &mut Vec<String>,
) -> Option<T> {
    let path = rel.split('/').fold(tree.to_path_buf(), |p, seg| p.join(seg));
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            problems.push(format!("{}: {e} — the root pins it", path.display()));
            return None;
        }
    };
    if text.len() as u64 != bytes {
        problems.push(format!("{rel}: {} bytes on disk, {bytes} pinned in the root", text.len()));
        return None;
    }
    let sha = crate::hash::text(&text);
    if sha != sha256 {
        problems.push(format!(
            "{rel}: sha256 {sha}, root pinned {sha256} — a satellite that does not match its pin MUST be rejected \
             and the root retained (OBCC_Spec.md §11.1)"
        ));
        return None;
    }
    match serde_json::from_str(&text) {
        Ok(doc) => Some(doc),
        Err(e) => {
            problems.push(format!("{rel}: {e}"));
            None
        }
    }
}

/// A [`ByteSource`] over an open file: positioned reads, nothing resident.
///
/// `RefCell` because `ByteSource::read_at` takes `&self` (the device's sources are
/// interior-mutable too) and verification is single-threaded.
struct FileSource {
    file: RefCell<File>,
    len: u32,
}

impl FileSource {
    fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let len = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
        // The format addresses bytes with u32 offsets, so a >4 GB artifact is not a
        // map the reader could ever open — say so here rather than truncating.
        let len = u32::try_from(len)
            .map_err(|_| format!("{}: {len} bytes exceeds the format's 4 GB limit", path.display()))?;
        Ok(Self { file: RefCell::new(file), len })
    }
}

impl ByteSource for FileSource {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), IoError> {
        let end = (offset as u64).checked_add(buf.len() as u64).ok_or(IoError::BadOffset)?;
        if end > u64::from(self.len) {
            return Err(IoError::BadOffset);
        }
        let mut file = self.file.try_borrow_mut().map_err(|_| IoError::Io)?;
        file.seek(SeekFrom::Start(u64::from(offset))).map_err(|_| IoError::Io)?;
        file.read_exact(buf).map_err(|_| IoError::Io)
    }

    fn len(&self) -> u32 {
        self.len
    }
}
