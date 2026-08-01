//! The input side of an assembly: one baked cell, opened through the **real reader**, plus the
//! [`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §4.1 preconditions an assembler must refuse on.
//!
//! Nothing here decodes geometry. A cell is opened only far enough to learn where its regions are
//! (the LOD table, the POI directory, the nav directory) and to check that it is the cell it claims
//! to be — its header bbox **is** its grid square (§3.1). The bytes below those directories are then
//! copied, not parsed, which is the whole point of §2.

use obc_formats::io::ByteSource;
use obc_formats::obcm::{HEADER_LEN, NAV_PROFILE_LEN, STYLE_RECORD_LEN};
use obc_reader::{Lod, MapCache, MapTables, NavDirectory, PoiDirectory, Reader};

use crate::grid::CellId;
use crate::{Error, Result};

/// Block size for a verbatim region copy. Big enough that a cell's chunk region moves in a handful
/// of reads, small enough that the engine's peak working set stays independent of cell size — which
/// is what lets a browser assemble a country.
const COPY_BLOCK: usize = 256 * 1024;

/// Byte offset of the header's `Style Offset` field (`OBCM_Spec.md` §1: magic 4, version 1, four
/// `int32` bbox fields — `4 + 1 + 16`).
const HEADER_STYLE_OFFSET_AT: usize = 21;

/// One cell handed to the assembler: which cell it is, which band it belongs to, and where its bytes
/// are. `band` is **not** inferable from the bytes (§3.1: a legitimately empty cell is
/// indistinguishable from an out-of-band one), so the caller states it — from the catalog.
pub struct CellInput<'a> {
    pub id: CellId,
    pub band: String,
    pub src: &'a dyn ByteSource,
    /// The catalog's `partial` flag (§3.7). An assembler refuses a partial cell unless the caller
    /// has accepted the reduced coverage.
    pub partial: bool,
}

/// A cell opened for grafting: its directories, resident, plus the raw style and profile tables the
/// §4.1 cross-cell agreement checks compare.
pub struct Cell<'a> {
    pub id: CellId,
    pub band: String,
    pub src: &'a dyn ByteSource,
    pub partial: bool,
    /// Per-LOD regions, ladder order (`OBCM_Spec.md` §3).
    pub lods: Vec<Lod>,
    pub pois: PoiDirectory,
    pub nav: NavDirectory,
    /// The §8.6 profile table, verbatim — copied into the output after every cell is checked to
    /// agree (§4.3).
    pub profile_table: Vec<u8>,
    /// The style table's ids and count, for the §4.1 agreement check. Values are the cell's
    /// placeholders and are replaced by the skin (§4.7), so only the ids matter.
    pub style_ids: Vec<u8>,
    pub bytes: u64,
}

