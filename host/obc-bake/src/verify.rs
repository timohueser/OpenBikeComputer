//! "Verify the output actually opens with `obc-reader`" — meant literally.
//!
//! A header sniff would pass on a file that is a valid 40-byte header followed by garbage, and that
//! is exactly the artifact a killed packer, a full disk, or a truncated copy leaves behind. So
//! verification runs the real reader, the same crate the device runs, over the whole artifact:
//! parse the tables, then walk every LOD's quadtree and decode every feature in every chunk it
//! reaches.
//!
//! Two things make that a real gate rather than a smoke test:
//!
//! - [`DecodeStatus`] is checked, not just the `Result`. The reader is written to survive a corrupt
//!   map on a rider's SD card: it consumes an undecodable feature whole and keeps going, counting
//!   it. So a walk that succeeded can still have skipped a thousand malformed features, and only
//!   the counters say so. Any `malformed` or `capacity_dropped` fails the artifact, because the
//!   packer validates its chunk size against the reader's cap and neither can be a legitimate
//!   outcome of a good bake.
//! - The read goes through a file-backed [`ByteSource`], not a slice. A country artifact is
//!   hundreds of megabytes; verifying it must not need it resident, and reading it through
//!   `read_at` is also closer to how the device sees it.
//!
//! Verification happens on the temporary file, before it is renamed into the bake tree, so a failed
//! artifact never exists at a path the catalog generator walks. That is the mechanism behind "a
//! corrupted artifact never reaches the manifest": not a check the publisher performs, but a file
//! that was never there.
//!
//! An empty cell is not a failure — open sea, or a `network`-band square with no roads — so the
//! walk has no "must contain features" rule; what it reports back is only what the caller then
//! checks against the id.
use std::path::Path;

use obc_file_source::FileSource;
use obc_formats::io::ByteSource;
use obc_map_scene::BBox;
use obc_reader::{MapCache, MapTables, Reader, MAX_FEAT_PTS, MAX_FEAT_RINGS};

/// What a verified artifact states about itself once the whole of it has been walked.
///
/// Only what a caller acts on: the walk's real product is the absence of an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verified {
    pub obcm_version: u8,
    pub bbox: BBox,
    /// Whether the cell carries a landmark section. The credit for what is in it lives in the
    /// catalog, not in the cell, so a store that has one and no catalog block is unlicensed.
    pub landmarks: bool,
}

/// The full walk for one cell artifact, plus the one check that makes it a cell: its header bbox
/// must be exactly its grid square.
///
/// The bbox is checked against the id rather than merely for sanity, because that identity is what
/// lets an assembler graft the cell's chunk bytes in without decoding them — a cell whose header
/// disagrees with its id would land its geometry somewhere else, silently.
pub fn verify_cell(path: &Path, square: (i64, i64, i64, i64)) -> Result<Verified, String> {
    let verified = walk(path)?;
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

fn walk(path: &Path) -> Result<Verified, String> {
    let src = open_map(path)?;
    let tables = MapTables::parse(&src).map_err(|e| format!("{}: not a readable OBCM map: {e:?}", path.display()))?;
    // The cache is about 277 KB — heap, never the stack.
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

    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    for lod in 0..reader.lods().len() {
        // Collect first: `for_each_feature` borrows the same cache the walk does.
        let mut chunks: Vec<(u32, BBox)> = Vec::new();
        reader
            .for_each_chunk(lod, &bbox, |id, node| chunks.push((id, node)))
            .map_err(|e| format!("{}: LOD{lod} index walk failed: {e:?}", path.display()))?;
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
        }
    }

    Ok(Verified { obcm_version: reader.version, bbox, landmarks: landmark_section(path, &src)? })
}

