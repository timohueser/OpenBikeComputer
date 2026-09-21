//! The input side of an assembly: one baked cell, opened through the real reader, plus the
//! preconditions an assembler must refuse on.
//!
//! Nothing here decodes geometry. A cell is opened only far enough to learn where its regions are —
//! the LOD table, the POI directory, the nav directory — and to check that it is the cell it claims
//! to be, its header bbox being its grid square. The bytes below those directories are copied, not
//! parsed.

use obc_formats::io::ByteSource;
use obc_formats::obcm::{HEADER_LEN, NAV_PROFILE_LEN, STYLE_RECORD_LEN};
use obc_reader::{Lod, MapCache, MapTables, NavDirectory, PoiDirectory, Reader};

use crate::grid::CellId;
use crate::{Error, Result};

/// Block size for a verbatim region copy. Big enough that a cell's chunk region moves in a handful
/// of reads, small enough that the engine's peak working set stays independent of cell size — which
/// is what lets a browser assemble a country.
const COPY_BLOCK: usize = 256 * 1024;

/// One cell handed to the assembler: which cell it is, which band it belongs to, and where its
/// bytes are. `band` is not inferable from the bytes, because a legitimately empty cell is
/// indistinguishable from an out-of-band one, so the caller states it from the catalog.
pub struct CellInput<'a> {
    pub id: CellId,
    pub band: String,
    pub src: &'a dyn ByteSource,
    /// The catalog's `partial` flag. An assembler refuses a partial cell unless the caller has
    /// accepted the reduced coverage.
    pub partial: bool,
}

/// A cell opened for grafting: its directories, resident, plus the raw style and profile tables
/// the cross-cell agreement checks compare.
pub struct Cell<'a> {
    pub id: CellId,
    pub band: String,
    pub src: &'a dyn ByteSource,
    pub partial: bool,
    /// Per-LOD regions, ladder order.
    pub lods: Vec<Lod>,
    pub pois: PoiDirectory,
    pub nav: NavDirectory,
    /// The profile table, verbatim — copied into the output after every cell is checked to agree.
    pub profile_table: Vec<u8>,
    /// Canonical style ids. The skin must preserve this assignment.
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
        // The file's unit, checked once at the door and then relied on everywhere. The graft copies
        // a cell's offset-table entries with a constant added, and those entries count units, not
        // bytes — so a cell whose unit is not the assembly's would relocate onto a byte the
        // output's own scale cannot name, or, worse, onto one it can: an entry read at `U = 1` and
        // re-emitted at `U = 16` addresses a plausible place sixteen times too far in.
        if tables.scale() != crate::emit::SCALE {
            return Err(Error::Format(format!(
                "cell {}: its offsets count {}-byte units but this assembly writes {}-byte ones (OBCM §1.1)",
                input.id,
                tables.scale().unit(),
                crate::emit::SCALE.unit()
            )));
        }
        // The header bbox must be exactly the grid square — the one place the packer's usual "bbox
        // is what the content covers" rule is inverted, and the fact the whole graft rests on.
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
            bytes: src.len(),
        })
    }

    /// This cell's LOD `i` region, or an error if the ladder is shorter than the schema's.
    pub fn lod(&self, i: usize) -> Result<Lod> {
        self.lods.get(i).copied().ok_or_else(|| {
            Error::Format(format!("cell {}: no ladder level {i} (it writes {})", self.id, self.lods.len()))
        })
    }

    /// Read `len` bytes at `offset`. The offset is a position in a file, so it is a `u64`; the
    /// length is a buffer this engine allocates, so it stays `usize`. The distinction matters here
    /// because this engine also runs in a browser tab, where `usize` is 32 bits and the address
    /// space, not the format, is what bounds an allocation.
    pub fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        read_at(self.src, offset, len)
    }

    /// Read `buf.len()` bytes at `offset` into a buffer the caller owns — [`Cell::read`] without the
    /// allocation, for a loop that runs once per record and must not turn each one into a `Vec`.
    pub fn read_into(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        self.src.read_at(offset, buf).map_err(Error::Io)
    }

    /// Stream `len` bytes at `offset` through `sink` in [`COPY_BLOCK`] pieces — the verbatim copy,
    /// which never materialises a whole cell region.
    pub fn copy(&self, offset: u64, len: usize, sink: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()> {
        let mut buf = vec![0u8; COPY_BLOCK.min(len.max(1))];
        let mut done = 0usize;
        while done < len {
            let take = COPY_BLOCK.min(len - done);
            let part = &mut buf[..take];
            self.src.read_at(offset + done as u64, part).map_err(Error::Io)?;
            sink(part)?;
            done += take;
        }
        Ok(())
    }
}

/// Read a byte range from any source, with the offset arithmetic checked.
pub fn read_at(src: &dyn ByteSource, offset: u64, len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; len];
    if len == 0 {
        return Ok(out);
    }
    src.read_at(offset, &mut out).map_err(Error::Io)?;
    Ok(out)
}

/// Preserve the canonical band assignment from the table already read for style ids.
fn read_style_ids(src: &dyn ByteSource) -> Result<Vec<u8>> {
    let header = read_at(src, 0, HEADER_LEN)?;
    // Through the file's own `Offset Scale`: the header is 57 bytes, so the table does not start
    // where the header ends and the field is the only thing that says where it does.
    let style_offset = crate::emit::header_style_offset(&header)
        .ok_or_else(|| Error::Format("the cell's `Style Offset` does not resolve (OBCM §1.1)".into()))?;
    let count = read_at(src, style_offset, 1)?[0] as usize;
    let table = read_at(src, style_offset + 1, count * STYLE_RECORD_LEN)?;
    let mut styles = Vec::with_capacity(count);
    for r in table.as_chunks::<STYLE_RECORD_LEN>().0 {
        styles.push(r[0]);
    }
    Ok(styles)
}

/// The cross-cell preconditions: one OBCM version (the reader already enforced it), one style-id
/// assignment, one profile table. A hole or a partial cell is legal; silently is not.
pub fn check_agreement(cells: &[Cell<'_>], accept_partial: bool) -> Result<()> {
    let Some(first) = cells.first() else {
        return Err(Error::Input(
            "an assembly needs at least one OBCM cell artifact to verify its binary tables".into(),
        ));
    };
    // One cell per (band, id). Geometry would survive a duplicate, because the graft keys cells by
    // their grid slot, but the nav merge would not: it mints fresh node ids per copy, so every
    // interior junction of a duplicated `network` cell becomes two coincident nodes off a boundary
    // line, which unification refuses to join. The result is a doubled interior graph that verifies
    // as correct, at double the projected size.
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
                "cells {} and {} disagree on the style table's ids ({} vs {} entries) — they are not one schema \
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