impl<'a> Cell<'a> {
    /// Open a cell: parse it with the real reader, then check it is the cell it claims to be.
    pub fn open(input: CellInput<'a>, cache: &MapCache) -> Result<Cell<'a>> {
        let src = input.src;
        let tables = MapTables::parse(src).map_err(|e| {
            Error::Format(format!(
                "cell {}: not a readable OBCM v{} file ({e:?})",
                input.id,
                obc_formats::obcm::VERSION
            ))
        })?;
        // The header bbox MUST be exactly the grid square (§3.1) — the one place the packer's usual
        // "bbox is what the content covers" rule is inverted, and the fact the whole graft rests on.
        let (min_lon, min_lat, max_lon, max_lat) = input.id.square();
        let b = tables.bbox;
        if (b.min_lon as i64, b.min_lat as i64, b.max_lon as i64, b.max_lat as i64)
            != (min_lon, min_lat, max_lon, max_lat)
        {
            return Err(Error::Format(format!(
                "cell {}: header bbox ({}, {}, {}, {}) is not its grid square ({min_lon}, {min_lat}, {max_lon}, \
                 {max_lat}) — OBCA §3.1",
                input.id, b.min_lon, b.min_lat, b.max_lon, b.max_lat
            )));
        }

        let (lods, pois, nav) = {
            let reader = Reader::new(src, &tables, cache);
            (reader.lods().to_vec(), reader.poi_directory().clone(), *reader.nav_directory())
        };
        let profile_table = read_at(src, nav.profile_table_offset, nav.profile_count * NAV_PROFILE_LEN)?;
        let style_ids = read_style_ids(src)?;
        Ok(Cell {
            id: input.id,
            band: input.band,
            src,
            partial: input.partial,
            lods,
            pois,
            nav,
            profile_table,
            style_ids,
            bytes: src.len() as u64,
        })
    }

    /// This cell's LOD `i` region, or an error if the ladder is shorter than the schema's.
    pub fn lod(&self, i: usize) -> Result<Lod> {
        self.lods.get(i).copied().ok_or_else(|| {
            Error::Format(format!("cell {}: no ladder level {i} (it writes {})", self.id, self.lods.len()))
        })
    }

    /// Read `len` bytes at `offset`.
    pub fn read(&self, offset: usize, len: usize) -> Result<Vec<u8>> {
        read_at(self.src, offset, len)
    }

    /// Stream `len` bytes at `offset` through `sink` in [`COPY_BLOCK`] pieces — the verbatim copy of
    /// §2.3, which never materialises a whole cell region.
    pub fn copy(&self, offset: usize, len: usize, sink: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()> {
        let mut buf = vec![0u8; COPY_BLOCK.min(len.max(1))];
        let mut done = 0usize;
        while done < len {
            let take = COPY_BLOCK.min(len - done);
            let part = &mut buf[..take];
            let at = u32::try_from(offset + done).map_err(|_| Error::Io(obc_formats::io::Error::BadOffset))?;
            self.src.read_at(at, part).map_err(Error::Io)?;
            sink(part)?;
            done += take;
        }
        Ok(())
    }
}

/// Read a byte range from any source, with the offset arithmetic checked.
pub fn read_at(src: &dyn ByteSource, offset: usize, len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; len];
    if len == 0 {
        return Ok(out);
    }
    let at = u32::try_from(offset).map_err(|_| Error::Io(obc_formats::io::Error::BadOffset))?;
    src.read_at(at, &mut out).map_err(Error::Io)?;
    Ok(out)
}

/// The style table's ids, in table order. The values are the cell's schema placeholders — only the
/// **id set** is part of the §4.1 agreement, because the skin replaces everything else.
fn read_style_ids(src: &dyn ByteSource) -> Result<Vec<u8>> {
    let header = read_at(src, 0, HEADER_LEN)?;
    let style_offset = u32::from_le_bytes(
        header[HEADER_STYLE_OFFSET_AT..HEADER_STYLE_OFFSET_AT + 4].try_into().expect("4 bytes inside the header"),
    ) as usize;
    let count = read_at(src, style_offset, 1)?[0] as usize;
    let table = read_at(src, style_offset + 1, count * STYLE_RECORD_LEN)?;
    Ok(table.chunks_exact(STYLE_RECORD_LEN).map(|r| r[0]).collect())
}

/// OBCA §4.1's cross-cell preconditions: one OBCM version (the reader already enforced it), one
/// style-id assignment, one profile table. A hole or a partial cell is legal — silently is not.
pub fn check_agreement(cells: &[Cell<'_>], accept_partial: bool) -> Result<()> {
    let Some(first) = cells.first() else {
        return Err(Error::Input(
            "an assembly needs at least one OBCM cell artifact to verify its binary tables".into(),
        ));
    };
    // One cell per (band, id). Geometry would survive a duplicate — the graft keys cells by their
    // grid slot, so the second copy simply overwrites the first — but the nav merge would not: it
    // mints fresh node ids per copy, so every interior junction of a duplicated `network` cell
    // becomes two coincident nodes off a boundary line, which §4.6.2 refuses to unify. The result is
    // a doubled interior graph that §4.8 verifies as correct, and the projected sizes double too.
    let mut seen: std::collections::HashSet<(&str, CellId)> = std::collections::HashSet::new();
    for c in cells {
        if !seen.insert((c.band.as_str(), c.id)) {
            return Err(Error::Input(format!(
                "cell {} of band {:?} is listed twice — an assembly takes each cell once (OBCA §4.1)",
                c.id, c.band
            )));
        }
    }
    for c in cells {
        if c.style_ids != first.style_ids {
            return Err(Error::Input(format!(
                "cells {} and {} disagree on the style table's id set ({} vs {} entries) — they are not one schema \
                 revision (OBCA §4.1)",
                first.id,
                c.id,
                first.style_ids.len(),
                c.style_ids.len()
            )));
        }
        if c.profile_table != first.profile_table {
            return Err(Error::Input(format!(
                "cells {} and {} disagree on the §8.6 profile table — they are not one schema revision (OBCA §4.1)",
                first.id, c.id
            )));
        }
        if c.lods.len() != first.lods.len() {
            return Err(Error::Input(format!(
                "cells {} and {} write different ladder lengths ({} vs {})",
                first.id,
                c.id,
                first.lods.len(),
                c.lods.len()
            )));
        }
        if c.partial && !accept_partial {
            return Err(Error::Input(format!(
                "cell {} is `partial` — its sources do not cover its square (OBCA §3.7). Accept the reduced coverage \
                 explicitly, or wait for a covering bake.",
                c.id
            )));
        }
    }
    Ok(())
}