/// What a cell states about itself in its header: version, bbox, and whether it carries a landmark
/// section. Sixty-five bytes, so a whole cell store can be checked against its ids and against the
/// catalog's landmark block without decoding a chunk.
pub fn header_of(path: &Path) -> Result<Verified, String> {
    let src = open_map(path)?;
    let tables = MapTables::parse(&src).map_err(|e| format!("{}: not a readable OBCM map: {e:?}", path.display()))?;
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);
    Ok(Verified { obcm_version: reader.version, bbox: reader.bbox, landmarks: landmark_section(path, &src)? })
}

/// Whether the cell carries a landmark section, read through the device's own accessor so a
/// nonsense offset fails here rather than on a rider's card.
fn landmark_section(path: &Path, src: &FileSource) -> Result<bool, String> {
    obc_reader::landmarks::map_section(src)
        .map(|section| section.is_some())
        .map_err(|e| format!("{}: unreadable landmark section: {e:?}", path.display()))
}

/// How much of a cell store to open.
#[derive(Debug, Clone, Copy)]
pub struct CellTreeVerifyOptions {
    /// Full reader round-trip and digest on one cell in every `sample` — the spot check. `1` opens
    /// every cell; `0` opens none. Every cell is still checked for size and for
    /// header-bbox-equals-its-id, which needs 40 bytes.
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
    /// The terrain artifact class, counted separately because it is priced separately.
    pub terrain_cells: usize,
    pub terrain_bytes: u64,
    /// The landmark artifact class: one artifact per region that has one.
    pub landmark_artifacts: usize,
    pub landmark_bytes: u64,
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
        if self.terrain_cells > 0 {
            let _ = writeln!(s, "terrain:   {} cell(s), {} bytes", self.terrain_cells, self.terrain_bytes);
        }
        if self.landmark_artifacts > 0 {
            let _ = writeln!(s, "landmarks: {} artifact(s), {} bytes", self.landmark_artifacts, self.landmark_bytes);
        }
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

/// Verify a published cell tree against its own catalog.
///
/// The catalog is the thing a consumer trusts, so it is the thing this checks against — every claim
/// in it, back to the bytes:
///
/// 1. The satellites and previews are the ones the root pinned, by `bytes` and `sha256` per object.
/// 2. Every cell's header bbox is its id. Cheap and total, because it is the check the catalog
///    deliberately has no field for: the identifier states the coverage and the bytes are made to
///    agree with the identifier.
/// 3. Spot reader round-trips. A sampled cell is opened with the real reader and walked whole —
///    every chunk, every feature — and re-hashed against the manifest.
/// 4. The region lists resolve. Every cell a region names is in its band's index, and the root's
///    `bytes_by_band` adds up to its `bytes`, which is what the pre-download projection is
///    arithmetic over.
///
/// Problems are collected rather than thrown, so one run names everything wrong with a store
/// instead of the first thing.
pub fn verify_cell_tree(tree: &Path, opts: CellTreeVerifyOptions) -> Result<CellTreeReport, String> {
    use obc_pack::catalog::{parse_strict_id, Catalog, CellIndexDocument, RegionCellsDocument};
    use std::collections::{BTreeMap, BTreeSet};

    crate::planet::check_publishable_tree(tree)?;
    let root_path = tree.join(obc_pack::catalog::DEFAULT_MANIFEST_NAME);
    let text = std::fs::read_to_string(&root_path).map_err(|e| {
        format!(
            "{}: {e} — verify reads a tree's generated catalog; run the bake or `obc-pack catalog` first",
            root_path.display()
        )
    })?;
    let root: Catalog = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", root_path.display()))?;
    let mut report = CellTreeReport::default();
    let problem = |s: String, into: &mut Vec<String>| into.push(s);

    if root.schema_version != obc_pack::catalog::CATALOG_SCHEMA_VERSION {
        return Err(format!("{}: unsupported catalog schema_version {}", root_path.display(), root.schema_version));
    }
    if root.schema.obcm_version != obc_formats::obcm::VERSION {
        problem(
            format!(
                "the catalog publishes OBCM v{} but this build reads v{} — every cell in the store is unreadable to \
                 it (OBCC_Spec.md §10)",
                root.schema.obcm_version,
                obc_formats::obcm::VERSION
            ),
            &mut report.problems,
        );
    }

    // 1, per optional skin preview.
    for skin in &root.skins {
        let Some(pin) = &skin.preview else { continue };
        let path = tree.join(crate::previews::PREVIEWS_DIR).join(format!("{}.png", skin.id));
        match crate::hash::file(&path) {
            Ok((bytes, _)) if bytes != pin.bytes => problem(
                format!("{}: {bytes} bytes on disk, {} pinned in the root", path.display(), pin.bytes),
                &mut report.problems,
            ),
            Ok((_, sha)) if sha != pin.sha256 => {
                problem(format!("{}: sha256 {sha}, root pinned {}", path.display(), pin.sha256), &mut report.problems)
            }
            Ok(_) => {}
            Err(e) => problem(format!("{e} — the root pins this skin preview"), &mut report.problems),
        }
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
            let id = match parse_strict_id(&entry.id) {
                Ok(id) => id,
                Err(e) => {
                    problem(format!("{rel}: {e}"), &mut report.problems);
                    continue;
                }
            };
            if id.log2 != u32::from(band.cell_log2) {
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
            let (sq_min_lon, sq_min_lat, sq_max_lon, sq_max_lat) = id.square();
            match header_of(&path) {
                Ok(header) => {
                    if header.obcm_version != root.schema.obcm_version {
                        problem(
                            format!(
                                "{}: OBCM v{}, but the catalog says v{}",
                                path.display(),
                                header.obcm_version,
                                root.schema.obcm_version
                            ),
                            &mut report.problems,
                        );
                    }
                    // The licence-breaking direction, and the reason the header read reports the
                    // section at all: a cell's landmark bytes are Wikipedia text and Commons
                    // photos, and the only place their credit is published is the catalog's
                    // landmark block. Deleting the artifact from the tree and regenerating is all
                    // it takes to ship one without the other. Every cell, not a sample: one cell
                    // out of thousands can be the only one with a section.
                    if header.landmarks && root.landmarks.is_none() {
                        problem(
                            format!(
                                "{}: carries a landmark section, but the catalog publishes no landmark block — the \
                                 credit for that text and those photos is nowhere in the store (OBCC_Spec.md §14.2)",
                                path.display()
                            ),
                            &mut report.problems,
                        );
                    }
                    let bbox = header.bbox;
                    let got = (
                        i64::from(bbox.min_lat),
                        i64::from(bbox.min_lon),
                        i64::from(bbox.max_lat),
                        i64::from(bbox.max_lon),
                    );
                    let want = (sq_min_lat, sq_min_lon, sq_max_lat, sq_max_lon);
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
                if let Err(e) = verify_cell(&path, (sq_min_lon, sq_min_lat, sq_max_lon, sq_max_lat)) {
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

    // 1 + 2, for the terrain artifact class. Same machinery, its own document: the pinned index
    // must be the bytes the root named, and every terrain cell's container must state exactly the
    // 1 × 1 rectangle its id names. Its revision is not compared with the schema's — that is the
    // independence, and comparing them here would quietly reintroduce the lockstep the two tracks
    // exist to remove.
    let mut terrain_published: BTreeSet<String> = BTreeSet::new();
    if let Some(terrain) = &root.terrain {
        let rel = format!("cells/{}/index.json", obc_pack::catalog::TERRAIN_DIR);
        let pin = &terrain.cell_index;
        if let Some(doc) = satellite::<obc_pack::catalog::TerrainIndexDocument>(
            tree,
            &rel,
            pin.bytes,
            &pin.sha256,
            &mut report.problems,
        ) {
            if (doc.terrain_revision, doc.dataset_version.as_str(), doc.posting_log2, doc.cell_log2)
                != (terrain.terrain_revision, terrain.dataset_version.as_str(), terrain.posting_log2, terrain.cell_log2)
            {
                problem(
                    format!(
                        "{rel}: says {} {} at posting 2^{} / cell 2^{}, the root says {} {} at 2^{} / 2^{}",
                        doc.dataset_version,
                        doc.terrain_revision,
                        doc.posting_log2,
                        doc.cell_log2,
                        terrain.dataset_version,
                        terrain.terrain_revision,
                        terrain.posting_log2,
                        terrain.cell_log2
                    ),
                    &mut report.problems,
                );
            }
            if doc.cells.len() as u32 != pin.cell_count {
                problem(
                    format!("{rel}: holds {} terrain cells but the root pinned {}", doc.cells.len(), pin.cell_count),
                    &mut report.problems,
                );
            }
            report.terrain_cells = doc.cells.len();
            for entry in &doc.cells {
                terrain_published.insert(entry.id.clone());
                report.terrain_bytes += entry.bytes;
                let mut parts = entry.id.split('/');
                let (_, i, j) = (parts.next(), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                let path = tree.join("cells").join(obc_pack::catalog::TERRAIN_DIR).join(i).join(format!("{j}.obcd"));
                match crate::hash::file(&path) {
                    Ok((bytes, _)) if bytes != entry.bytes => problem(
                        format!("{}: {bytes} bytes on disk, {} in the catalog", path.display(), entry.bytes),
                        &mut report.problems,
                    ),
                    Ok((_, sha)) if sha != entry.sha256 => problem(
                        format!("{}: sha256 {sha} but the catalog pinned {}", path.display(), entry.sha256),
                        &mut report.problems,
                    ),
                    // The real reader, not a header sniff: `TerrainSet::open` is the same
                    // `obc-elevation` parse the packer and the device run, so a container that
                    // reads here reads everywhere.
                    Ok(_) => {
                        if let Err(e) = obc_pack::terrain::TerrainSet::open(&path) {
                            problem(e, &mut report.problems);
                        }
                    }
                    Err(e) => problem(format!("{e} — the catalog publishes it"), &mut report.problems),
                }
            }
        }
    }

    // 1 + 2, for the landmark artifact class. It has no index and no cells: one directory per
    // region, keyed by one digest over its files. So the checks are that digest, the recipe the
    // artifact declares, and the photos the content document says are beside it — the same read
    // the cell bake does, so an artifact that verifies here is one a re-bake would cut the same
    // cells from.
    verify_landmarks(tree, &root, &mut report);

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
                format!("region `{}`: bytes_by_band sums to {summed} but bytes is {} — the per-file projection of `OBCA_Spec.md` is arithmetic over exactly these numbers", region.id, region.bytes),
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
                        format!("{rel}: names cell `{id}` in band `{band}`, which is not published (OBCC_Spec.md §6)"),
                        &mut report.problems,
                    );
                }
            }
        }
        // A terrain id a region names must be an artifact or a known-empty square. Known-empty ones
        // are not in `terrain_published`, so only a missing id is reported; the generator already
        // refuses to publish a region naming one that is neither.
        let priced = region.terrain.as_ref().map_or(0, |t| t.cell_count);
        let listed = doc.terrain.iter().filter(|id| terrain_published.contains(*id)).count() as u32;
        if listed != priced {
            problem(
                format!(
                    "{rel}: lists {listed} downloadable terrain cell(s) but the root prices {priced} — a rider's \
                     terrain estimate would be wrong before the first byte moves"
                ),
                &mut report.problems,
            );
        }
    }

    Ok(report)
}

/// Every landmark artifact the catalog publishes, against the tree it published from.
fn verify_landmarks(tree: &Path, root: &obc_pack::catalog::Catalog, report: &mut CellTreeReport) {
    use crate::landmarks::{LandmarkDoc, LANDMARK_DOC, LANDMARK_RECIPE_VERSION};

    let published: std::collections::BTreeSet<&str> =
        root.landmarks.iter().flat_map(|l| l.artifacts.iter().map(|a| a.region_id.as_str())).collect();
    match obc_pack::catalog::landmark_artifact_dirs(tree) {
        Ok(dirs) => {
            for (id, dir) in dirs.iter().filter(|(id, _)| !published.contains(id.as_str())) {
                report.problems.push(format!(
                    "{}: a landmark artifact for `{id}` that the catalog does not record — re-generate the catalog \
                     with `obc-pack catalog`, or take the directory out of the tree",
                    dir.display()
                ));
            }
        }
        Err(e) => report.problems.push(e),
    }

    let Some(landmarks) = &root.landmarks else { return };
    for entry in &landmarks.artifacts {
        report.landmark_artifacts += 1;
        report.landmark_bytes += entry.bytes;
        let dir = entry.region_id.split('/').fold(tree.join(obc_pack::catalog::LANDMARKS_DIR), |p, seg| p.join(seg));
        let content = dir.join(obc_pack::landmarks::CONTENT_DOC);
        if !content.is_file() {
            report.problems.push(format!("{}: the catalog publishes this landmark artifact", content.display()));
            continue;
        }
        // The artifact's own declaration: the recipe it was compiled under, and the digest it
        // claims for its files.
        let doc: LandmarkDoc = match std::fs::read_to_string(dir.join(LANDMARK_DOC))
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
        {
            Ok(doc) => doc,
            Err(e) => {
                report.problems.push(format!("{}: {e} — the catalog publishes it", dir.join(LANDMARK_DOC).display()));
                continue;
            }
        };
        let stale = [
            (doc.recipe_version != LANDMARK_RECIPE_VERSION, format!("recipe v{}", doc.recipe_version)),
            (
                doc.policy_sha256 != crate::hash::bytes(obc_pack::landmarks::POLICY_BYTES),
                "another category policy".to_string(),
            ),
            (
                doc.language_sha256 != crate::hash::bytes(obc_pack::landmarks::LANGUAGE_BYTES),
                "another UI language set".to_string(),
            ),
        ];
        for (_, what) in stale.iter().filter(|(moved, _)| *moved) {
            report.problems.push(format!(
                "{}: compiled under {what}, but this build compiles landmarks at recipe v{LANDMARK_RECIPE_VERSION} \
                 — re-run `obc bake landmarks {}`",
                dir.display(),
                entry.region_id
            ));
        }
        match obc_pack::landmarks::artifact_digest(&dir) {
            Ok((sha256, bytes)) => {
                if sha256 != doc.artifact_sha256 || sha256 != entry.sha256 || bytes != entry.bytes {
                    report.problems.push(format!(
                        "{}: hashes to {sha256} over {bytes} bytes; its declaration says {} and the catalog says {} \
                         over {} bytes — a file beside the content document was lost, renamed or replaced",
                        dir.display(),
                        doc.artifact_sha256,
                        entry.sha256,
                        entry.bytes
                    ));
                }
            }
            Err(e) => report.problems.push(e),
        }
        if doc.languages != entry.languages {
            report.problems.push(format!(
                "{}: stores {:?} but the catalog publishes {:?}",
                dir.display(),
                doc.languages,
                entry.languages
            ));
        }
        // The cell bake's own read: it parses the content and checks every declared photo's bytes
        // and digest, so it fails exactly where a cut would have failed.
        if let Err(e) = obc_pack::landmark_map::fingerprint(std::slice::from_ref(&content)) {
            report.problems.push(format!("{}: {e}", content.display()));
        }
    }
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
             and the root retained (OBCC_Spec.md §9)"
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

/// The artifact as a [`ByteSource`]: positioned reads, nothing resident.
///
/// OBCM addresses bytes with `u32` offsets, so a >4 GB artifact is not a map the reader could ever
/// open. That is the format's wall, not the read seam's, which is why it is stated here rather than
/// inside the shared adapter.
fn open_map(path: &Path) -> Result<FileSource, String> {
    let src = FileSource::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = src.len();
    if len > u64::from(u32::MAX) {
        return Err(format!("{}: {len} bytes exceeds the format's 4 GB limit", path.display()));
    }
    Ok(src)
}
